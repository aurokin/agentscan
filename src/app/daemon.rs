use super::*;
use std::collections::HashMap;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use std::time::Instant;

const RECONCILE_INTERVAL: Duration = Duration::from_secs(1);
const STARTUP_FAILURE_OBSERVABILITY_WINDOW: Duration = Duration::from_millis(200);
const CONTROL_MODE_STARTUP_TIMEOUT: Duration = Duration::from_secs(2);
const CLIENT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);
const CLIENT_WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const SUBSCRIBER_WRITE_TIMEOUT: Duration = Duration::from_millis(250);
const SUBSCRIBER_MONITOR_POLL_INTERVAL: Duration = Duration::from_millis(250);
pub(crate) const MAX_PENDING_HANDSHAKES: usize = 8;
pub(crate) const MAX_SUBSCRIBERS: usize = 64;

type SubscriberId = u64;
pub(crate) type EncodedDaemonFrame = Arc<[u8]>;

pub(super) fn daemon_run() -> Result<()> {
    let socket_path = ipc::resolve_socket_path()?;
    daemon_run_with_socket_path_and_startup(&socket_path, DaemonStartup)
}

#[cfg(test)]
pub(crate) fn test_daemon_run_with_startup(
    socket_path: &Path,
    startup: impl StartupActions,
) -> Result<()> {
    daemon_run_with_socket_path_and_startup(socket_path, startup)
}

