use super::*;

pub(super) struct TmuxDaemonReadProvider<'a> {
    fallback: TmuxCommandReadProvider,
    broker: Option<TmuxControlModeReadBroker<'a>>,
}

impl TmuxReadProvider for TmuxDaemonReadProvider<'_> {
    fn list_all_panes(&mut self) -> Result<Vec<TmuxPaneRow>> {
        let Some(broker) = self.broker.as_mut() else {
            return self.fallback.list_all_panes();
        };

        match broker.list_all_panes() {
            Ok(rows) => Ok(rows),
            Err(error) => {
                eprintln!(
                    "agentscan: brokered full reconcile failed; falling back to tmux command: {error:#}"
                );
                self.fallback.list_all_panes()
            }
        }
    }

    fn list_target_panes(&mut self, target: &str) -> Result<Option<Vec<TmuxPaneRow>>> {
        let Some(broker) = self.broker.as_mut() else {
            return self.fallback.list_target_panes(target);
        };

        match broker.list_target_panes(target) {
            Ok(rows) => Ok(rows),
            Err(error) => {
                eprintln!(
                    "agentscan: brokered target refresh failed for {target}; falling back to tmux command: {error:#}"
                );
                self.fallback.list_target_panes(target)
            }
        }
    }

    fn list_pane(&mut self, pane_id: &str) -> Result<Option<TmuxPaneRow>> {
        let Some(broker) = self.broker.as_mut() else {
            return self.fallback.list_pane(pane_id);
        };

        match broker.list_pane(pane_id) {
            Ok(pane) => Ok(pane),
            Err(error) => {
                eprintln!(
                    "agentscan: brokered pane refresh failed for {pane_id}; falling back to tmux command: {error:#}"
                );
                self.fallback.list_pane(pane_id)
            }
        }
    }
}

struct TmuxControlModeReadBroker<'a> {
    stdin: &'a mut std::process::ChildStdin,
    line_rx: &'a mpsc::Receiver<Result<String>>,
    deferred_lines: &'a mut VecDeque<String>,
    broker_health: &'a mut TmuxBrokerHealth,
}

impl TmuxControlModeReadBroker<'_> {
    fn list_all_panes(&mut self) -> Result<Vec<TmuxPaneRow>> {
        match control_mode_list_all_panes(self.stdin, self.line_rx, self.deferred_lines) {
            Ok(rows) => Ok(rows),
            Err(error) => {
                self.broker_health.disable_after_error(&error);
                Err(error)
            }
        }
    }

    fn list_target_panes(&mut self, target: &str) -> Result<Option<Vec<TmuxPaneRow>>> {
        match control_mode_list_panes_target(
            self.stdin,
            self.line_rx,
            target,
            self.deferred_lines,
            MissingTargetScope::PaneWindowSession,
        ) {
            Ok(rows) => Ok(rows),
            Err(error) => {
                self.broker_health.disable_after_error(&error);
                Err(error)
            }
        }
    }

    fn list_pane(&mut self, pane_id: &str) -> Result<Option<TmuxPaneRow>> {
        match control_mode_list_panes_target(
            self.stdin,
            self.line_rx,
            pane_id,
            self.deferred_lines,
            MissingTargetScope::PaneWindow,
        ) {
            Ok(rows) => Ok(rows.and_then(|mut rows| rows.pop())),
            Err(error) => {
                self.broker_health.disable_after_error(&error);
                Err(error)
            }
        }
    }
}

#[derive(Default)]
struct TmuxBrokerHealth {
    disabled_reason: Option<String>,
    reconnect_count: u32,
    fallback_count: u64,
}

impl TmuxBrokerHealth {
    fn enabled(&self) -> bool {
        self.disabled_reason.is_none()
    }

    fn disable_after_error(&mut self, error: &anyhow::Error) {
        if self.disabled_reason.is_none() {
            self.fallback_count = self.fallback_count.saturating_add(1);
            self.disabled_reason = Some(format!("{error:#}"));
        }
    }

    fn mark_reconnected(&mut self) {
        self.disabled_reason = None;
        self.reconnect_count = self.reconnect_count.saturating_add(1);
    }

