use super::*;
use std::collections::VecDeque;

const OBSERVABILITY_EVENT_RING_CAPACITY: usize = 256;

pub(super) struct DaemonSocketServer {
    listener: std::os::unix::net::UnixListener,
    socket_path: PathBuf,
    socket_identity: Option<SocketFileIdentity>,
    state: DaemonSocketState,
    stop: Arc<AtomicBool>,
}

#[derive(Clone, Copy)]
struct SocketFileIdentity {
    dev: u64,
    ino: u64,
}

impl SocketFileIdentity {
    fn from_path(path: &Path) -> Result<Option<Self>> {
        let metadata = fs::symlink_metadata(path)
            .with_context(|| format!("failed to stat socket path {}", path.display()))?;
        if !metadata.file_type().is_socket() {
            return Ok(None);
        }
        Ok(Some(Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
        }))
    }

    fn still_matches(self, path: &Path) -> bool {
        Self::from_path(path)
            .ok()
            .flatten()
            .is_some_and(|current| current.dev == self.dev && current.ino == self.ino)
    }
}

impl DaemonSocketServer {
    pub(super) fn bind(socket_path: &Path) -> Result<Self> {
        let listener = std::os::unix::net::UnixListener::bind(socket_path)
            .with_context(|| format!("failed to bind daemon socket {}", socket_path.display()))?;
        listener
            .set_nonblocking(true)
            .context("failed to configure daemon socket listener")?;
        Ok(Self {
            listener,
            socket_path: socket_path.to_path_buf(),
            socket_identity: SocketFileIdentity::from_path(socket_path)?,
            state: DaemonSocketState::new(),
            stop: Arc::new(AtomicBool::new(false)),
        })
    }

    pub(super) fn state(&self) -> DaemonSocketState {
        self.state.clone()
    }

    pub(super) fn spawn(self) -> DaemonSocketServerHandle {
        let stop = self.stop.clone();
        let handle_stop = self.stop.clone();
        let join = std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match self.listener.accept() {
                    Ok((stream, _)) => {
                        let state = self.state.clone();
                        if let Some(pending_handshake) = state.try_acquire_pending_handshake() {
                            std::thread::spawn(move || {
                                if let Err(error) = handle_daemon_socket_client_with_pending(
                                    stream,
                                    &state,
                                    pending_handshake,
                                ) {
                                    eprintln!("agentscan: daemon socket client failed: {error:#}");
                                }
                            });
                        } else {
                            refuse_server_busy(stream);
                        }
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                        ) =>
                    {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) if is_transient_accept_error(&error) => {
                        // Resource pressure — fd exhaustion (EMFILE/ENFILE), out of
                        // buffers, or a peer that hung up before accept completed — is
                        // recoverable: the listener works again once the condition
                        // clears. Back off a little longer than the idle poll so we
                        // don't spin the CPU while waiting, then keep accepting.
                        // Breaking here would silently turn the daemon deaf while it
                        // still holds the socket and flock, so every later client hangs.
                        eprintln!(
                            "agentscan: daemon socket accept hit transient error, retrying: {error}"
                        );
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(error) => {
                        // A genuinely broken listener (e.g. EBADF/EINVAL) cannot
                        // recover. Rather than leave a live-but-deaf daemon, request a
                        // clean shutdown: the run loop observes the flag, exits and
                        // releases the socket/flock, and the next client auto-starts a
                        // healthy daemon.
                        eprintln!(
                            "agentscan: daemon socket accept failed fatally, requesting shutdown: {error}"
                        );
                        DAEMON_SHUTDOWN_REQUESTED.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            }
        });
        DaemonSocketServerHandle {
            stop: handle_stop,
            join: Some(join),
            socket_path: self.socket_path,
            socket_identity: self.socket_identity,
        }
    }
}

/// Whether a failed `accept()` is a recoverable, transient condition (resource
/// pressure or an aborted connection) rather than a permanently broken listener.
/// Transient errors are retried; anything else is treated as fatal so the daemon
/// shuts down instead of going silently deaf.
pub(crate) fn is_transient_accept_error(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::ConnectionAborted {
        return true;
    }
    matches!(
        error.raw_os_error(),
        Some(libc::EMFILE | libc::ENFILE | libc::ENOBUFS | libc::ENOMEM)
    )
}

pub(super) struct DaemonSocketServerHandle {
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
    socket_path: PathBuf,
    socket_identity: Option<SocketFileIdentity>,
}