fn daemon_run_with_socket_path_and_startup(
    socket_path: &Path,
    startup: impl StartupActions,
) -> Result<()> {
    let server = DaemonSocketServer::bind(socket_path)?;
    let socket_state = server.state();
    let server_handle = server.spawn();

    let pending_snapshot = match startup.initial_snapshot().and_then(PreparedSnapshot::new) {
        Ok(pending_snapshot) => pending_snapshot,
        Err(error) => {
            let message = startup_failure_message("initial snapshot", &error);
            socket_state.mark_startup_failed(message.clone());
            std::thread::sleep(STARTUP_FAILURE_OBSERVABILITY_WINDOW);
            drop(server_handle);
            return Err(error.context(message));
        }
    };

    let mut tmux_client = match startup.start_tmux_control_mode_client() {
        Ok(client) => client,
        Err(error) => {
            let message = startup_failure_message("tmux control-mode startup", &error);
            socket_state.mark_startup_failed(message.clone());
            std::thread::sleep(STARTUP_FAILURE_OBSERVABILITY_WINDOW);
            drop(server_handle);
            return Err(error.context(message));
        }
    };

    if let Err(error) = startup.publish_initial_cache_snapshot(&pending_snapshot.snapshot) {
        let message = startup_failure_message("initial snapshot publication", &error);
        socket_state.mark_startup_failed(message.clone());
        tmux_client.cleanup();
        std::thread::sleep(STARTUP_FAILURE_OBSERVABILITY_WINDOW);
        drop(server_handle);
        return Err(error.context(message));
    }
    let mut snapshot = pending_snapshot.snapshot.clone();
    socket_state.publish_prepared_snapshot(pending_snapshot);
    let mut closing_guard = DaemonClosingGuard::new(socket_state.clone());
    let stdout_reader = tmux_client
        .stdout_reader
        .take()
        .context("tmux control-mode client did not provide stdout")?;
    let mut child = tmux_client
        .child
        .take()
        .context("tmux control-mode client did not provide child process")?;
    let mut running_tmux_client = RunningTmuxControlModeClient {
        child: &mut child,
        _stdin: tmux_client.stdin.take(),
    };

    let (line_tx, line_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = stdout_reader;
        loop {
            match read_control_mode_line(&mut reader) {
                Ok(Some(line)) => {
                    if line_tx.send(Ok(line)).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = line_tx.send(Err(error));
                    break;
                }
            }
        }
    });

    let mut next_reconcile_at = Instant::now() + RECONCILE_INTERVAL;

    loop {
        let now = Instant::now();
        if now >= next_reconcile_at {
            reconcile_full_snapshot(&mut snapshot)?;
            socket_state.publish_later_snapshot(snapshot.clone());
            cache::write_snapshot_to_cache(&snapshot)?;
            next_reconcile_at = Instant::now() + RECONCILE_INTERVAL;
        }

        let timeout = next_reconcile_at.saturating_duration_since(Instant::now());
        match line_rx.recv_timeout(timeout) {
            Ok(line) => {
                let line = line?;
                let event = control_event_from_line(&line);
                let should_exit = event == ControlEvent::Exit;
                if apply_control_event(&mut snapshot, &line, &event)? {
                    socket_state.publish_later_snapshot(snapshot.clone());
                    cache::write_snapshot_to_cache(&snapshot)?;
                }

                if should_exit {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                reconcile_full_snapshot(&mut snapshot)?;
                socket_state.publish_later_snapshot(snapshot.clone());
                cache::write_snapshot_to_cache(&snapshot)?;
                next_reconcile_at = Instant::now() + RECONCILE_INTERVAL;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    closing_guard.mark_closing();

    running_tmux_client.wait_for_exit()?;

    Ok(())
}

pub(crate) trait StartupActions {
    fn initial_snapshot(&self) -> Result<SnapshotEnvelope>;
    fn start_tmux_control_mode_client(&self) -> Result<StartedTmuxControlModeClient>;
    fn publish_initial_cache_snapshot(&self, snapshot: &SnapshotEnvelope) -> Result<()>;
}

#[derive(Default)]
struct DaemonStartup;

impl StartupActions for DaemonStartup {
    fn initial_snapshot(&self) -> Result<SnapshotEnvelope> {
        cache::daemon_snapshot_from_tmux()
    }

    fn start_tmux_control_mode_client(&self) -> Result<StartedTmuxControlModeClient> {
        start_tmux_control_mode_client().map(StartedTmuxControlModeClient::from_real)
    }

    fn publish_initial_cache_snapshot(&self, snapshot: &SnapshotEnvelope) -> Result<()> {
        cache::write_snapshot_to_cache(snapshot)
    }
}

pub(crate) struct StartedTmuxControlModeClient {
    child: Option<std::process::Child>,
    stdout_reader: Option<BufReader<std::process::ChildStdout>>,
    stdin: Option<std::process::ChildStdin>,
}

impl StartedTmuxControlModeClient {
    fn from_real(
        (child, stdout_reader, stdin): (
            std::process::Child,
            BufReader<std::process::ChildStdout>,
            std::process::ChildStdin,
        ),
    ) -> Self {
        Self {
            child: Some(child),
            stdout_reader: Some(stdout_reader),
            stdin: Some(stdin),
        }
    }

    #[cfg(test)]
    pub(crate) fn test_started_without_process() -> Self {
        Self {
            child: None,
            stdout_reader: None,
            stdin: None,
        }
    }

    fn cleanup(&mut self) {
        if let Some(child) = &mut self.child {
            cleanup_startup_child(child);
        }
    }
}

struct RunningTmuxControlModeClient<'a> {
    child: &'a mut std::process::Child,
    _stdin: Option<std::process::ChildStdin>,
}

impl RunningTmuxControlModeClient<'_> {
    fn wait_for_exit(&mut self) -> Result<()> {
        let status = self
            .child
            .wait()
            .context("failed while waiting for tmux control-mode client to exit")?;
        if !status.success() {
            bail!("tmux control-mode client exited with status {status}");
        }
        Ok(())
    }
}

impl Drop for RunningTmuxControlModeClient<'_> {
    fn drop(&mut self) {
        cleanup_startup_child(self.child);
    }
}

struct DaemonClosingGuard {
    state: DaemonSocketState,
    marked: bool,
}

impl DaemonClosingGuard {
    fn new(state: DaemonSocketState) -> Self {
        Self {
            state,
            marked: false,
        }
    }

    fn mark_closing(&mut self) {
        if !self.marked {
            self.state.mark_closing();
            self.marked = true;
        }
    }
}

impl Drop for DaemonClosingGuard {
    fn drop(&mut self) {
        self.mark_closing();
    }
}

fn startup_failure_message(context: &str, error: &anyhow::Error) -> String {
    format!(
        "{context} failed before daemon socket readiness; no usable socket snapshot was published: {error:#}"
    )
}