    fn status_frame(&self) -> ipc::ControlModeBrokerStatusFrame {
        ipc::ControlModeBrokerStatusFrame {
            mode: if self.enabled() {
                ipc::ControlModeBrokerMode::Active
            } else {
                ipc::ControlModeBrokerMode::Fallback
            },
            disabled_reason: self.disabled_reason.clone(),
            reconnect_count: self.reconnect_count,
            fallback_count: Some(self.fallback_count),
            // Filled in by `RunningTmuxControlModeClient::broker_status_frame`,
            // which owns the subscriber set; broker health alone cannot see it.
            subscriber_count: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn test_disabled_reason(&self) -> Option<&str> {
        self.disabled_reason.as_deref()
    }
}

#[cfg(test)]
pub(crate) fn test_broker_health_after_error(message: &str) -> (bool, Option<String>, u64) {
    let mut health = TmuxBrokerHealth::default();
    let error = anyhow::anyhow!(message.to_string());
    health.disable_after_error(&error);
    (
        health.enabled(),
        health.test_disabled_reason().map(str::to_string),
        health.fallback_count,
    )
}

#[cfg(test)]
pub(crate) fn test_broker_health_after_repeated_error(message: &str) -> (Option<String>, u64) {
    let mut health = TmuxBrokerHealth::default();
    let error = anyhow::anyhow!(message.to_string());
    health.disable_after_error(&error);
    health.disable_after_error(&error);
    (
        health.test_disabled_reason().map(str::to_string),
        health.fallback_count,
    )
}

#[cfg(test)]
pub(crate) fn test_broker_health_after_reconnect(
    message: &str,
) -> ipc::ControlModeBrokerStatusFrame {
    let mut health = TmuxBrokerHealth::default();
    let error = anyhow::anyhow!(message.to_string());
    health.disable_after_error(&error);
    health.mark_reconnected();
    health.status_frame()
}

#[cfg(test)]
pub(crate) fn test_reconnect_preserves_deferred_lines() -> Vec<String> {
    let deferred_lines = VecDeque::from([
        "%subscription-changed agentscan $1 @1 0 %1 : %1:Codex:codex::::".to_string(),
    ]);
    let mut health = TmuxBrokerHealth::default();
    let error = anyhow::anyhow!("broken pipe");
    health.disable_after_error(&error);
    health.mark_reconnected();

    deferred_lines.into_iter().collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MissingTargetScope {
    PaneWindow,
    PaneWindowSession,
}

impl MissingTargetScope {
    fn matches(self, message: &str) -> bool {
        tmux::tmux_target_is_missing(message.as_bytes())
            || (self == MissingTargetScope::PaneWindowSession
                && message.contains("can't find session"))
    }
}

pub(crate) struct StartedTmuxControlModeClient {
    child: Option<std::process::Child>,
    stdout_reader: Option<BufReader<std::process::ChildStdout>>,
    stdin: Option<std::process::ChildStdin>,
}

impl StartedTmuxControlModeClient {
    pub(super) fn from_real(
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
}

pub(super) struct RunningTmuxControlModeClient {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    line_rx: mpsc::Receiver<Result<String>>,
    // Retained so per-session subscriber clients and primary reconnects can feed
    // the same shared event stream the run loop drains. Because this keeps a live
    // sender, the channel never reports `Disconnected` on its own; primary-client
    // death is instead detected by the events the primary forwards — `%exit` on a
    // clean tmux exit and a `Fatal` read error on an abnormal one — not by channel
    // closure.
    line_tx: mpsc::Sender<Result<String>>,
    // Event-only control clients, one per non-primary session. tmux control mode
    // is scoped to the attached session, so these provide event coverage for
    // panes the primary client cannot see. They never issue commands; their
    // reader threads feed the shared channel.
    subscribers: Vec<SubscriberClient>,
    // False when there are more non-primary sessions than the subscriber cap, so
    // some sessions have no event client. The run loop keeps the reconcile poll
    // active in that case rather than relaxing to the self-heal backstop.
    subscriber_coverage_complete: bool,
    deferred_lines: VecDeque<String>,
    broker_health: TmuxBrokerHealth,
}

struct SubscriberClient {
    session_id: String,
    child: std::process::Child,
    // Retained for keep-alive and per-pane output gating (`refresh-client -A`).
    #[allow(dead_code)]
    stdin: std::process::ChildStdin,
}

impl Drop for SubscriberClient {
    fn drop(&mut self) {
        cleanup_startup_child(&mut self.child);
    }
}

impl RunningTmuxControlModeClient {
    pub(super) fn from_started(started: StartedTmuxControlModeClient) -> Result<Self> {
        let (child, stdin, line_rx, line_tx) = connect_primary_control_client(started)?;
        Ok(Self {
            child,
            stdin,
            line_rx,
            line_tx,
            subscribers: Vec::new(),
            subscriber_coverage_complete: true,
            deferred_lines: VecDeque::new(),
            broker_health: TmuxBrokerHealth::default(),
        })
    }

    // Attach an event-only subscriber client for a non-primary session. Its
    // reader feeds the shared channel; a subscriber read error ends only its own
    // thread (Quiet) so one failing session does not bounce the daemon.
    pub(super) fn attach_subscriber(
        &mut self,
        session_id: String,
        started: StartedTmuxControlModeClient,
    ) -> Result<()> {
        if self.subscribers.iter().any(|s| s.session_id == session_id) {
            return Ok(());
        }
        let (child, stdin) =
            spawn_control_client_reader(started, self.line_tx.clone(), ClientErrorMode::Quiet)
                .with_context(|| {
                    format!("failed to attach subscriber control client for session {session_id}")
                })?;
        self.subscribers.push(SubscriberClient {
            session_id,
            child,
            stdin,
        });
        Ok(())
    }

    pub(super) fn has_subscriber(&self, session_id: &str) -> bool {
        self.subscribers
            .iter()
            .any(|subscriber| subscriber.session_id == session_id)
    }

    // Drop subscribers whose client process has exited so a following reconcile
    // re-attaches them. Covers the case where a subscriber client dies while its
    // session is still alive (a closed session is instead handled by
    // `retain_subscriber_sessions`, since the session leaves the desired set).
    pub(super) fn prune_dead_subscribers(&mut self) {
        self.subscribers
            .retain_mut(|subscriber| !matches!(subscriber.child.try_wait(), Ok(Some(_))));
    }

    pub(super) fn subscriber_count(&self) -> usize {
        self.subscribers.len()
    }

    // Whether the primary control client process has exited. Because the retained
    // `line_tx` keeps the shared channel from ever reporting `Disconnected`, the
    // run loop polls this to detect a primary that died without emitting `%exit`
    // (e.g. the tmux server was SIGKILLed and the pipe closed at EOF). Checks the
    // current primary child, so it is correct across reconnects (which install a
    // fresh, live child).
    pub(super) fn primary_child_exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    pub(super) fn set_subscriber_coverage_complete(&mut self, complete: bool) {
        self.subscriber_coverage_complete = complete;
    }

    pub(super) fn subscriber_coverage_complete(&self) -> bool {
        self.subscriber_coverage_complete
    }

    // Drop subscriber clients whose sessions are no longer desired (each dropped
    // `SubscriberClient` cleans up its child process).
    pub(super) fn retain_subscriber_sessions(&mut self, desired: &[String]) {
        self.subscribers
            .retain(|subscriber| desired.iter().any(|id| id == &subscriber.session_id));
    }

    pub(super) fn read_provider(&mut self) -> TmuxDaemonReadProvider<'_> {
        TmuxDaemonReadProvider {
            fallback: TmuxCommandReadProvider,
            broker: self
                .broker_health
                .enabled()
                .then_some(TmuxControlModeReadBroker {
                    stdin: &mut self.stdin,
                    line_rx: &self.line_rx,
                    deferred_lines: &mut self.deferred_lines,
                    broker_health: &mut self.broker_health,
                }),
        }
    }

    pub(super) fn recv_timeout(
        &mut self,
        timeout: Duration,
    ) -> std::result::Result<Result<String>, mpsc::RecvTimeoutError> {
        if let Some(line) = self.deferred_lines.pop_front() {
            Ok(Ok(line))
        } else {
            self.line_rx.recv_timeout(timeout)
        }
    }

    pub(super) fn broker_status_frame(&self) -> ipc::ControlModeBrokerStatusFrame {
        ipc::ControlModeBrokerStatusFrame {
            subscriber_count: Some(self.subscriber_count()),
            ..self.broker_health.status_frame()
        }
    }

    pub(super) fn broker_enabled(&self) -> bool {
        self.broker_health.enabled()
    }

    pub(super) fn recover_broker_if_disabled(
        &mut self,
        startup: &impl StartupActions,
        socket_state: &DaemonSocketState,
    ) -> bool {
        if self.broker_health.enabled() {
            return false;
        }

        socket_state.update_control_mode_broker_status(self.broker_status_frame());
        match self.reconnect(startup) {
            Ok(()) => {
                socket_state.update_control_mode_broker_status(self.broker_status_frame());
                true
            }
            Err(error) => {
                eprintln!(
                    "agentscan: failed to reconnect tmux control-mode broker; continuing with short-lived tmux reads: {error:#}"
                );
                false
            }
        }
    }

    fn reconnect(&mut self, startup: &impl StartupActions) -> Result<()> {
        let started = startup
            .start_tmux_control_mode_client()
            .context("failed to restart tmux control-mode client")?;
        // Re-spawn the primary reader into the existing shared channel rather than
        // recreating it, so any attached subscriber clients keep delivering events.
        let (mut replacement_child, replacement_stdin) =
            spawn_control_client_reader(started, self.line_tx.clone(), ClientErrorMode::Fatal)
                .context("failed to configure restarted tmux control-mode client")?;

        std::mem::swap(&mut self.child, &mut replacement_child);
        cleanup_startup_child(&mut replacement_child);
        self.stdin = replacement_stdin;
        self.broker_health.mark_reconnected();
        Ok(())
    }

    pub(super) fn wait_for_exit(&mut self) -> Result<()> {
        let status = self
            .child
            .wait()
            .context("failed while waiting for tmux control-mode client to exit")?;
        if !status.success() {
            bail!("tmux control-mode client exited with status {status}");
        }
        Ok(())
    }

    pub(super) fn terminate(&mut self) {
        // Dropping each subscriber cleans up its child process.
        self.subscribers.clear();
        cleanup_startup_child(&mut self.child);
    }
}

impl Drop for RunningTmuxControlModeClient {
    fn drop(&mut self) {
        cleanup_startup_child(&mut self.child);
    }
}

// Connect a control client's stdout to a shared event channel. The channel is
// owned at the broker level and stays stable across primary reconnects, so the
// returned `line_tx` can be cloned to feed per-session subscriber clients into
// the same stream the run loop drains.
type PrimaryControlConnection = (
    std::process::Child,
    std::process::ChildStdin,
    mpsc::Receiver<Result<String>>,
    mpsc::Sender<Result<String>>,
);

fn connect_primary_control_client(
    started: StartedTmuxControlModeClient,
) -> Result<PrimaryControlConnection> {
    let (line_tx, line_rx) = mpsc::channel();
    let (child, stdin) =
        spawn_control_client_reader(started, line_tx.clone(), ClientErrorMode::Fatal)?;
    Ok((child, stdin, line_rx, line_tx))
}

// How a client's reader thread reports a read error on its stdout. The primary
// client's errors are fatal (they propagate to the run loop and bounce the
// daemon into broker recovery), but a subscriber client dying must not take the
// daemon down — it just ends its own thread and is reconnected by the lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ClientErrorMode {
    Fatal,
    Quiet,
}

fn spawn_control_client_reader(
    mut started: StartedTmuxControlModeClient,
    line_tx: mpsc::Sender<Result<String>>,
    error_mode: ClientErrorMode,
) -> Result<(std::process::Child, std::process::ChildStdin)> {
    let stdout_reader = started
        .stdout_reader
        .take()
        .context("tmux control-mode client did not provide stdout")?;
    let child = started
        .child
        .take()
        .context("tmux control-mode client did not provide child process")?;
    let stdin = started
        .stdin
        .take()
        .context("tmux control-mode client did not provide stdin")?;
    spawn_control_mode_line_reader(stdout_reader, line_tx, error_mode);
    Ok((child, stdin))
}

// A `%exit` on a subscriber (Quiet) client signals only that this one
// non-primary control client is detaching, so it must not be forwarded to the
// shared stream (where `%exit` parses as a server-wide `ControlEvent::Exit`).
// The primary (Fatal) client still forwards `%exit` to drive daemon shutdown.
fn subscriber_local_exit(error_mode: ClientErrorMode, line: &str) -> bool {
    matches!(error_mode, ClientErrorMode::Quiet) && is_control_exit_line(line)
}

#[cfg(test)]
pub(crate) fn test_subscriber_local_exit(quiet: bool, line: &str) -> bool {
    let error_mode = if quiet {
        ClientErrorMode::Quiet
    } else {
        ClientErrorMode::Fatal
    };
    subscriber_local_exit(error_mode, line)
}

fn spawn_control_mode_line_reader(
    stdout_reader: BufReader<std::process::ChildStdout>,
    line_tx: mpsc::Sender<Result<String>>,
    error_mode: ClientErrorMode,
) {
    std::thread::spawn(move || {
        let mut reader = stdout_reader;
        loop {
            match read_control_mode_line(&mut reader) {
                Ok(Some(line)) => {
                    // A subscriber client's `%exit` reports only that this one
                    // client is detaching (e.g. its session was killed); it must
                    // not reach the shared stream, where `%exit` is parsed as a
                    // server-wide `ControlEvent::Exit` that stops the daemon loop.
                    // The subscriber's removal is handled by `%sessions-changed`
                    // reconcile and dead-subscriber pruning instead. The primary
                    // client (Fatal) still forwards `%exit` to drive shutdown.
                    if subscriber_local_exit(error_mode, &line) {
                        break;
                    }
                    if line_tx.send(Ok(line)).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    if matches!(error_mode, ClientErrorMode::Fatal) {
                        let _ = line_tx.send(Err(error));
                    }
                    break;
                }
            }
        }
    });
}

extern "C" fn daemon_shutdown_signal_handler(_signal: libc::c_int) {
    DAEMON_SHUTDOWN_REQUESTED.store(true, Ordering::Relaxed);
}

pub(super) fn install_shutdown_signal_handlers() {
    unsafe {
        libc::signal(
            libc::SIGTERM,
            daemon_shutdown_signal_handler as *const () as usize,
        );
        libc::signal(
            libc::SIGINT,
            daemon_shutdown_signal_handler as *const () as usize,
        );
    }
}

pub(super) struct DaemonClosingGuard {
    state: DaemonSocketState,
    marked: bool,
}

impl DaemonClosingGuard {
    pub(super) fn new(state: DaemonSocketState) -> Self {
        Self {
            state,
            marked: false,
        }
    }

    pub(super) fn mark_closing(&mut self) {
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

pub(super) fn startup_failure_message(context: &str, error: &anyhow::Error) -> String {
    format!(
        "{context} failed before daemon socket readiness; no usable socket snapshot was published: {error:#}"
    )
}

// All control clients attach with these flags:
//   ignore-size: the client never participates in window-size calculation, so
//     attaching (especially to a detached session) cannot resize the user's panes.
//   no-output: tmux never sends `%output` to the client. The daemon does not need
//     the pty firehose — status/title/command/metadata arrive via the 1s
//     `%subscription-changed` stream and topology via structural notifications.
//     This is what keeps cost flat as the number of active agents grows.
const CONTROL_CLIENT_ATTACH_FLAGS: &str = "ignore-size,no-output";

// Start the primary control client attached to an explicit session target. The
// caller resolves and owns the primary session id (once, at startup) so the
// primary attach and the subscriber-exclusion set agree and do not drift if the
// launching tmux client later switches sessions.
pub(super) fn start_tmux_control_mode_client_for(
    session_target: &str,
) -> Result<(
    std::process::Child,
    BufReader<std::process::ChildStdout>,
    std::process::ChildStdin,
)> {
    start_control_mode_client_for(session_target)
}

pub(super) fn start_subscriber_control_mode_client(
    session_id: &str,
) -> Result<(
    std::process::Child,
    BufReader<std::process::ChildStdout>,
    std::process::ChildStdin,
)> {
    start_control_mode_client_for(session_id)
}

fn start_control_mode_client_for(
    session_target: &str,
) -> Result<(
    std::process::Child,
    BufReader<std::process::ChildStdout>,
    std::process::ChildStdin,
)> {
    let mut child = tmux::tmux_command()
        .args([
            "-C",
            "attach-session",
            "-f",
            CONTROL_CLIENT_ATTACH_FLAGS,
            "-t",
            session_target,
        ])
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

pub(super) fn control_mode_startup_response_from_line(line: &str, context: &str) -> Result<bool> {
    if line.starts_with("%error") {
        bail!("tmux rejected {context}: {line}");
    }
    Ok(line.starts_with("%end"))
}

fn control_mode_list_all_panes(
    stdin: &mut std::process::ChildStdin,
    line_rx: &mpsc::Receiver<Result<String>>,
    deferred_lines: &mut VecDeque<String>,
) -> Result<Vec<TmuxPaneRow>> {
    writeln!(stdin, "list-panes -a -F '{PANE_FORMAT}'")
        .context("failed to write brokered tmux list-panes -a")?;
    stdin
        .flush()
        .context("failed to flush brokered tmux list-panes -a")?;

    match collect_control_mode_command_outcome(
        line_rx,
        CONTROL_MODE_COMMAND_TIMEOUT,
        deferred_lines,
    )? {
        ControlModeBrokerCommandOutcome::Response(response) => {
            tmux::parse_pane_rows(&response.output.join("\n"))
        }
        ControlModeBrokerCommandOutcome::Error { message, .. } => {
            bail!("tmux control-mode command failed: {message}");
        }
    }
}

fn control_mode_list_panes_target(
    stdin: &mut std::process::ChildStdin,
    line_rx: &mpsc::Receiver<Result<String>>,
    target: &str,
    deferred_lines: &mut VecDeque<String>,
    missing_target_scope: MissingTargetScope,
) -> Result<Option<Vec<TmuxPaneRow>>> {
    writeln!(stdin, "list-panes -t {target} -F '{PANE_FORMAT}'")
        .with_context(|| format!("failed to write brokered tmux list-panes for {target}"))?;
    stdin
        .flush()
        .with_context(|| format!("failed to flush brokered tmux list-panes for {target}"))?;

    match collect_control_mode_command_outcome(
        line_rx,
        CONTROL_MODE_COMMAND_TIMEOUT,
        deferred_lines,
    )? {
        ControlModeBrokerCommandOutcome::Response(response) => {
            let rows = tmux::parse_pane_rows(&response.output.join("\n"))?;
            Ok(Some(rows))
        }
        ControlModeBrokerCommandOutcome::Error {
            message,
            deferred_events: _,
        } if missing_target_scope.matches(&message) => Ok(None),
        ControlModeBrokerCommandOutcome::Error { message, .. } => {
            bail!("tmux control-mode command failed: {message}");
        }
    }
}

fn collect_control_mode_command_outcome(
    line_rx: &mpsc::Receiver<Result<String>>,
    timeout: Duration,
    deferred_lines: &mut VecDeque<String>,
) -> Result<ControlModeBrokerCommandOutcome> {
    let deadline = Instant::now() + timeout;
    let mut active_id = None;
    let mut output = Vec::new();

    loop {
        let now = Instant::now();
        if now >= deadline {
            bail!("timed out waiting for control-mode command response");
        }
        let line = line_rx
            .recv_timeout(deadline.saturating_duration_since(now))
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => {
                    anyhow::anyhow!("timed out waiting for control-mode command response")
                }
                mpsc::RecvTimeoutError::Disconnected => {
                    anyhow::anyhow!(
                        "tmux control-mode stream ended before command response completed"
                    )
                }
            })??;

        if active_id.is_some() && control_mode_broker_line_is_command_output(&line) {
            output.push(line);
            continue;
        }

        match (active_id.as_ref(), control_mode_command_marker(&line)) {
            (None, Some(ControlModeCommandMarker::Begin(id))) => active_id = Some(id),
            (Some(id), Some(ControlModeCommandMarker::End(end_id))) if *id == end_id => {
                return Ok(ControlModeBrokerCommandOutcome::Response(
                    ControlModeCommandPrototypeResponse {
                        output,
                        deferred_events: Vec::new(),
                    },
                ));
            }
            (
                Some(id),
                Some(ControlModeCommandMarker::Error {
                    id: error_id,
                    message,
                }),
            ) if *id == error_id => {
                let message = control_mode_command_error_message(message, &output);
                return Ok(ControlModeBrokerCommandOutcome::Error {
                    message,
                    deferred_events: Vec::new(),
                });
            }
            (Some(_), Some(_)) => {
                bail!("interleaved control-mode command frame before expected %end");
            }
            (Some(_), None) if control_mode_broker_should_defer_line(&line) => {
                deferred_lines.push_back(line);
            }
            (Some(_), None) => output.push(line),
            (None, Some(_)) | (None, None) => deferred_lines.push_back(line),
        }
    }
}

fn control_mode_broker_line_is_command_output(line: &str) -> bool {
    line.contains(PANE_DELIM) || line.contains(TMUX_FORMAT_DELIM)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ControlModeCommandFrameId {
    pub(crate) timestamp: String,
    pub(crate) command_number: String,
    pub(crate) flags: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ControlModeCommandMarker {
    Begin(ControlModeCommandFrameId),
    End(ControlModeCommandFrameId),
    Error {
        id: ControlModeCommandFrameId,
        message: String,
    },
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn control_mode_command_marker(line: &str) -> Option<ControlModeCommandMarker> {
    let mut parts = line.splitn(5, ' ');
    let marker = parts.next()?;
    if !matches!(marker, "%begin" | "%end" | "%error") {
        return None;
    }

    let id = ControlModeCommandFrameId {
        timestamp: parts.next()?.to_string(),
        command_number: parts.next()?.to_string(),
        flags: parts.next()?.to_string(),
    };

    match marker {
        "%begin" => Some(ControlModeCommandMarker::Begin(id)),
        "%end" => Some(ControlModeCommandMarker::End(id)),
        "%error" => Some(ControlModeCommandMarker::Error {
            id,
            message: parts.next().unwrap_or_default().to_string(),
        }),
        _ => None,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ControlModeCommandPrototypeResponse {
    pub(crate) output: Vec<String>,
    pub(crate) deferred_events: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ControlModeBrokerCommandOutcome {
    Response(ControlModeCommandPrototypeResponse),
    Error {
        message: String,
        deferred_events: Vec<String>,
    },
}

#[cfg(test)]
pub(crate) fn test_collect_control_mode_command_response<'a>(
    expected_id: &ControlModeCommandFrameId,
    lines: impl IntoIterator<Item = &'a str>,
) -> Result<ControlModeCommandPrototypeResponse> {
    let mut started = false;
    let mut output = Vec::new();
    let mut deferred_events = Vec::new();

    for line in lines {
        match control_mode_command_marker(line) {
            Some(ControlModeCommandMarker::Begin(id)) if id == *expected_id && !started => {
                started = true;
            }
            Some(ControlModeCommandMarker::Begin(_)) if started => {
                bail!("interleaved control-mode command frame before expected %end");
            }
            Some(ControlModeCommandMarker::End(id)) if id == *expected_id && started => {
                return Ok(ControlModeCommandPrototypeResponse {
                    output,
                    deferred_events,
                });
            }
            Some(ControlModeCommandMarker::Error { id, message })
                if id == *expected_id && started =>
            {
                bail!("tmux control-mode command failed: {message}");
            }
            Some(_) if started => {
                bail!("interleaved control-mode command frame before expected %end");
            }
            Some(_) | None if started => output.push(line.to_string()),
            Some(_) | None => deferred_events.push(line.to_string()),
        }
    }

    bail!("control-mode command response ended before expected %end")
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ControlModeBrokerTranscriptStep {
    Line(String),
    Timeout,
    Eof,
}

#[cfg(test)]
impl ControlModeBrokerTranscriptStep {
    pub(crate) fn line(line: impl Into<String>) -> Self {
        Self::Line(line.into())
    }
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct ControlModeBrokerTranscriptHarness {
    steps: std::collections::VecDeque<ControlModeBrokerTranscriptStep>,
    written_commands: Vec<String>,
}

#[cfg(test)]
impl ControlModeBrokerTranscriptHarness {
    pub(crate) fn new(steps: impl IntoIterator<Item = ControlModeBrokerTranscriptStep>) -> Self {
        Self {
            steps: steps.into_iter().collect(),
            written_commands: Vec::new(),
        }
    }

    pub(crate) fn written_commands(&self) -> &[String] {
        &self.written_commands
    }

    pub(crate) fn list_all_panes(
        &mut self,
        expected_id: &ControlModeCommandFrameId,
    ) -> Result<ControlModeBrokerListAllResponse> {
        self.written_commands
            .push(format!("list-panes -a -F {PANE_FORMAT}"));

        let response = match self.collect_command_outcome(expected_id)? {
            ControlModeBrokerCommandOutcome::Response(response) => response,
            ControlModeBrokerCommandOutcome::Error { message, .. } => {
                bail!("tmux control-mode command failed: {message}");
            }
        };
        let rows = tmux::parse_pane_rows(&response.output.join("\n"))?;
        Ok(ControlModeBrokerListAllResponse {
            rows,
            deferred_events: response.deferred_events,
        })
    }

    pub(crate) fn list_pane(
        &mut self,
        pane_id: &str,
        expected_id: &ControlModeCommandFrameId,
    ) -> Result<ControlModeBrokerListPaneResponse> {
        let response = self.list_target_panes_with_scope(
            pane_id,
            expected_id,
            MissingTargetScope::PaneWindow,
        )?;
        Ok(ControlModeBrokerListPaneResponse {
            pane: response.rows.and_then(|mut rows| rows.pop()),
            deferred_events: response.deferred_events,
        })
    }

    pub(crate) fn list_target_panes(
        &mut self,
        target: &str,
        expected_id: &ControlModeCommandFrameId,
    ) -> Result<ControlModeBrokerListTargetResponse> {
        self.list_target_panes_with_scope(
            target,
            expected_id,
            MissingTargetScope::PaneWindowSession,
        )
    }

    fn list_target_panes_with_scope(
        &mut self,
        target: &str,
        expected_id: &ControlModeCommandFrameId,
        missing_target_scope: MissingTargetScope,
    ) -> Result<ControlModeBrokerListTargetResponse> {
        self.written_commands
            .push(format!("list-panes -t {target} -F {PANE_FORMAT}"));

        let response = match self.collect_command_outcome(expected_id)? {
            ControlModeBrokerCommandOutcome::Response(response) => response,
            ControlModeBrokerCommandOutcome::Error {
                message,
                deferred_events,
            } if missing_target_scope.matches(&message) => {
                return Ok(ControlModeBrokerListTargetResponse {
                    rows: None,
                    deferred_events,
                });
            }
            ControlModeBrokerCommandOutcome::Error { message, .. } => {
                bail!("tmux control-mode command failed: {message}");
            }
        };
        let rows = tmux::parse_pane_rows(&response.output.join("\n"))?;
        Ok(ControlModeBrokerListTargetResponse {
            rows: Some(rows),
            deferred_events: response.deferred_events,
        })
    }

    pub(crate) fn collect_command_response(
        &mut self,
        expected_id: &ControlModeCommandFrameId,
    ) -> Result<ControlModeCommandPrototypeResponse> {
        match self.collect_command_outcome(expected_id)? {
            ControlModeBrokerCommandOutcome::Response(response) => Ok(response),
            ControlModeBrokerCommandOutcome::Error { message, .. } => {
                bail!("tmux control-mode command failed: {message}");
            }
        }
    }

    fn collect_command_outcome(
        &mut self,
        expected_id: &ControlModeCommandFrameId,
    ) -> Result<ControlModeBrokerCommandOutcome> {
        let mut started = false;
        let mut output = Vec::new();
        let mut deferred_events = Vec::new();

        loop {
            let Some(line) = self.next_line()? else {
                bail!("tmux control-mode stream ended before command response completed");
            };

            if started && control_mode_broker_line_is_command_output(&line) {
                output.push(line);
                continue;
            }

            match control_mode_command_marker(&line) {
                Some(ControlModeCommandMarker::Begin(id)) if id == *expected_id && !started => {
                    started = true;
                }
                Some(ControlModeCommandMarker::Begin(_)) if started => {
                    bail!("interleaved control-mode command frame before expected %end");
                }
                Some(ControlModeCommandMarker::End(id)) if id == *expected_id && started => {
                    return Ok(ControlModeBrokerCommandOutcome::Response(
                        ControlModeCommandPrototypeResponse {
                            output,
                            deferred_events,
                        },
                    ));
                }
                Some(ControlModeCommandMarker::Error { id, message })
                    if id == *expected_id && started =>
                {
                    let message = control_mode_command_error_message(message, &output);
                    return Ok(ControlModeBrokerCommandOutcome::Error {
                        message,
                        deferred_events,
                    });
                }
                Some(_) if started => {
                    bail!("interleaved control-mode command frame before expected %end");
                }
                None if started && control_mode_broker_should_defer_line(&line) => {
                    deferred_events.push(line);
                }
                Some(_) | None if started => output.push(line),
                Some(_) | None => deferred_events.push(line),
            }
        }
    }

    fn next_line(&mut self) -> Result<Option<String>> {
        match self.steps.pop_front() {
            Some(ControlModeBrokerTranscriptStep::Line(line)) => Ok(Some(line)),
            Some(ControlModeBrokerTranscriptStep::Timeout) => {
                bail!("timed out waiting for control-mode command response");
            }
            Some(ControlModeBrokerTranscriptStep::Eof) | None => Ok(None),
        }
    }
}

fn control_mode_command_error_message(marker_message: String, output: &[String]) -> String {
    if marker_message.is_empty() {
        output.last().cloned().unwrap_or_default()
    } else {
        marker_message
    }
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct ControlModeBrokerListPaneResponse {
    pub(crate) pane: Option<TmuxPaneRow>,
    pub(crate) deferred_events: Vec<String>,
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct ControlModeBrokerListAllResponse {
    pub(crate) rows: Vec<TmuxPaneRow>,
    pub(crate) deferred_events: Vec<String>,
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct ControlModeBrokerListTargetResponse {
    pub(crate) rows: Option<Vec<TmuxPaneRow>>,
    pub(crate) deferred_events: Vec<String>,
}

fn control_mode_broker_should_defer_line(line: &str) -> bool {
    !matches!(control_event_from_line(line), ControlEvent::Ignored)
}