impl DaemonSocketServerHandle {
    pub(super) fn socket_still_matches(&self) -> bool {
        self.socket_identity
            .is_none_or(|identity| identity.still_matches(&self.socket_path))
    }

    /// Whether the acceptor thread is still running. The acceptor lives for the
    /// daemon's whole life, so a finished thread means it stopped accepting (a
    /// fatal accept error, or a panic that bypassed the shutdown flag). The run
    /// loop uses this as a defense-in-depth liveness check so a deaf daemon exits
    /// instead of lingering.
    pub(super) fn accept_thread_alive(&self) -> bool {
        self.join.as_ref().is_some_and(|join| !join.is_finished())
    }
}

impl Drop for DaemonSocketServerHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        if self
            .socket_identity
            .is_some_and(|identity| identity.still_matches(&self.socket_path))
            && let Err(error) = fs::remove_file(&self.socket_path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!(
                "agentscan: failed to remove daemon socket {}: {error}",
                self.socket_path.display()
            );
        }
    }
}

#[derive(Clone)]
pub(crate) struct DaemonSocketState {
    inner: Arc<Mutex<DaemonSocketStateInner>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum DaemonStartupState {
    Initializing,
    Ready,
    StartupFailed(String),
    Closing,
}

struct DaemonSocketStateInner {
    startup_state: DaemonStartupState,
    identity: Option<DaemonRuntimeIdentity>,
    snapshots: SnapshotStore,
    control_mode_broker: Option<ipc::ControlModeBrokerStatusFrame>,
    runtime_telemetry: Option<ipc::RuntimeTelemetryFrame>,
    recent_events: VecDeque<ipc::DaemonObservabilityEventFrame>,
    client_event_tx: Option<mpsc::Sender<Result<ControlModeLine>>>,
    pending_handshakes: usize,
    subscribers: HashMap<SubscriberId, SubscriberMailbox>,
    next_subscriber_id: SubscriberId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SnapshotUpdateTelemetry {
    pub(super) source: &'static str,
    pub(super) detail: Option<String>,
    pub(super) duration_ms: Option<u64>,
}

pub(super) struct SnapshotPublishContext {
    source: &'static str,
    detail: Option<String>,
    started_at: Option<Instant>,
}

impl SnapshotPublishContext {
    pub(super) fn new(source: &'static str) -> Self {
        Self {
            source,
            detail: None,
            started_at: Some(Instant::now()),
        }
    }

    fn initial() -> Self {
        Self {
            source: "initial_snapshot",
            detail: None,
            started_at: None,
        }
    }

    #[cfg(test)]
    fn manual() -> Self {
        Self {
            source: "manual_update",
            detail: None,
            started_at: None,
        }
    }

    pub(super) fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    fn telemetry(&self) -> SnapshotUpdateTelemetry {
        SnapshotUpdateTelemetry {
            source: self.source,
            detail: self.detail.clone(),
            duration_ms: self.started_at.map(elapsed_millis_u64),
        }
    }
}

fn elapsed_millis_u64(started_at: Instant) -> u64 {
    started_at
        .elapsed()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

pub(super) struct PreparedSnapshot {
    pub(super) snapshot: SnapshotEnvelope,
    pub(super) frame: EncodedDaemonFrame,
}

impl PreparedSnapshot {
    pub(super) fn new(snapshot: SnapshotEnvelope) -> Result<Self> {
        let frame = encode_snapshot_frame(&snapshot)?;
        Ok(Self { snapshot, frame })
    }
}

impl DaemonSocketState {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(DaemonSocketStateInner {
                startup_state: DaemonStartupState::Initializing,
                identity: None,
                snapshots: SnapshotStore::default(),
                control_mode_broker: None,
                runtime_telemetry: None,
                recent_events: VecDeque::with_capacity(OBSERVABILITY_EVENT_RING_CAPACITY),
                client_event_tx: None,
                pending_handshakes: 0,
                subscribers: HashMap::new(),
                next_subscriber_id: 1,
            })),
        }
    }

    pub(super) fn set_identity(&self, identity: DaemonRuntimeIdentity) {
        self.lock().identity = Some(identity);
    }

    #[cfg(test)]
    pub(crate) fn publish_initial_snapshot(&self, snapshot: SnapshotEnvelope) -> Result<()> {
        let prepared = PreparedSnapshot::new(snapshot)
            .context("initial daemon snapshot exceeded socket frame limit")?;
        self.publish_prepared_snapshot(prepared);
        Ok(())
    }

    pub(super) fn publish_prepared_snapshot(&self, prepared: PreparedSnapshot) {
        let context = SnapshotPublishContext::initial();
        self.publish_prepared_snapshot_with_context(prepared, context);
    }