fn start_tmux_control_mode_client() -> Result<(
    std::process::Child,
    BufReader<std::process::ChildStdout>,
    std::process::ChildStdin,
)> {
    let session_target = tmux::default_session_target()?;
    let mut child = tmux::tmux_command()
        .args(["-C", "attach-session", "-t", &session_target])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("failed to start tmux control-mode client")?;

    match configure_started_tmux_control_mode_client(&mut child) {
        Ok((stdout_reader, stdin)) => Ok((child, stdout_reader, stdin)),
        Err(error) => {
            cleanup_startup_child(&mut child);
            Err(error)
        }
    }
}

fn configure_started_tmux_control_mode_client(
    child: &mut std::process::Child,
) -> Result<(
    BufReader<std::process::ChildStdout>,
    std::process::ChildStdin,
)> {
    let stdout = child
        .stdout
        .take()
        .context("tmux control-mode client did not provide stdout")?;
    let mut stdout_reader = BufReader::new(stdout);

    let mut stdin = child
        .stdin
        .take()
        .context("tmux control-mode client did not provide stdin")?;
    wait_for_control_mode_startup_response(&mut stdout_reader, "tmux control-mode attach")?;
    writeln!(stdin, "refresh-client -B '{DAEMON_SUBSCRIPTION_FORMAT}'")
        .context("failed to subscribe to pane and metadata updates")?;
    stdin
        .flush()
        .context("failed to flush tmux control commands")?;
    wait_for_control_mode_startup_response(&mut stdout_reader, "daemon subscription setup")?;

    Ok((stdout_reader, stdin))
}

fn wait_for_control_mode_startup_response(
    reader: &mut BufReader<std::process::ChildStdout>,
    context: &str,
) -> Result<()> {
    let deadline = Instant::now() + CONTROL_MODE_STARTUP_TIMEOUT;
    loop {
        let line =
            read_control_mode_line_before_deadline(reader, deadline)?.with_context(|| {
                format!("tmux control-mode client exited before confirming {context}")
            })?;
        if control_mode_startup_response_from_line(&line, context)? {
            return Ok(());
        }
    }
}

fn control_mode_startup_response_from_line(line: &str, context: &str) -> Result<bool> {
    if line.starts_with("%error") {
        bail!("tmux rejected {context}: {line}");
    }
    Ok(line.starts_with("%end"))
}

#[cfg(test)]
pub(crate) fn test_wait_for_attach_then_subscription_transcript(lines: &[&str]) -> Result<()> {
    let mut waiting_for_attach = true;
    for line in lines {
        let context = if waiting_for_attach {
            "tmux control-mode attach"
        } else {
            "daemon subscription setup"
        };
        if control_mode_startup_response_from_line(line, context)? {
            if waiting_for_attach {
                waiting_for_attach = false;
            } else {
                return Ok(());
            }
        }
    }

    bail!("transcript ended before confirming daemon subscription setup")
}

fn read_control_mode_line_before_deadline(
    reader: &mut BufReader<std::process::ChildStdout>,
    deadline: Instant,
) -> Result<Option<String>> {
    wait_for_control_mode_readable(reader, deadline)?;
    read_control_mode_line(reader)
}

fn wait_for_control_mode_readable(
    reader: &BufReader<std::process::ChildStdout>,
    deadline: Instant,
) -> Result<()> {
    if !reader.buffer().is_empty() {
        return Ok(());
    }

    loop {
        let now = Instant::now();
        if now >= deadline {
            bail!("timed out waiting for tmux control-mode subscription setup");
        }
        let timeout = deadline.saturating_duration_since(now);
        let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
        let mut pollfd = libc::pollfd {
            fd: reader.get_ref().as_raw_fd(),
            events: libc::POLLIN | libc::POLLHUP | libc::POLLERR,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
        if result > 0 {
            return Ok(());
        }
        if result == 0 {
            bail!("timed out waiting for tmux control-mode subscription setup");
        }

        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error).context("failed to wait for tmux control-mode output");
        }
    }
}

fn cleanup_startup_child(child: &mut std::process::Child) {
    if let Ok(None) = child.try_wait() {
        let _ = child.kill();
    }
    let _ = child.wait();
}

