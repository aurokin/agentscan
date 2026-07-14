use super::*;
use std::collections::VecDeque;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;

const OBSERVABILITY_EVENT_RING_CAPACITY: usize = 256;

/// Seq stamped on the bootstrap (initial) full snapshot. Post-bootstrap publishes
/// advance from here, so the first `snapshot_diff` a subscriber sees is `seq + 1`.
const INITIAL_SNAPSHOT_SEQ: u64 = 1;

pub(super) struct DaemonSocketServer {
    listener: std::os::unix::net::UnixListener,
    socket_path: PathBuf,
    socket_identity: Option<SocketFileIdentity>,
    state: DaemonSocketState,
    stop: Arc<AtomicBool>,
    // Read end of a self-pipe the acceptor waits on alongside the listener; a byte
    // written to `wake_tx` unblocks `poll` so shutdown is instant and path-independent
    // (it works even if the socket file was already unlinked or replaced).
    wake_rx: UnixStream,
    wake_tx: Option<UnixStream>,
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
        // Nonblocking so that after `poll` reports readiness the `accept` never blocks on a
        // spurious wakeup (e.g. a peer that aborted between poll and accept).
        listener
            .set_nonblocking(true)
            .context("failed to configure daemon socket listener")?;
        let (wake_tx, wake_rx) =
            UnixStream::pair().context("failed to create daemon socket acceptor wake pipe")?;
        Ok(Self {
            listener,
            socket_path: socket_path.to_path_buf(),
            socket_identity: SocketFileIdentity::from_path(socket_path)?,
            state: DaemonSocketState::new(),
            stop: Arc::new(AtomicBool::new(false)),
            wake_rx,
            wake_tx: Some(wake_tx),
        })
    }

    pub(super) fn state(&self) -> DaemonSocketState {
        self.state.clone()
    }

    pub(super) fn spawn(mut self) -> DaemonSocketServerHandle {
        let stop = self.stop.clone();
        let handle_stop = self.stop.clone();
        let wake_tx = self.wake_tx.take();
        let listener = self.listener;
        let state = self.state;
        let wake_rx = self.wake_rx;
        let join = std::thread::spawn(move || {
            let listener_fd = listener.as_raw_fd();
            let wake_fd = wake_rx.as_raw_fd();
            while !stop.load(Ordering::SeqCst) {
                // Block until the listener is readable or a wake byte arrives. `poll(-1)`
                // sleeps indefinitely, so a fully idle daemon issues zero accept-thread
                // wakeups (the old 10ms WouldBlock poll spun ~100x/sec forever).
                let mut fds = [
                    libc::pollfd {
                        fd: listener_fd,
                        events: libc::POLLIN,
                        revents: 0,
                    },
                    libc::pollfd {
                        fd: wake_fd,
                        events: libc::POLLIN,
                        revents: 0,
                    },
                ];
                let poll_result = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
                if poll_result < 0 {
                    let error = std::io::Error::last_os_error();
                    if error.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    eprintln!(
                        "agentscan: daemon socket acceptor poll failed fatally, requesting shutdown: {error}"
                    );
                    DAEMON_SHUTDOWN_REQUESTED.store(true, Ordering::Relaxed);
                    break;
                }
                // A wake byte means shutdown was requested; stop before touching the
                // listener. The byte is never drained — the thread is exiting.
                if fds[1].revents != 0 {
                    break;
                }
                match listener.accept() {
                    Ok((stream, _)) => {
                        // A client that connected in the same instant shutdown was
                        // requested: drop it and stop rather than half-serve it.
                        if stop.load(Ordering::SeqCst) {
                            drop(stream);
                            break;
                        }
                        let state = state.clone();
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
                            // Offload the busy refusal to a short-lived thread like the
                            // success path: writing it inline would let one non-reading
                            // client stall the whole acceptor for the write timeout.
                            std::thread::spawn(move || refuse_server_busy(stream));
                        }
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                        ) =>
                    {
                        // Spurious readiness (peer aborted between poll and accept) or an
                        // interrupted syscall: just re-poll, no sleep/spin.
                        continue;
                    }
                    Err(error) if is_transient_accept_error(&error) => {
                        // Resource pressure — fd exhaustion (EMFILE/ENFILE), out of
                        // buffers, or a peer that hung up before accept completed — is
                        // recoverable: the listener works again once the condition
                        // clears. Back off briefly so we don't spin the CPU while the
                        // pressure clears (poll would keep reporting the fd readable),
                        // then keep accepting. Breaking here would silently turn the
                        // daemon deaf while it still holds the socket and flock, so every
                        // later client hangs.
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
            wake_tx,
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
    // Write end of the acceptor wake pipe; writing a byte unblocks its `poll`.
    wake_tx: Option<UnixStream>,
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
        self.stop.store(true, Ordering::SeqCst);
        // Wake the acceptor's `poll` so it observes the stop flag immediately. Using the
        // self-pipe (rather than a connect to the socket path) makes this robust even when
        // the socket file was already unlinked or replaced — the case a path-based wake
        // would miss, leaving `join` to hang forever.
        if let Some(wake_tx) = self.wake_tx.take() {
            use std::io::Write;
            let _ = (&wake_tx).write(&[1u8]);
        }
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
    client_event_tx: Option<mpsc::SyncSender<Result<ControlModeLine>>>,
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
            duration_ms: self
                .started_at
                .map(|started_at| super::duration_millis_u64(started_at.elapsed())),
        }
    }
}

pub(super) struct PreparedSnapshot {
    pub(super) snapshot: SnapshotEnvelope,
    pub(super) frame: EncodedDaemonFrame,
    pub(super) seq: u64,
}

impl PreparedSnapshot {
    pub(super) fn new(snapshot: SnapshotEnvelope) -> Result<Self> {
        // The prepared snapshot is only ever the bootstrap, so it always encodes
        // at `INITIAL_SNAPSHOT_SEQ`; this also validates the frame fits the wire
        // limit before the daemon commits to serving it.
        let seq = INITIAL_SNAPSHOT_SEQ;
        let frame = encode_snapshot_frame(&snapshot, seq)?;
        Ok(Self {
            snapshot,
            frame,
            seq,
        })
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
            let frame = inner.snapshots.publish_initial(prepared, telemetry);
            inner.startup_state = DaemonStartupState::Ready;
            (frame, subscriber_mailboxes(&inner))
        };
        // The bootstrap full frame is absolute, so a bare full-frame broadcast is
        // safe here: any coalesce with a later diff already upgrades to a full
        // frame in `enqueue`.
        fan_out_broadcast(DaemonBroadcast::from_full(frame), subscribers);
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
        let (broadcast, subscribers) = {
            let mut inner = self.lock();
            match inner.snapshots.publish_diff(snapshot, telemetry) {
                Ok(broadcast) => {
                    inner.startup_state = DaemonStartupState::Ready;
                    (broadcast, subscriber_mailboxes(&inner))
                }
                Err(error) => {
                    eprintln!(
                        "agentscan: skipped daemon socket snapshot update because encoded frame exceeded {} bytes; previous good snapshot remains active: {error:#}",
                        ipc::DAEMON_FRAME_MAX_BYTES
                    );
                    return false;
                }
            }
        };
        fan_out_broadcast(broadcast, subscribers);
        true
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
        let mut inner = self.lock();
        // Resolve the non-Ready states first so the immutable `startup_state`
        // borrow is released before the mutable `latest_frame` lazy-encode below.
        if let Some(response) = match &inner.startup_state {
            DaemonStartupState::Closing => Some(DaemonSocketResponse::Unavailable {
                reason: ipc::UnavailableReason::ServerClosing,
                message: "daemon socket server is closing".to_string(),
            }),
            DaemonStartupState::StartupFailed(message) => Some(DaemonSocketResponse::Unavailable {
                reason: ipc::UnavailableReason::StartupFailed,
                message: message.clone(),
            }),
            DaemonStartupState::Initializing => Some(DaemonSocketResponse::Unavailable {
                reason: ipc::UnavailableReason::DaemonNotReady,
                message: "daemon has not published its initial snapshot yet".to_string(),
            }),
            DaemonStartupState::Ready => None,
        } {
            return response;
        }

        if let Some(frame) = inner.snapshots.latest_frame() {
            DaemonSocketResponse::Snapshot(frame)
        } else {
            DaemonSocketResponse::Unavailable {
                reason: ipc::UnavailableReason::StartupFailed,
                message: "daemon reported ready without a snapshot".to_string(),
            }
        }
    }

    fn subscribe_response(&self) -> SubscribeResponse {
        let mut inner = self.lock();
        // As in `snapshot_response`, settle the non-Ready states before the
        // mutable `latest_frame` lazy-encode.
        if let Some(response) = match &inner.startup_state {
            DaemonStartupState::Closing => Some(SubscribeResponse::Unavailable {
                reason: ipc::UnavailableReason::ServerClosing,
                message: "daemon socket server is closing".to_string(),
            }),
            DaemonStartupState::StartupFailed(message) => Some(SubscribeResponse::Unavailable {
                reason: ipc::UnavailableReason::StartupFailed,
                message: message.clone(),
            }),
            DaemonStartupState::Initializing => Some(SubscribeResponse::Unavailable {
                reason: ipc::UnavailableReason::DaemonNotReady,
                message: "daemon has not published its initial snapshot yet".to_string(),
            }),
            DaemonStartupState::Ready => None,
        } {
            return response;
        }

        {
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

    pub(super) fn set_client_event_sender(
        &self,
        sender: mpsc::SyncSender<Result<ControlModeLine>>,
    ) {
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
        let (sender, receiver) = mpsc::sync_channel(256);
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

fn fan_out_broadcast(broadcast: DaemonBroadcast, subscribers: Vec<SubscriberMailbox>) {
    for subscriber in subscribers {
        subscriber.enqueue(broadcast.clone());
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

/// Bench seam: runs the publish/fan-out full-frame encode path and returns the
/// encoded frame length so benchmarks can exercise it without touching subscribers.
pub(crate) fn bench_encode_snapshot_frame_len(snapshot: &SnapshotEnvelope) -> Result<usize> {
    encode_snapshot_frame(snapshot, INITIAL_SNAPSHOT_SEQ).map(|frame| frame.len())
}

/// Bench seam: encodes a `snapshot_diff` frame for the given delta and returns
/// its byte length, so benchmarks can compare diff vs full-frame encode cost.
pub(crate) fn bench_encode_diff_frame_len(
    previous: &SnapshotEnvelope,
    current: &SnapshotEnvelope,
) -> Result<usize> {
    let (changed_panes, removed_pane_ids) = super::refresh::snapshot_wire_diff(previous, current);
    encode_diff_frame(
        INITIAL_SNAPSHOT_SEQ + 1,
        current,
        changed_panes,
        removed_pane_ids,
    )
    .map(|frame| frame.len())
}

/// Full-frame encode seam for the snapshot store's lazy bootstrap/query path.
pub(super) fn encode_full_frame(
    snapshot: &SnapshotEnvelope,
    seq: u64,
) -> Result<EncodedDaemonFrame> {
    encode_snapshot_frame(snapshot, seq)
}

/// Builds the post-bootstrap fan-out broadcast: a `snapshot_diff` primary frame
/// against `previous`, plus a lazily-encoded absolute full frame for coalesce
/// safety.
pub(super) fn build_diff_broadcast(
    seq: u64,
    previous: &SnapshotEnvelope,
    current: &Arc<SnapshotEnvelope>,
) -> Result<DaemonBroadcast> {
    let (changed_panes, removed_pane_ids) = super::refresh::snapshot_wire_diff(previous, current);
    let primary = encode_diff_frame(seq, current, changed_panes, removed_pane_ids)?;
    Ok(DaemonBroadcast::new(primary, current.clone(), seq))
}

/// Builds a full-frame broadcast (no prior snapshot to diff against). The primary
/// frame is itself absolute, so it doubles as the coalesce fallback.
pub(super) fn build_full_broadcast(
    seq: u64,
    current: &Arc<SnapshotEnvelope>,
) -> Result<DaemonBroadcast> {
    let primary = encode_snapshot_frame(current, seq)?;
    Ok(DaemonBroadcast::from_full(primary))
}

fn encode_snapshot_frame(snapshot: &SnapshotEnvelope, seq: u64) -> Result<EncodedDaemonFrame> {
    let frame = ipc::DaemonFrame::Snapshot {
        seq,
        snapshot: snapshot.clone(),
    };
    encode_bounded_frame(&frame, "snapshot")
}

fn encode_diff_frame(
    seq: u64,
    snapshot: &SnapshotEnvelope,
    changed_panes: Vec<PaneRecord>,
    removed_pane_ids: Vec<String>,
) -> Result<EncodedDaemonFrame> {
    let frame = ipc::DaemonFrame::SnapshotDiff {
        seq,
        schema_version: snapshot.schema_version,
        generated_at: snapshot.generated_at.clone(),
        source: snapshot.source.clone(),
        changed_panes,
        removed_pane_ids,
    };
    encode_bounded_frame(&frame, "snapshot diff")
}

fn encode_bounded_frame(frame: &ipc::DaemonFrame, label: &str) -> Result<EncodedDaemonFrame> {
    let encoded = ipc::encode_frame(frame)?;
    if encoded.len() > ipc::DAEMON_FRAME_MAX_BYTES {
        bail!(
            "encoded {label} frame was {} bytes, exceeding daemon frame limit of {} bytes",
            encoded.len(),
            ipc::DAEMON_FRAME_MAX_BYTES
        );
    }
    Ok(Arc::<[u8]>::from(encoded))
}

/// One fan-out unit: the frame to enqueue when a subscriber's mailbox is empty
/// (a diff, or the bootstrap full frame) plus a lazily-encoded absolute full
/// frame used only when coalescing would otherwise drop an undelivered frame.
#[derive(Clone)]
pub(crate) struct DaemonBroadcast {
    primary: EncodedDaemonFrame,
    full: SharedFullFrame,
}

impl DaemonBroadcast {
    fn new(primary: EncodedDaemonFrame, snapshot: Arc<SnapshotEnvelope>, seq: u64) -> Self {
        Self {
            primary,
            full: SharedFullFrame::new(snapshot, seq),
        }
    }

    /// A broadcast whose primary frame is already a full snapshot. Used for the
    /// bootstrap publish, where the coalesce fallback and the primary coincide.
    fn from_full(frame: EncodedDaemonFrame) -> Self {
        Self {
            primary: frame.clone(),
            full: SharedFullFrame::from_encoded(frame),
        }
    }

    /// Encoded size of the primary frame, used by the snapshot store's running
    /// full-frame size bound.
    pub(super) fn primary_len(&self) -> usize {
        self.primary.len()
    }

    #[cfg(test)]
    pub(crate) fn test_diff(
        primary: EncodedDaemonFrame,
        full_snapshot: SnapshotEnvelope,
        seq: u64,
    ) -> Self {
        Self::new(primary, Arc::new(full_snapshot), seq)
    }
}

/// The absolute full snapshot frame for a broadcast, encoded at most once and
/// shared across every coalescing subscriber. `None` from `encoded` means the
/// full frame exceeds the wire limit and no safe coalesced frame exists.
#[derive(Clone)]
struct SharedFullFrame {
    inner: Arc<SharedFullFrameInner>,
}

struct SharedFullFrameInner {
    source: SharedFullFrameSource,
    cell: std::sync::OnceLock<Option<EncodedDaemonFrame>>,
}

enum SharedFullFrameSource {
    // Not yet encoded; encode lazily from this snapshot at this seq.
    Lazy {
        snapshot: Arc<SnapshotEnvelope>,
        seq: u64,
    },
    // Already a full frame (bootstrap): the cell is pre-seeded in `from_encoded`.
    Ready,
}

impl SharedFullFrame {
    fn new(snapshot: Arc<SnapshotEnvelope>, seq: u64) -> Self {
        Self {
            inner: Arc::new(SharedFullFrameInner {
                source: SharedFullFrameSource::Lazy { snapshot, seq },
                cell: std::sync::OnceLock::new(),
            }),
        }
    }

    fn from_encoded(frame: EncodedDaemonFrame) -> Self {
        let cell = std::sync::OnceLock::new();
        let _ = cell.set(Some(frame));
        Self {
            inner: Arc::new(SharedFullFrameInner {
                source: SharedFullFrameSource::Ready,
                cell,
            }),
        }
    }

    fn encoded(&self) -> Option<EncodedDaemonFrame> {
        self.inner
            .cell
            .get_or_init(|| match &self.inner.source {
                SharedFullFrameSource::Lazy { snapshot, seq } => {
                    encode_snapshot_frame(snapshot, *seq).ok()
                }
                // Unreachable in practice: `Ready` pre-seeds the cell, so
                // `get_or_init` never runs this arm.
                SharedFullFrameSource::Ready => None,
            })
            .clone()
    }
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

    pub(crate) fn enqueue(&self, broadcast: DaemonBroadcast) {
        let (lock, condvar) = &*self.inner;
        let mut state = recover_lock(lock.lock());
        if state.closed {
            return;
        }
        if state.pending_frame.is_some() {
            // A previous frame is still undelivered, so this enqueue coalesces
            // over it. With diff frames that would silently and permanently
            // diverge the slow subscriber (it never sees the dropped diff), so
            // replace the whole pending slot with the absolute full snapshot
            // instead — the subscriber re-syncs no matter which diffs it missed.
            // The full frame is encoded at most once and shared across all
            // coalescing subscribers.
            match broadcast.full.encoded() {
                Some(full) => state.pending_frame = Some(full),
                None => {
                    // The full frame exceeds the wire limit, so no safe coalesced
                    // frame exists. Close the mailbox so the subscriber reconnects
                    // and re-bootstraps rather than diverging.
                    state.pending_frame = None;
                    state.closed = true;
                }
            }
        } else {
            state.pending_frame = Some(broadcast.primary);
        }
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
        // The mailbox is now closed (client disconnect, write error, or daemon shutdown
        // via `mark_closing`). Shut the shared connection down so the reader loop below,
        // blocked in `read`, wakes with EOF and terminates promptly instead of waiting for
        // the client to close its end. Any final frame was already flushed above.
        let _ = writer.shutdown(std::net::Shutdown::Both);
    });

    // Clear any read timeout left over from the handshake so the read below truly blocks
    // indefinitely rather than returning a spurious `TimedOut` (which the arms below would
    // misread as a disconnect).
    stream.set_read_timeout(None).ok();
    // Rely on a blocking `read` returning 0/EOF on disconnect rather than a timed poll: an
    // idle subscriber parks here at zero cost. The read wakes on client disconnect (its FIN)
    // or on the writer thread's `shutdown(Both)` above, which fires when the mailbox is
    // closed — covering both the client-disconnect and daemon-shutdown teardown paths.
    let mut byte = [0; 1];
    loop {
        match stream.read(&mut byte) {
            // EOF (peer closed or our own writer shut the socket down) or an unexpected
            // inbound byte (subscribers never write): either way the subscriber is done.
            Ok(_) => {
                state.retire_subscriber(id);
                break;
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