    fn publish_prepared_snapshot_with_context(
        &self,
        prepared: PreparedSnapshot,
        context: SnapshotPublishContext,
    ) {
        let telemetry = context.telemetry();
        let (frame, subscribers) = {
            let mut inner = self.lock();
            let frame = inner.snapshots.publish(prepared, telemetry);
            inner.startup_state = DaemonStartupState::Ready;
            (frame, subscriber_mailboxes(&inner))
        };
        fan_out_snapshot(frame, subscribers);
    }

    #[cfg(test)]
    pub(crate) fn publish_later_snapshot(&self, snapshot: SnapshotEnvelope) -> bool {
        self.publish_later_snapshot_with_context(snapshot, SnapshotPublishContext::manual())
    }

    pub(super) fn publish_later_snapshot_with_context(
        &self,
        snapshot: SnapshotEnvelope,
        context: SnapshotPublishContext,
    ) -> bool {
        let telemetry = context.telemetry();
        match encode_snapshot_frame(&snapshot) {
            Ok(frame) => {
                let prepared = PreparedSnapshot { snapshot, frame };
                let (frame, subscribers) = {
                    let mut inner = self.lock();
                    let frame = inner.snapshots.publish(prepared, telemetry);
                    inner.startup_state = DaemonStartupState::Ready;
                    (frame, subscriber_mailboxes(&inner))
                };
                fan_out_snapshot(frame, subscribers);
                true
            }
            Err(error) => {
                eprintln!(
                    "agentscan: skipped daemon socket snapshot update because encoded frame exceeded {} bytes; previous good snapshot remains active: {error:#}",
                    ipc::DAEMON_FRAME_MAX_BYTES
                );
                false
            }
        }
    }

    pub(crate) fn mark_startup_failed(&self, message: String) {
        let mut inner = self.lock();
        inner.startup_state = DaemonStartupState::StartupFailed(message);
    }

    pub(crate) fn mark_closing(&self) {
        let subscribers = {
            let mut inner = self.lock();
            inner.startup_state = DaemonStartupState::Closing;
            std::mem::take(&mut inner.subscribers)
        };
        close_subscribers(subscribers);
    }

    fn snapshot_response(&self) -> DaemonSocketResponse {
        let inner = self.lock();
        match &inner.startup_state {
            DaemonStartupState::Closing => DaemonSocketResponse::Unavailable {
                reason: ipc::UnavailableReason::ServerClosing,
                message: "daemon socket server is closing".to_string(),
            },
            DaemonStartupState::StartupFailed(message) => DaemonSocketResponse::Unavailable {
                reason: ipc::UnavailableReason::StartupFailed,
                message: message.clone(),
            },
            DaemonStartupState::Ready => {
                if let Some(frame) = inner.snapshots.latest_frame() {
                    DaemonSocketResponse::Snapshot(frame)
                } else {
                    DaemonSocketResponse::Unavailable {
                        reason: ipc::UnavailableReason::StartupFailed,
                        message: "daemon reported ready without a snapshot".to_string(),
                    }
                }
            }
            DaemonStartupState::Initializing => DaemonSocketResponse::Unavailable {
                reason: ipc::UnavailableReason::DaemonNotReady,
                message: "daemon has not published its initial snapshot yet".to_string(),
            },
        }
    }

    fn subscribe_response(&self) -> SubscribeResponse {
        let mut inner = self.lock();
        match &inner.startup_state {
            DaemonStartupState::Closing => SubscribeResponse::Unavailable {
                reason: ipc::UnavailableReason::ServerClosing,
                message: "daemon socket server is closing".to_string(),
            },
            DaemonStartupState::StartupFailed(message) => SubscribeResponse::Unavailable {
                reason: ipc::UnavailableReason::StartupFailed,
                message: message.clone(),
            },
            DaemonStartupState::Initializing => SubscribeResponse::Unavailable {
                reason: ipc::UnavailableReason::DaemonNotReady,
                message: "daemon has not published its initial snapshot yet".to_string(),
            },
            DaemonStartupState::Ready => {
                let Some(bootstrap_frame) = inner.snapshots.latest_frame() else {
                    return SubscribeResponse::Unavailable {
                        reason: ipc::UnavailableReason::StartupFailed,
                        message: "daemon reported ready without a snapshot".to_string(),
                    };
                };
                if inner.subscribers.len() >= MAX_SUBSCRIBERS {
                    return SubscribeResponse::Unavailable {
                        reason: ipc::UnavailableReason::SubscriberLimitReached,
                        message: format!(
                            "daemon subscriber limit reached ({MAX_SUBSCRIBERS} subscribers)"
                        ),
                    };
                }

                let id = inner.next_subscriber_id;
                inner.next_subscriber_id = inner.next_subscriber_id.saturating_add(1);
                let mailbox = SubscriberMailbox::new();
                inner.subscribers.insert(id, mailbox.clone());
                SubscribeResponse::Registered(SubscriberRegistration {
                    id,
                    bootstrap_frame,
                    mailbox,
                })
            }
        }
    }