struct DaemonSocketServer {
    listener: std::os::unix::net::UnixListener,
    socket_path: PathBuf,
    state: DaemonSocketState,
    stop: Arc<AtomicBool>,
}

impl DaemonSocketServer {
    fn bind(socket_path: &Path) -> Result<Self> {
        let listener = std::os::unix::net::UnixListener::bind(socket_path)
            .with_context(|| format!("failed to bind daemon socket {}", socket_path.display()))?;
        listener
            .set_nonblocking(true)
            .context("failed to configure daemon socket listener")?;
        Ok(Self {
            listener,
            socket_path: socket_path.to_path_buf(),
            state: DaemonSocketState::new(),
            stop: Arc::new(AtomicBool::new(false)),
        })
    }

    fn state(&self) -> DaemonSocketState {
        self.state.clone()
    }

    fn spawn(self) -> DaemonSocketServerHandle {
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
        }
    }
}

struct DaemonSocketServerHandle {
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
    socket_path: PathBuf,
}

impl Drop for DaemonSocketServerHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        if let Err(error) = fs::remove_file(&self.socket_path)
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
    latest_snapshot: Option<SnapshotEnvelope>,
    latest_snapshot_frame: Option<EncodedDaemonFrame>,
    pending_handshakes: usize,
    subscribers: HashMap<SubscriberId, SubscriberMailbox>,
    next_subscriber_id: SubscriberId,
}

struct PreparedSnapshot {
    snapshot: SnapshotEnvelope,
    frame: EncodedDaemonFrame,
}

impl PreparedSnapshot {
    fn new(snapshot: SnapshotEnvelope) -> Result<Self> {
        let frame = encode_snapshot_frame(&snapshot)?;
        Ok(Self { snapshot, frame })
    }
}

