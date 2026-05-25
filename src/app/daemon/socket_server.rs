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
                    Err(error) => {
                        eprintln!("agentscan: daemon socket accept failed: {error}");
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

    pub(super) fn record_observability_event(&self, event: ipc::DaemonObservabilityEventFrame) {
        let mut inner = self.lock();
        if inner.recent_events.len() >= OBSERVABILITY_EVENT_RING_CAPACITY {
            inner.recent_events.pop_front();
        }
        inner.recent_events.push_back(event);
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

    fn lock(&self) -> std::sync::MutexGuard<'_, DaemonSocketStateInner> {
        self.inner
            .lock()
            .expect("daemon socket state lock poisoned")
    }
}

enum DaemonSocketResponse {
    Snapshot(EncodedDaemonFrame),
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
        let mut state = lock.lock().expect("subscriber mailbox lock poisoned");
        if state.closed {
            return;
        }
        state.pending_frame = Some(frame);
        condvar.notify_one();
    }

    pub(crate) fn close(&self) {
        let (lock, condvar) = &*self.inner;
        let mut state = lock.lock().expect("subscriber mailbox lock poisoned");
        state.closed = true;
        state.pending_frame = None;
        condvar.notify_all();
    }

    pub(crate) fn close_with_frame(&self, frame: EncodedDaemonFrame) {
        let (lock, condvar) = &*self.inner;
        let mut state = lock.lock().expect("subscriber mailbox lock poisoned");
        state.pending_frame = Some(frame);
        state.closed = true;
        condvar.notify_all();
    }

    pub(crate) fn recv(&self) -> Option<EncodedDaemonFrame> {
        let (lock, condvar) = &*self.inner;
        let mut state = lock.lock().expect("subscriber mailbox lock poisoned");
        loop {
            if let Some(frame) = state.pending_frame.take() {
                return Some(frame);
            }
            if state.closed {
                return None;
            }
            state = condvar
                .wait(state)
                .expect("subscriber mailbox lock poisoned while waiting");
        }
    }

    #[cfg(test)]
    pub(crate) fn try_take_pending(&self) -> Option<EncodedDaemonFrame> {
        let (lock, _) = &*self.inner;
        lock.lock()
            .expect("subscriber mailbox lock poisoned")
            .pending_frame
            .take()
    }

    #[cfg(test)]
    pub(crate) fn is_closed(&self) -> bool {
        let (lock, _) = &*self.inner;
        lock.lock()
            .expect("subscriber mailbox lock poisoned")
            .closed
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