    fn lifecycle_status(&self) -> ipc::LifecycleStatusFrame {
        let inner = self.lock();
        let identity = inner
            .identity
            .as_ref()
            .cloned()
            .unwrap_or_else(DaemonRuntimeIdentity::unknown_for_tests);
        let (state, unavailable_reason, message) = match &inner.startup_state {
            DaemonStartupState::Initializing => (
                ipc::LifecycleDaemonState::Initializing,
                Some(ipc::UnavailableReason::DaemonNotReady),
                Some("daemon has not published its initial snapshot yet".to_string()),
            ),
            DaemonStartupState::Ready => (ipc::LifecycleDaemonState::Ready, None, None),
            DaemonStartupState::StartupFailed(message) => (
                ipc::LifecycleDaemonState::StartupFailed,
                Some(ipc::UnavailableReason::StartupFailed),
                Some(message.clone()),
            ),
            DaemonStartupState::Closing => (
                ipc::LifecycleDaemonState::Closing,
                Some(ipc::UnavailableReason::ServerClosing),
                Some("daemon socket server is closing".to_string()),
            ),
        };
        ipc::LifecycleStatusFrame {
            state,
            identity: identity.frame(),
            subscriber_count: inner.subscribers.len(),
            latest_snapshot_generated_at: inner.snapshots.latest_generated_at(),
            latest_snapshot_pane_count: inner.snapshots.latest_pane_count(),
            latest_snapshot_update_source: inner
                .snapshots
                .latest_update()
                .map(|update| update.source.to_string()),
            latest_snapshot_update_detail: inner
                .snapshots
                .latest_update()
                .and_then(|update| update.detail.clone()),
            latest_snapshot_update_duration_ms: inner
                .snapshots
                .latest_update()
                .and_then(|update| update.duration_ms),
            control_mode_broker: inner.control_mode_broker.clone(),
            runtime_telemetry: inner.runtime_telemetry.clone(),
            latest_snapshot_observability: inner.snapshots.latest_observability(),
            recent_events: inner.recent_events.iter().cloned().collect(),
            unavailable_reason,
            message,
        }
    }

    pub(super) fn update_control_mode_broker_status(
        &self,
        status: ipc::ControlModeBrokerStatusFrame,
    ) {
        let mut inner = self.lock();
        inner.control_mode_broker = Some(status);
    }

    pub(super) fn update_runtime_telemetry(&self, telemetry: ipc::RuntimeTelemetryFrame) {
        let mut inner = self.lock();
        inner.runtime_telemetry = Some(telemetry);
    }

    pub(super) fn set_client_event_sender(&self, sender: mpsc::Sender<Result<ControlModeLine>>) {
        let mut inner = self.lock();
        inner.client_event_tx = Some(sender);
    }

    pub(super) fn record_observability_event(&self, event: ipc::DaemonObservabilityEventFrame) {
        let mut inner = self.lock();
        if inner.recent_events.len() >= OBSERVABILITY_EVENT_RING_CAPACITY {
            inner.recent_events.pop_front();
        }
        inner.recent_events.push_back(event);
    }

    fn client_event_response(&self, event: ipc::ClientEventFrame) -> ClientEventResponse {
        let sender = {
            let inner = self.lock();
            match &inner.startup_state {
                DaemonStartupState::Closing => {
                    return ClientEventResponse::Unavailable {
                        reason: ipc::UnavailableReason::ServerClosing,
                        message: "daemon socket server is closing".to_string(),
                    };
                }
                DaemonStartupState::StartupFailed(message) => {
                    return ClientEventResponse::Unavailable {
                        reason: ipc::UnavailableReason::StartupFailed,
                        message: message.clone(),
                    };
                }
                DaemonStartupState::Initializing => {
                    return ClientEventResponse::Unavailable {
                        reason: ipc::UnavailableReason::DaemonNotReady,
                        message: "daemon has not published its initial snapshot yet".to_string(),
                    };
                }
                DaemonStartupState::Ready => {}
            }
            inner.client_event_tx.clone()
        };

        let Some(sender) = sender else {
            return ClientEventResponse::Unavailable {
                reason: ipc::UnavailableReason::DaemonNotReady,
                message: "daemon client event queue is not initialized".to_string(),
            };
        };

        if sender
            .send(Ok(ControlModeLine::client_event(event)))
            .is_err()
        {
            return ClientEventResponse::Unavailable {
                reason: ipc::UnavailableReason::ServerClosing,
                message: "daemon client event queue is closed".to_string(),
            };
        }

        ClientEventResponse::Accepted
    }