impl DaemonSocketState {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(DaemonSocketStateInner {
                startup_state: DaemonStartupState::Initializing,
                latest_snapshot: None,
                latest_snapshot_frame: None,
                pending_handshakes: 0,
                subscribers: HashMap::new(),
                next_subscriber_id: 1,
            })),
        }
    }

    #[cfg(test)]
    pub(crate) fn publish_initial_snapshot(&self, snapshot: SnapshotEnvelope) -> Result<()> {
        let prepared = PreparedSnapshot::new(snapshot)
            .context("initial daemon snapshot exceeded socket frame limit")?;
        self.publish_prepared_snapshot(prepared);
        Ok(())
    }

    fn publish_prepared_snapshot(&self, prepared: PreparedSnapshot) {
        let subscribers = {
            let mut inner = self.lock();
            inner.latest_snapshot = Some(prepared.snapshot);
            inner.latest_snapshot_frame = Some(prepared.frame.clone());
            inner.startup_state = DaemonStartupState::Ready;
            subscriber_mailboxes(&inner)
        };
        fan_out_snapshot(prepared.frame, subscribers);
    }

    pub(crate) fn publish_later_snapshot(&self, snapshot: SnapshotEnvelope) {
        match encode_snapshot_frame(&snapshot) {
            Ok(frame) => {
                let subscribers = {
                    let mut inner = self.lock();
                    inner.latest_snapshot = Some(snapshot);
                    inner.latest_snapshot_frame = Some(frame.clone());
                    inner.startup_state = DaemonStartupState::Ready;
                    subscriber_mailboxes(&inner)
                };
                fan_out_snapshot(frame, subscribers);
            }
            Err(error) => {
                eprintln!(
                    "agentscan: skipped daemon socket snapshot update because encoded frame exceeded {} bytes; previous good snapshot remains active: {error:#}",
                    ipc::DAEMON_FRAME_MAX_BYTES
                );
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
                if let Some(frame) = &inner.latest_snapshot_frame {
                    DaemonSocketResponse::Snapshot(frame.clone())
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
                let Some(bootstrap_frame) = inner.latest_snapshot_frame.clone() else {
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
    for subscriber in subscribers.into_values() {
        subscriber.close();
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
        .set_write_timeout(Some(CLIENT_WRITE_TIMEOUT))
        .context("failed to set daemon socket write timeout")?;
    let mut writer = stream
        .try_clone()
        .context("failed to clone daemon socket stream")?;
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
                DaemonSocketResponse::Snapshot(bytes) => writer
                    .write_all(&bytes)
                    .context("failed to write daemon snapshot frame")?,
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
                        writer
                            .write_all(&registration.bootstrap_frame)
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
            let result = writer
                .write_all(&frame)
                .and_then(|()| writer.flush())
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
                if !state.has_subscriber(id) {
                    break;
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

#[derive(Debug, Eq, PartialEq)]
enum ControlEvent {
    PaneChanged(String),
    TitleChanged { pane_id: String, title: String },
    WindowChanged(String),
    SessionChanged(String),
    Resnapshot,
    Exit,
    Ignored,
}

fn control_event_from_line(line: &str) -> ControlEvent {
    if line.starts_with("%exit") {
        return ControlEvent::Exit;
    }

    if let Some(pane_id) = subscription_changed_pane_id(line) {
        return ControlEvent::PaneChanged(pane_id.to_string());
    }

    if let Some(change) = output_title_change(line) {
        return ControlEvent::TitleChanged {
            pane_id: change.pane_id.to_string(),
            title: change.title,
        };
    }

    if let Some(window_id) = window_notification_target(line) {
        return ControlEvent::WindowChanged(window_id.to_string());
    }

    if let Some(session_id) = session_notification_target(line) {
        return ControlEvent::SessionChanged(session_id.to_string());
    }

    if should_resnapshot_from_notification(line) {
        return ControlEvent::Resnapshot;
    }

    ControlEvent::Ignored
}

fn apply_control_event(
    snapshot: &mut SnapshotEnvelope,
    line: &str,
    event: &ControlEvent,
) -> Result<bool> {
    match event {
        ControlEvent::PaneChanged(pane_id) => {
            refresh_snapshot_pane(snapshot, pane_id)?;
            merge_cached_panes(snapshot, Some(pane_id));
            Ok(true)
        }
        ControlEvent::TitleChanged { pane_id, title } => {
            refresh_snapshot_pane_with_title(snapshot, pane_id, Some(title.as_str()))?;
            merge_cached_panes(snapshot, Some(pane_id));
            Ok(true)
        }
        ControlEvent::WindowChanged(window_id) => {
            refresh_snapshot_window(snapshot, window_id)
                .or_else(|error| fallback_to_full_resnapshot(snapshot, line, error))?;
            Ok(true)
        }
        ControlEvent::SessionChanged(session_id) => {
            refresh_snapshot_session(snapshot, session_id)
                .or_else(|error| fallback_to_full_resnapshot(snapshot, line, error))?;
            Ok(true)
        }
        ControlEvent::Resnapshot => {
            reconcile_full_snapshot(snapshot)?;
            Ok(true)
        }
        ControlEvent::Exit | ControlEvent::Ignored => Ok(false),
    }
}

pub(crate) fn read_control_mode_line(reader: &mut impl BufRead) -> Result<Option<String>> {
    let mut bytes = Vec::new();
    let bytes_read = reader
        .read_until(b'\n', &mut bytes)
        .context("failed to read tmux control-mode output")?;
    if bytes_read == 0 {
        return Ok(None);
    }

    if bytes.ends_with(b"\n") {
        bytes.pop();
    }
    if bytes.ends_with(b"\r") {
        bytes.pop();
    }

    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

pub(crate) fn should_resnapshot_from_notification(line: &str) -> bool {
    matches!(
        notification_name(line),
        Some(
            "%sessions-changed"
                | "%session-changed"
                | "%session-renamed"
                | "%session-window-changed"
                | "%layout-change"
                | "%window-add"
                | "%window-close"
                | "%unlinked-window-close"
                | "%window-pane-changed"
                | "%window-renamed"
        )
    )
}

pub(crate) fn subscription_changed_pane_id(line: &str) -> Option<&str> {
    let mut fields = line.split_whitespace();
    if fields.next()? != "%subscription-changed" {
        return None;
    }
    let _subscription_name = fields.next()?;
    let _session = fields.next()?;
    let _window = fields.next()?;
    let _flags = fields.next()?;
    let pane_id = fields.next()?;
    pane_id.starts_with('%').then_some(pane_id)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn output_title_change_pane_id(line: &str) -> Option<&str> {
    output_title_change(line).map(|change| change.pane_id)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn output_title_change_title(line: &str) -> Option<String> {
    output_title_change(line).map(|change| change.title)
}

struct OutputTitleChange<'a> {
    pane_id: &'a str,
    title: String,
}

fn output_title_change(line: &str) -> Option<OutputTitleChange<'_>> {
    let mut fields = line.splitn(3, ' ');
    if fields.next()? != "%output" {
        return None;
    }

    let pane_id = fields.next()?;
    let payload = fields.next()?;
    let title = terminal_title_from_control_payload(payload)?;
    if !pane_id.starts_with('%') {
        return None;
    }

    Some(OutputTitleChange { pane_id, title })
}

fn terminal_title_from_control_payload(payload: &str) -> Option<String> {
    let decoded = decode_tmux_control_payload(payload);
    terminal_title_from_decoded_output(&decoded)
}

fn decode_tmux_control_payload(payload: &str) -> String {
    let bytes = payload.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'\\'
            && index + 3 < bytes.len()
            && is_octal_digit(bytes[index + 1])
            && is_octal_digit(bytes[index + 2])
            && is_octal_digit(bytes[index + 3])
        {
            let value = ((bytes[index + 1] - b'0') << 6)
                | ((bytes[index + 2] - b'0') << 3)
                | (bytes[index + 3] - b'0');
            decoded.push(value);
            index += 4;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }

    String::from_utf8_lossy(&decoded).into_owned()
}

const fn is_octal_digit(byte: u8) -> bool {
    byte >= b'0' && byte <= b'7'
}

fn terminal_title_from_decoded_output(output: &str) -> Option<String> {
    let bytes = output.as_bytes();
    let mut index = 0;
    let mut title = None;

    while index + 4 <= bytes.len() {
        if bytes[index] == 0x1b
            && bytes[index + 1] == b']'
            && matches!(bytes[index + 2], b'0' | b'2')
            && bytes[index + 3] == b';'
        {
            let title_start = index + 4;
            let mut title_end = title_start;
            while title_end < bytes.len() {
                if bytes[title_end] == 0x07 {
                    title =
                        Some(String::from_utf8_lossy(&bytes[title_start..title_end]).into_owned());
                    index = title_end + 1;
                    break;
                }
                if title_end + 1 < bytes.len()
                    && bytes[title_end] == 0x1b
                    && bytes[title_end + 1] == b'\\'
                {
                    title =
                        Some(String::from_utf8_lossy(&bytes[title_start..title_end]).into_owned());
                    index = title_end + 2;
                    break;
                }
                title_end += 1;
            }

            if title_end == bytes.len() {
                break;
            }
        } else {
            index += 1;
        }
    }

    title
}

fn refresh_snapshot_pane(snapshot: &mut SnapshotEnvelope, pane_id: &str) -> Result<()> {
    refresh_snapshot_pane_with_title(snapshot, pane_id, None)
}

fn refresh_snapshot_pane_with_title(
    snapshot: &mut SnapshotEnvelope,
    pane_id: &str,
    title_override: Option<&str>,
) -> Result<()> {
    let pane = tmux::tmux_list_pane(pane_id)?.map(|mut row| {
        if let Some(title) = title_override {
            row.pane_title_raw = title.to_string();
        }
        let mut pane = classify::pane_from_row(row);
        let proc_inspector = proc::ProcProcessInspector;
        classify::apply_proc_fallback(&mut pane, &proc_inspector);
        scanner::apply_pane_output_status_fallbacks(std::slice::from_mut(&mut pane));
        pane.diagnostics.cache_origin = "daemon_update".to_string();
        pane
    });

    if let Some(index) = snapshot
        .panes
        .iter()
        .position(|existing| existing.pane_id == pane_id)
    {
        if let Some(pane) = pane {
            snapshot.panes[index] = pane;
        } else {
            snapshot.panes.remove(index);
        }
    } else if let Some(pane) = pane {
        snapshot.panes.push(pane);
    }

    cache::sort_snapshot_panes(snapshot);
    cache::mark_snapshot_as_daemon(snapshot)
}

fn refresh_snapshot_window(snapshot: &mut SnapshotEnvelope, window_id: &str) -> Result<()> {
    refresh_snapshot_scope(snapshot, TargetScope::Window, window_id)
}

fn refresh_snapshot_session(snapshot: &mut SnapshotEnvelope, session_id: &str) -> Result<()> {
    refresh_snapshot_scope(snapshot, TargetScope::Session, session_id)
}

fn refresh_snapshot_scope(
    snapshot: &mut SnapshotEnvelope,
    scope: TargetScope,
    target_id: &str,
) -> Result<()> {
    let rows = tmux::tmux_list_panes_target(target_id)?;

    snapshot
        .panes
        .retain(|pane| !scope.matches(pane, target_id));

    if let Some(rows) = rows {
        let proc_inspector = proc::ProcProcessInspector;
        let mut panes = classify::panes_from_rows_with_proc_fallback(rows, &proc_inspector);
        scanner::apply_pane_output_status_fallbacks(&mut panes);
        snapshot.panes.extend(panes.into_iter().map(|mut pane| {
            pane.diagnostics.cache_origin = "daemon_update".to_string();
            pane
        }));
    }

    merge_cached_panes(snapshot, None);
    cache::sort_snapshot_panes(snapshot);
    cache::mark_snapshot_as_daemon(snapshot)
}

fn fallback_to_full_resnapshot(
    snapshot: &mut SnapshotEnvelope,
    line: &str,
    error: anyhow::Error,
) -> Result<()> {
    eprintln!(
        "agentscan: targeted refresh failed for control-mode line {:?}: {error:#}",
        line
    );
    reconcile_full_snapshot(snapshot)
}

fn reconcile_full_snapshot(snapshot: &mut SnapshotEnvelope) -> Result<()> {
    *snapshot = cache::daemon_snapshot_from_tmux()?;
    merge_cached_panes(snapshot, None);
    Ok(())
}

fn merge_cached_panes(snapshot: &mut SnapshotEnvelope, excluded_pane_id: Option<&str>) {
    let Some(existing) = cache::read_existing_snapshot_if_valid() else {
        return;
    };

    for pane in &mut snapshot.panes {
        if excluded_pane_id.is_some_and(|pane_id| pane.pane_id == pane_id) {
            continue;
        }

        if let Some(existing_pane) = existing
            .panes
            .iter()
            .find(|cached| cached.pane_id == pane.pane_id)
            && has_more_recent_helper_state(existing_pane, pane)
        {
            *pane = existing_pane.clone();
        }
    }
}

pub(crate) fn window_notification_target(line: &str) -> Option<&str> {
    match notification_name(line) {
        Some(
            "%layout-change"
            | "%window-add"
            | "%window-close"
            | "%unlinked-window-close"
            | "%unlinked-window-renamed"
            | "%window-pane-changed"
            | "%window-renamed",
        ) => line
            .split_whitespace()
            .nth(1)
            .filter(|value| value.starts_with('@')),
        _ => None,
    }
}

pub(crate) fn session_notification_target(line: &str) -> Option<&str> {
    match notification_name(line) {
        Some("%session-renamed") => line
            .split_whitespace()
            .nth(1)
            .filter(|value| value.starts_with('$')),
        _ => None,
    }
}

fn has_more_recent_helper_state(existing: &PaneRecord, current: &PaneRecord) -> bool {
    existing.agent_metadata.provider != current.agent_metadata.provider
        || existing.agent_metadata.label != current.agent_metadata.label
        || existing.agent_metadata.cwd != current.agent_metadata.cwd
        || existing.agent_metadata.state != current.agent_metadata.state
        || existing.agent_metadata.session_id != current.agent_metadata.session_id
}

pub(crate) fn notification_name(line: &str) -> Option<&str> {
    line.split_whitespace()
        .next()
        .filter(|token| token.starts_with('%'))
}

#[derive(Clone, Copy)]
enum TargetScope {
    Window,
    Session,
}

impl TargetScope {
    fn matches(self, pane: &PaneRecord, target_id: &str) -> bool {
        match self {
            Self::Window => pane.tmux.window_id.as_deref() == Some(target_id),
            Self::Session => pane.tmux.session_id.as_deref() == Some(target_id),
        }
    }
}