    pub(crate) fn try_acquire_pending_handshake(&self) -> Option<PendingHandshake> {
        let mut inner = self.lock();
        if inner.pending_handshakes >= MAX_PENDING_HANDSHAKES {
            return None;
        }
        inner.pending_handshakes += 1;
        Some(PendingHandshake {
            state: self.clone(),
            active: true,
        })
    }

    fn release_pending_handshake(&self) {
        let mut inner = self.lock();
        inner.pending_handshakes = inner.pending_handshakes.saturating_sub(1);
    }

    fn retire_subscriber(&self, id: SubscriberId) -> bool {
        let subscriber = {
            let mut inner = self.lock();
            inner.subscribers.remove(&id)
        };
        if let Some(mailbox) = subscriber {
            mailbox.close();
            true
        } else {
            false
        }
    }

    fn has_subscriber(&self, id: SubscriberId) -> bool {
        self.lock().subscribers.contains_key(&id)
    }

    #[cfg(test)]
    pub(crate) fn subscriber_count(&self) -> usize {
        self.lock().subscribers.len()
    }

    #[cfg(test)]
    pub(crate) fn pending_handshake_count(&self) -> usize {
        self.lock().pending_handshakes
    }

    #[cfg(test)]
    pub(crate) fn test_register_subscriber_for_capacity(&self) -> Option<SubscriberId> {
        match self.subscribe_response() {
            SubscribeResponse::Registered(registration) => Some(registration.id),
            SubscribeResponse::Unavailable { .. } => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn test_retire_subscriber(&self, id: SubscriberId) -> bool {
        self.retire_subscriber(id)
    }

    #[cfg(test)]
    pub(crate) fn test_record_observability_event(
        &self,
        event: ipc::DaemonObservabilityEventFrame,
    ) {
        self.record_observability_event(event);
    }

    #[cfg(test)]
    pub(crate) fn test_recent_event_count(&self) -> usize {
        self.lock().recent_events.len()
    }

    /// Poison the state mutex by panicking while holding it, mimicking a
    /// handler thread that panics mid-critical-section. Used to prove that
    /// subsequent `lock()` calls still recover instead of propagating.
    #[cfg(test)]
    pub(crate) fn test_poison_lock(&self) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = self
                .inner
                .lock()
                .expect("acquire daemon socket state lock to poison it");
            panic!("intentionally poisoning daemon socket state lock");
        }));
    }

    #[cfg(test)]
    pub(crate) fn test_install_client_event_sender(
        &self,
    ) -> mpsc::Receiver<Result<ControlModeLine>> {
        let (sender, receiver) = mpsc::channel();
        self.set_client_event_sender(sender);
        receiver
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, DaemonSocketStateInner> {
        // Recover from a poisoned lock instead of propagating the panic: a
        // single handler thread panicking while holding this mutex must not
        // permanently deafen the long-running daemon.
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
pub(crate) fn test_recv_client_event(
    receiver: &mpsc::Receiver<Result<ControlModeLine>>,
) -> Option<ipc::ClientEventFrame> {
    receiver
        .recv_timeout(Duration::from_secs(2))
        .ok()
        .and_then(Result::ok)
        .and_then(|line| line.emitted_client_event())
}

enum DaemonSocketResponse {
    Snapshot(EncodedDaemonFrame),
    Unavailable {
        reason: ipc::UnavailableReason,
        message: String,
    },
}

enum ClientEventResponse {
    Accepted,
    Unavailable {
        reason: ipc::UnavailableReason,
        message: String,
    },
}

enum SubscribeResponse {
    Registered(SubscriberRegistration),
    Unavailable {
        reason: ipc::UnavailableReason,
        message: String,
    },
}

struct SubscriberRegistration {
    id: SubscriberId,
    bootstrap_frame: EncodedDaemonFrame,
    mailbox: SubscriberMailbox,
}

pub(crate) struct PendingHandshake {
    state: DaemonSocketState,
    active: bool,
}

impl PendingHandshake {
    fn release(mut self) {
        if self.active {
            self.active = false;
            self.state.release_pending_handshake();
        }
    }
}

impl Drop for PendingHandshake {
    fn drop(&mut self) {
        if self.active {
            self.active = false;
            self.state.release_pending_handshake();
        }
    }
}

fn subscriber_mailboxes(inner: &DaemonSocketStateInner) -> Vec<SubscriberMailbox> {
    inner.subscribers.values().cloned().collect()
}

fn fan_out_snapshot(frame: EncodedDaemonFrame, subscribers: Vec<SubscriberMailbox>) {
    for subscriber in subscribers {
        subscriber.enqueue(frame.clone());
    }
}

fn close_subscribers(subscribers: HashMap<SubscriberId, SubscriberMailbox>) {
    let closing_frame = ipc::encode_frame(&ipc::DaemonFrame::Unavailable {
        reason: ipc::UnavailableReason::ServerClosing,
        message: "daemon socket server is closing".to_string(),
    })
    .map(Arc::<[u8]>::from)
    .ok();
    for subscriber in subscribers.into_values() {
        if let Some(frame) = &closing_frame {
            subscriber.close_with_frame(frame.clone());
        } else {
            subscriber.close();
        }
    }
}

/// Bench seam: runs the publish/fan-out encode path and returns the encoded
/// frame length so benchmarks can exercise it without touching subscribers.
pub(crate) fn bench_encode_snapshot_frame_len(snapshot: &SnapshotEnvelope) -> Result<usize> {
    encode_snapshot_frame(snapshot).map(|frame| frame.len())
}

fn encode_snapshot_frame(snapshot: &SnapshotEnvelope) -> Result<EncodedDaemonFrame> {
    let frame = ipc::DaemonFrame::Snapshot {
        snapshot: snapshot.clone(),
    };
    let encoded = ipc::encode_frame(&frame)?;
    if encoded.len() > ipc::DAEMON_FRAME_MAX_BYTES {
        bail!(
            "encoded snapshot frame was {} bytes, exceeding daemon frame limit of {} bytes",
            encoded.len(),
            ipc::DAEMON_FRAME_MAX_BYTES
        );
    }
    Ok(Arc::<[u8]>::from(encoded))
}

#[derive(Clone)]
pub(crate) struct SubscriberMailbox {
    inner: Arc<(Mutex<SubscriberMailboxState>, Condvar)>,
}

struct SubscriberMailboxState {
    pending_frame: Option<EncodedDaemonFrame>,
    closed: bool,
}

/// Recover a lock guard from a poisoned mutex/condvar result instead of
/// panicking. A handler thread panicking while holding a subscriber mailbox
/// lock must not permanently wedge fan-out for the long-running daemon.
fn recover_lock<T>(result: std::sync::LockResult<T>) -> T {
    result.unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl SubscriberMailbox {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new((
                Mutex::new(SubscriberMailboxState {
                    pending_frame: None,
                    closed: false,
                }),
                Condvar::new(),
            )),
        }
    }

    pub(crate) fn enqueue(&self, frame: EncodedDaemonFrame) {
        let (lock, condvar) = &*self.inner;
        let mut state = recover_lock(lock.lock());
        if state.closed {
            return;
        }
        state.pending_frame = Some(frame);
        condvar.notify_one();
    }

    pub(crate) fn close(&self) {
        let (lock, condvar) = &*self.inner;
        let mut state = recover_lock(lock.lock());
        state.closed = true;
        state.pending_frame = None;
        condvar.notify_all();
    }

    pub(crate) fn close_with_frame(&self, frame: EncodedDaemonFrame) {
        let (lock, condvar) = &*self.inner;
        let mut state = recover_lock(lock.lock());
        state.pending_frame = Some(frame);
        state.closed = true;
        condvar.notify_all();
    }

    pub(crate) fn recv(&self) -> Option<EncodedDaemonFrame> {
        let (lock, condvar) = &*self.inner;
        let mut state = recover_lock(lock.lock());
        loop {
            if let Some(frame) = state.pending_frame.take() {
                return Some(frame);
            }
            if state.closed {
                return None;
            }
            state = recover_lock(condvar.wait(state));
        }
    }

    #[cfg(test)]
    pub(crate) fn try_take_pending(&self) -> Option<EncodedDaemonFrame> {
        let (lock, _) = &*self.inner;
        recover_lock(lock.lock()).pending_frame.take()
    }

    #[cfg(test)]
    pub(crate) fn is_closed(&self) -> bool {
        let (lock, _) = &*self.inner;
        recover_lock(lock.lock()).closed
    }
}

#[cfg(test)]
pub(crate) fn handle_daemon_socket_client(
    stream: std::os::unix::net::UnixStream,
    state: &DaemonSocketState,
) -> Result<()> {
    if let Some(pending_handshake) = state.try_acquire_pending_handshake() {
        handle_daemon_socket_client_with_pending(stream, state, pending_handshake)
    } else {
        refuse_server_busy(stream);
        Ok(())
    }
}

fn handle_daemon_socket_client_with_pending(
    mut stream: std::os::unix::net::UnixStream,
    state: &DaemonSocketState,
    pending_handshake: PendingHandshake,
) -> Result<()> {
    stream
        .set_nonblocking(false)
        .context("failed to configure daemon socket client as blocking")?;
    stream
        .set_write_timeout(Some(CLIENT_WRITE_TIMEOUT))
        .context("failed to set daemon socket write timeout")?;
    let mut writer = stream
        .try_clone()
        .context("failed to clone daemon socket stream")?;
    writer
        .set_nonblocking(false)
        .context("failed to configure daemon socket writer as blocking")?;
    writer
        .set_write_timeout(Some(CLIENT_WRITE_TIMEOUT))
        .context("failed to set daemon socket writer timeout")?;
    let Some(frame) = read_client_frame_with_deadline(&mut stream)? else {
        return Ok(());
    };

    let ack = match ipc::validate_client_hello(&frame) {
        ack @ ipc::DaemonFrame::HelloAck { .. } => ack,
        shutdown => {
            pending_handshake.release();
            write_daemon_frame(&mut writer, &shutdown)?;
            return Ok(());
        }
    };
    pending_handshake.release();

    match frame {
        ipc::ClientFrame::Hello {
            mode: ipc::ClientMode::Snapshot,
            ..
        } => {
            write_daemon_frame(&mut writer, &ack)?;
            match state.snapshot_response() {
                DaemonSocketResponse::Snapshot(bytes) => {
                    write_all_with_deadline(&mut writer, &bytes, CLIENT_WRITE_TIMEOUT)
                        .context("failed to write daemon snapshot frame")?
                }
                DaemonSocketResponse::Unavailable { reason, message } => write_daemon_frame(
                    &mut writer,
                    &ipc::DaemonFrame::Unavailable { reason, message },
                )?,
            }
            writer
                .flush()
                .context("failed to flush daemon socket frame")
        }
        ipc::ClientFrame::Hello {
            mode: ipc::ClientMode::Subscribe,
            ..
        } => match state.subscribe_response() {
            SubscribeResponse::Registered(registration) => {
                write_daemon_frame(&mut writer, &ack)
                    .and_then(|()| {
                        write_all_with_deadline(
                            &mut writer,
                            &registration.bootstrap_frame,
                            CLIENT_WRITE_TIMEOUT,
                        )
                        .context("failed to write daemon subscriber bootstrap snapshot")
                    })
                    .and_then(|()| {
                        writer
                            .flush()
                            .context("failed to flush daemon socket frame")
                    })
                    .inspect_err(|_| {
                        state.retire_subscriber(registration.id);
                    })?;
                serve_subscriber(stream, writer, state.clone(), registration);
                Ok(())
            }
            SubscribeResponse::Unavailable { reason, message } => {
                write_daemon_frame(&mut writer, &ack)?;
                write_daemon_frame(
                    &mut writer,
                    &ipc::DaemonFrame::Unavailable { reason, message },
                )?;
                writer
                    .flush()
                    .context("failed to flush daemon socket frame")
            }
        },
        ipc::ClientFrame::Hello {
            mode: ipc::ClientMode::LifecycleStatus,
            ..
        } => {
            write_daemon_frame(&mut writer, &ack)?;
            write_daemon_frame(
                &mut writer,
                &ipc::DaemonFrame::LifecycleStatus {
                    status: Box::new(state.lifecycle_status()),
                },
            )?;
            writer
                .flush()
                .context("failed to flush daemon socket frame")
        }
        ipc::ClientFrame::ClientEvent { event, .. } => {
            write_daemon_frame(&mut writer, &ack)?;
            match state.client_event_response(event) {
                ClientEventResponse::Accepted => {}
                ClientEventResponse::Unavailable { reason, message } => {
                    write_daemon_frame(
                        &mut writer,
                        &ipc::DaemonFrame::Unavailable { reason, message },
                    )?;
                }
            }
            writer
                .flush()
                .context("failed to flush daemon socket frame")
        }
    }
}

pub(crate) fn refuse_server_busy(mut stream: std::os::unix::net::UnixStream) {
    let _ = stream.set_write_timeout(Some(SUBSCRIBER_WRITE_TIMEOUT));
    let _ = write_daemon_frame(
        &mut stream,
        &ipc::DaemonFrame::Shutdown {
            reason: ipc::ShutdownReason::ServerBusy,
            message: format!("daemon is busy handling {MAX_PENDING_HANDSHAKES} pending clients"),
        },
    );
    let _ = stream.flush();
}

fn serve_subscriber(
    mut stream: std::os::unix::net::UnixStream,
    mut writer: std::os::unix::net::UnixStream,
    state: DaemonSocketState,
    registration: SubscriberRegistration,
) {
    let SubscriberRegistration { id, mailbox, .. } = registration;
    let writer_state = state.clone();
    let writer_mailbox = mailbox.clone();
    std::thread::spawn(move || {
        writer
            .set_write_timeout(Some(SUBSCRIBER_WRITE_TIMEOUT))
            .ok();
        while let Some(frame) = writer_mailbox.recv() {
            let result = write_all_with_deadline(&mut writer, &frame, SUBSCRIBER_WRITE_TIMEOUT)
                .and_then(|()| writer.flush().context("failed to flush subscriber frame"))
                .with_context(|| format!("failed to write subscriber frame for {id}"));
            if result.is_err() {
                writer_state.retire_subscriber(id);
                break;
            }
        }
    });

    stream
        .set_read_timeout(Some(SUBSCRIBER_MONITOR_POLL_INTERVAL))
        .ok();
    let mut byte = [0; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => {
                state.retire_subscriber(id);
                break;
            }
            Ok(_) => {
                state.retire_subscriber(id);
                break;
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                let should_back_off = error.kind() == std::io::ErrorKind::WouldBlock;
                if !state.has_subscriber(id) {
                    break;
                }
                if should_back_off {
                    std::thread::sleep(SUBSCRIBER_MONITOR_POLL_INTERVAL);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => {
                state.retire_subscriber(id);
                break;
            }
        }
    }
}

fn read_client_frame_with_deadline(
    stream: &mut std::os::unix::net::UnixStream,
) -> Result<Option<ipc::ClientFrame>> {
    let deadline = Instant::now() + CLIENT_HANDSHAKE_TIMEOUT;
    let mut output = Vec::new();
    let mut byte = [0; 1];

    loop {
        let now = Instant::now();
        if now >= deadline {
            bail!("timed out waiting for daemon client hello");
        }
        stream
            .set_read_timeout(Some(deadline.saturating_duration_since(now)))
            .context("failed to set daemon socket read timeout")?;

        match stream.read(&mut byte) {
            Ok(0) if output.is_empty() => return Ok(None),
            Ok(0) => bail!("IPC frame ended before newline"),
            Ok(_) => {
                if output.len() >= ipc::CLIENT_HELLO_MAX_BYTES {
                    bail!(
                        "IPC frame exceeds {} byte limit",
                        ipc::CLIENT_HELLO_MAX_BYTES
                    );
                }
                if byte[0] == b'\n' {
                    if output.ends_with(b"\r") {
                        output.pop();
                    }
                    return ipc::decode_client_frame(&output).map(Some);
                }
                output.push(byte[0]);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                bail!("timed out waiting for daemon client hello");
            }
            Err(error) => return Err(error).context("failed to read daemon client hello"),
        }
    }
}

fn write_daemon_frame(writer: &mut impl Write, frame: &ipc::DaemonFrame) -> Result<()> {
    let encoded = ipc::encode_frame(frame)?;
    if encoded.len() > ipc::DAEMON_FRAME_MAX_BYTES {
        bail!(
            "daemon frame was {} bytes, exceeding daemon frame limit of {} bytes",
            encoded.len(),
            ipc::DAEMON_FRAME_MAX_BYTES
        );
    }
    writer
        .write_all(&encoded)
        .context("failed to write daemon socket frame")
}

fn write_all_with_deadline(
    writer: &mut impl Write,
    mut bytes: &[u8],
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while !bytes.is_empty() {
        match writer.write(bytes) {
            Ok(0) => bail!("daemon socket write returned zero bytes"),
            Ok(written) => bytes = &bytes[written..],
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(error).context("failed to write daemon socket frame"),
        }
    }
    Ok(())
}
