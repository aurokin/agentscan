use super::*;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use std::time::Instant;

mod control_mode;
mod lifecycle;
mod snapshot_store;
mod socket_server;

pub(crate) use control_mode::StartedTmuxControlModeClient;
#[cfg(test)]
use control_mode::control_mode_startup_response_from_line;
#[cfg(test)]
pub(crate) use control_mode::{
    ControlModeBrokerTranscriptHarness, ControlModeBrokerTranscriptStep, ControlModeCommandFrameId,
    ControlModeCommandMarker, control_mode_command_marker, test_broker_health_after_error,
    test_broker_health_after_reconnect, test_broker_health_after_repeated_error,
    test_collect_control_mode_command_response, test_reconnect_preserves_deferred_lines,
};
use control_mode::{
    DaemonClosingGuard, RunningTmuxControlModeClient, install_shutdown_signal_handlers,
    start_tmux_control_mode_client, startup_failure_message,
};
pub(crate) use lifecycle::{
    AutoStartPolicy, DaemonSnapshotError, daemon_restart, daemon_run, daemon_start, daemon_status,
    daemon_stop, snapshot_via_socket, snapshot_via_socket_path_with_start_command,
    spawn_subscription_worker, stream_subscription_events_json,
};
use lifecycle::{DaemonLifecycleGuard, LifecyclePaths, remove_stale_socket_if_present};
#[cfg(test)]
pub(crate) use lifecycle::{
    daemon_status_with_socket_path, snapshot_via_socket_path, test_daemon_restart_with_steps,
    test_daemon_start_env_removes_from, test_daemon_start_tmux_envs_from,
    test_explicit_macos_daemon_start_preflight, test_implicit_consumer_macos_auto_start_preflight,
    test_macos_executable_assessment_for_outputs, test_macos_start_requires_trust_preflight,
    test_tui_macos_auto_start_preflight,
};
use snapshot_store::SnapshotStore;
pub(crate) use socket_server::DaemonSocketState;
use socket_server::{
    DaemonSocketServer, DaemonSocketServerHandle, PreparedSnapshot, SnapshotPublishContext,
};
#[cfg(test)]
pub(crate) use socket_server::{
    SubscriberMailbox, handle_daemon_socket_client, refuse_server_busy,
};

const RECONCILE_INTERVAL: Duration = Duration::from_secs(1);
const CONTROL_MODE_EVENT_BATCH_WINDOW: Duration = Duration::from_millis(50);
const CONTROL_MODE_MIN_WAIT: Duration = Duration::from_millis(1);
const PANE_OUTPUT_STATUS_CACHE_TTL: Duration = Duration::from_secs(2);
const STARTUP_FAILURE_OBSERVABILITY_WINDOW: Duration = Duration::from_millis(200);
const CONTROL_MODE_STARTUP_TIMEOUT: Duration = Duration::from_secs(2);
const CONTROL_MODE_COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
const CLIENT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);
const CLIENT_WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const SUBSCRIBER_WRITE_TIMEOUT: Duration = Duration::from_millis(250);
const SUBSCRIBER_MONITOR_POLL_INTERVAL: Duration = Duration::from_millis(250);
pub(crate) const MAX_PENDING_HANDSHAKES: usize = 8;
pub(crate) const MAX_SUBSCRIBERS: usize = 64;
const LIFECYCLE_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const DAEMON_START_READINESS_TIMEOUT: Duration = Duration::from_secs(5);
const DAEMON_STOP_TIMEOUT: Duration = Duration::from_secs(3);
const LIFECYCLE_POLL_INTERVAL: Duration = Duration::from_millis(50);
const LOG_TRUNCATE_THRESHOLD_BYTES: u64 = 1024 * 1024;
#[cfg(not(test))]
const TUI_SUBSCRIPTION_INITIAL_BACKOFF: Duration = Duration::from_millis(250);
#[cfg(test)]
const TUI_SUBSCRIPTION_INITIAL_BACKOFF: Duration = Duration::from_millis(10);
const TUI_SUBSCRIPTION_MAX_BACKOFF: Duration = Duration::from_secs(1);
pub(crate) const NO_AUTO_START_ENV_VAR: &str = "AGENTSCAN_NO_AUTO_START";
const DEEP_CONTROL_MODE_TELEMETRY_ENV_VAR: &str = "AGENTSCAN_DEEP_CONTROL_MODE_TELEMETRY";
static DAEMON_SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

type SubscriberId = u64;
pub(crate) type EncodedDaemonFrame = Arc<[u8]>;

#[derive(Clone)]
struct DaemonRuntimeIdentity {
    pid: u32,
    daemon_start_time: String,
    executable: String,
    executable_canonical: Option<String>,
    socket_path: String,
}

impl DaemonRuntimeIdentity {
    fn new(socket_path: &Path) -> Result<Self> {
        let executable = env::current_exe()
            .context("failed to resolve current executable")?
            .display()
            .to_string();
        let executable_canonical = fs::canonicalize(&executable)
            .ok()
            .map(|path| path.display().to_string());
        Ok(Self {
            pid: std::process::id(),
            daemon_start_time: snapshot::now_rfc3339()?,
            executable,
            executable_canonical,
            socket_path: socket_path.display().to_string(),
        })
    }

    fn frame(&self) -> ipc::DaemonIdentityFrame {
        ipc::DaemonIdentityFrame {
            pid: self.pid,
            daemon_start_time: self.daemon_start_time.clone(),
            executable: self.executable.clone(),
            executable_canonical: self.executable_canonical.clone(),
            socket_path: self.socket_path.clone(),
            protocol_version: ipc::WIRE_PROTOCOL_VERSION,
            snapshot_schema_version: CACHE_SCHEMA_VERSION,
        }
    }

    fn unknown_for_tests() -> Self {
        Self {
            pid: std::process::id(),
            daemon_start_time: "1970-01-01T00:00:00Z".to_string(),
            executable: "unknown".to_string(),
            executable_canonical: None,
            socket_path: "unknown".to_string(),
        }
    }
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
    install_shutdown_signal_handlers();
    DAEMON_SHUTDOWN_REQUESTED.store(false, Ordering::Relaxed);
    let identity = DaemonRuntimeIdentity::new(socket_path)?;
    let lifecycle_paths = LifecyclePaths::from_socket_path(socket_path);
    remove_stale_socket_if_present(socket_path)?;
    let _lifecycle_guard = DaemonLifecycleGuard::acquire(&lifecycle_paths, &identity)?;
    let server = DaemonSocketServer::bind(socket_path)?;
    let socket_state = server.state();
    socket_state.set_identity(identity);
    let server_handle = server.spawn();

    let tmux_version = startup.tmux_version();

    let pending_snapshot = match startup
        .initial_snapshot(tmux_version.as_deref())
        .and_then(PreparedSnapshot::new)
    {
        Ok(pending_snapshot) => pending_snapshot,
        Err(error) => {
            let message = startup_failure_message("initial snapshot", &error);
            socket_state.mark_startup_failed(message.clone());
            std::thread::sleep(STARTUP_FAILURE_OBSERVABILITY_WINDOW);
            drop(server_handle);
            return Err(error.context(message));
        }
    };

    let tmux_client = match startup.start_tmux_control_mode_client() {
        Ok(client) => client,
        Err(error) => {
            let message = startup_failure_message("tmux control-mode startup", &error);
            socket_state.mark_startup_failed(message.clone());
            std::thread::sleep(STARTUP_FAILURE_OBSERVABILITY_WINDOW);
            drop(server_handle);
            return Err(error.context(message));
        }
    };

    let mut closing_guard = DaemonClosingGuard::new(socket_state.clone());
    let mut runtime = DaemonRuntime::from_started(
        startup,
        socket_state,
        tmux_version,
        pending_snapshot,
        tmux_client,
    )?;
    runtime.run(&server_handle)?;

    closing_guard.mark_closing();
    runtime.shutdown_control_mode()?;

    Ok(())
}

struct DaemonRuntime<S> {
    startup: S,
    socket_state: DaemonSocketState,
    tmux_version: Option<String>,
    snapshot: SnapshotEnvelope,
    pane_output_cache: scanner::PaneOutputStatusCache,
    control_mode: RunningTmuxControlModeClient,
    next_reconcile_at: Instant,
    telemetry: RuntimeTelemetry,
    deep_control_mode_telemetry: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct RuntimeTelemetry {
    control_event_refresh_count: u64,
    reconcile_attempt_count: u64,
    reconcile_noop_count: u64,
    reconcile_changed_snapshot_count: u64,
    targeted_refresh_fallback_to_full_count: u64,
}

impl RuntimeTelemetry {
    fn record_control_event_refresh(&mut self) {
        self.control_event_refresh_count = self.control_event_refresh_count.saturating_add(1);
    }

    fn record_targeted_refresh_fallback_to_full(&mut self) {
        self.targeted_refresh_fallback_to_full_count = self
            .targeted_refresh_fallback_to_full_count
            .saturating_add(1);
    }

    fn record_reconcile_result(&mut self, previous: &SnapshotEnvelope, current: &SnapshotEnvelope) {
        self.reconcile_attempt_count = self.reconcile_attempt_count.saturating_add(1);
        if snapshots_are_materially_equal(previous, current) {
            self.reconcile_noop_count = self.reconcile_noop_count.saturating_add(1);
        } else {
            self.reconcile_changed_snapshot_count =
                self.reconcile_changed_snapshot_count.saturating_add(1);
        }
    }

    fn frame(
        &self,
        broker_status: &ipc::ControlModeBrokerStatusFrame,
    ) -> ipc::RuntimeTelemetryFrame {
        ipc::RuntimeTelemetryFrame {
            control_event_refresh_count: self.control_event_refresh_count,
            reconcile_attempt_count: self.reconcile_attempt_count,
            reconcile_noop_count: self.reconcile_noop_count,
            reconcile_changed_snapshot_count: self.reconcile_changed_snapshot_count,
            targeted_refresh_fallback_to_full_count: self.targeted_refresh_fallback_to_full_count,
            broker_fallback_count: broker_status.fallback_count.unwrap_or_default(),
        }
    }
}

enum RefreshRequest<'a> {
    IntervalReconcile,
    TimeoutReconcile,
    ControlModeLines(&'a [String]),
}

struct RefreshOutcome {
    should_exit: bool,
    publish_context: Option<SnapshotPublishContext>,
    reset_reconcile_timer: bool,
}

impl RefreshOutcome {
    fn no_publish() -> Self {
        Self {
            should_exit: false,
            publish_context: None,
            reset_reconcile_timer: false,
        }
    }

    fn no_publish_and_reset_reconcile_timer() -> Self {
        Self {
            should_exit: false,
            publish_context: None,
            reset_reconcile_timer: true,
        }
    }

    fn exit() -> Self {
        Self {
            should_exit: true,
            publish_context: None,
            reset_reconcile_timer: false,
        }
    }

    fn publish(publish_context: SnapshotPublishContext) -> Self {
        Self {
            should_exit: false,
            publish_context: Some(publish_context),
            reset_reconcile_timer: false,
        }
    }

    fn publish_and_reset_reconcile_timer(publish_context: SnapshotPublishContext) -> Self {
        Self {
            should_exit: false,
            publish_context: Some(publish_context),
            reset_reconcile_timer: true,
        }
    }
}

impl<S: StartupActions> DaemonRuntime<S> {
    fn from_started(
        startup: S,
        socket_state: DaemonSocketState,
        tmux_version: Option<String>,
        pending_snapshot: PreparedSnapshot,
        tmux_client: StartedTmuxControlModeClient,
    ) -> Result<Self> {
        let snapshot = pending_snapshot.snapshot.clone();
        socket_state.publish_prepared_snapshot(pending_snapshot);
        let control_mode = RunningTmuxControlModeClient::from_started(tmux_client)?;
        let telemetry = RuntimeTelemetry::default();
        let broker_status = control_mode.broker_status_frame();
        socket_state.update_control_mode_broker_status(broker_status.clone());
        socket_state.update_runtime_telemetry(telemetry.frame(&broker_status));
        Ok(Self {
            startup,
            socket_state,
            tmux_version,
            snapshot,
            pane_output_cache: scanner::PaneOutputStatusCache::new(PANE_OUTPUT_STATUS_CACHE_TTL),
            control_mode,
            next_reconcile_at: Instant::now() + RECONCILE_INTERVAL,
            telemetry,
            deep_control_mode_telemetry: deep_control_mode_telemetry_enabled(),
        })
    }

    fn run(&mut self, server_handle: &DaemonSocketServerHandle) -> Result<()> {
        loop {
            if DAEMON_SHUTDOWN_REQUESTED.load(Ordering::Relaxed) {
                break;
            }
            if !server_handle.socket_still_matches() {
                eprintln!("agentscan: daemon socket path no longer matches this daemon; exiting");
                DAEMON_SHUTDOWN_REQUESTED.store(true, Ordering::Relaxed);
                break;
            }
            if Instant::now() >= self.next_reconcile_at {
                self.apply_refresh_request(RefreshRequest::IntervalReconcile)?;
            }

            let timeout = self.next_control_mode_wait();
            match self.control_mode.recv_timeout(timeout) {
                Ok(line) => {
                    let line = line?;
                    let lines = self.collect_control_mode_batch(line)?;
                    if self.apply_refresh_request(RefreshRequest::ControlModeLines(&lines))? {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.apply_refresh_request(RefreshRequest::TimeoutReconcile)?;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        Ok(())
    }

    fn next_control_mode_wait(&self) -> Duration {
        self.next_reconcile_at
            .saturating_duration_since(Instant::now())
            .max(CONTROL_MODE_MIN_WAIT)
    }

    fn collect_control_mode_batch(&mut self, first_line: String) -> Result<Vec<String>> {
        let mut lines = vec![first_line];
        let deadline = Instant::now() + CONTROL_MODE_EVENT_BATCH_WINDOW;
        loop {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            match self
                .control_mode
                .recv_timeout(deadline.saturating_duration_since(now))
            {
                Ok(line) => lines.push(line?),
                Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {
                    break;
                }
            }
        }
        Ok(lines)
    }

    fn apply_refresh_request(&mut self, request: RefreshRequest<'_>) -> Result<bool> {
        let outcome = match request {
            RefreshRequest::IntervalReconcile => self.apply_reconcile_refresh(
                SnapshotPublishContext::new("reconcile").with_detail("interval"),
            )?,
            RefreshRequest::TimeoutReconcile => self.apply_reconcile_refresh(
                SnapshotPublishContext::new("reconcile").with_detail("timeout"),
            )?,
            RefreshRequest::ControlModeLines(lines) => self.apply_control_mode_refresh(lines)?,
        };
        if let Some(publish_context) = outcome.publish_context {
            self.publish_current_snapshot(publish_context);
        }
        if outcome.reset_reconcile_timer {
            self.next_reconcile_at = Instant::now() + RECONCILE_INTERVAL;
        }
        Ok(outcome.should_exit)
    }

    fn apply_control_mode_refresh(&mut self, lines: &[String]) -> Result<RefreshOutcome> {
        let batch = ControlEventBatch::from_lines(lines);
        let should_exit = batch.should_exit;
        let event_publish_context = batch.publish_context();
        let previous_snapshot = batch
            .can_refresh_full_snapshot()
            .then(|| self.snapshot.clone());
        let mut event_tmux_reads = self.control_mode.read_provider();
        let event_outcome = apply_control_event_batch(
            &mut self.snapshot,
            &mut event_tmux_reads,
            &batch,
            &mut self.pane_output_cache,
        )?;
        if !event_outcome.changed {
            if self.deep_control_mode_telemetry {
                self.update_runtime_telemetry();
            }
            if should_exit {
                return Ok(RefreshOutcome::exit());
            }
            return Ok(RefreshOutcome::no_publish());
        }
        self.telemetry.record_control_event_refresh();
        if event_outcome.full_snapshot_refresh
            && let Some(previous_snapshot) = previous_snapshot
        {
            self.telemetry
                .record_reconcile_result(&previous_snapshot, &self.snapshot);
        }
        if event_outcome.fallback_to_full {
            self.telemetry.record_targeted_refresh_fallback_to_full();
        }

        let reconnected = self.recover_broker_and_reconcile_if_needed()?;
        let mut outcome = RefreshOutcome::publish(if reconnected {
            SnapshotPublishContext::new("reconcile").with_detail("broker_reconnect")
        } else {
            event_publish_context.unwrap_or_else(|| {
                SnapshotPublishContext::new("control_event").with_detail("unknown")
            })
        });
        outcome.should_exit = should_exit;
        Ok(outcome)
    }

    fn apply_reconcile_refresh(
        &mut self,
        publish_context: SnapshotPublishContext,
    ) -> Result<RefreshOutcome> {
        let previous_snapshot = self.snapshot.clone();
        let mut reconcile_tmux_reads = self.control_mode.read_provider();
        reconcile_full_snapshot(
            &mut self.snapshot,
            &mut reconcile_tmux_reads,
            self.tmux_version.as_deref(),
            &mut self.pane_output_cache,
        )?;
        self.telemetry
            .record_reconcile_result(&previous_snapshot, &self.snapshot);
        self.recover_broker_and_reconcile_if_needed()?;
        let outcome =
            reconcile_refresh_outcome(&previous_snapshot, &self.snapshot, publish_context);
        if outcome.publish_context.is_none() {
            self.update_runtime_telemetry();
        }
        Ok(outcome)
    }

    fn recover_broker_and_reconcile_if_needed(&mut self) -> Result<bool> {
        let reconnected = self
            .control_mode
            .recover_broker_if_disabled(&self.startup, &self.socket_state);
        if reconnected {
            let previous_snapshot = self.snapshot.clone();
            let tmux_version = self.snapshot.source.tmux_version.clone();
            let mut reconnect_tmux_reads = self.control_mode.read_provider();
            reconcile_full_snapshot(
                &mut self.snapshot,
                &mut reconnect_tmux_reads,
                tmux_version.as_deref(),
                &mut self.pane_output_cache,
            )?;
            self.telemetry
                .record_reconcile_result(&previous_snapshot, &self.snapshot);
        }
        Ok(reconnected)
    }

    fn publish_current_snapshot(&self, publish_context: SnapshotPublishContext) {
        self.update_runtime_telemetry();
        self.socket_state
            .publish_later_snapshot_with_context(self.snapshot.clone(), publish_context);
    }

    fn update_runtime_telemetry(&self) {
        let broker_status = self.control_mode.broker_status_frame();
        self.socket_state
            .update_control_mode_broker_status(broker_status.clone());
        self.socket_state
            .update_runtime_telemetry(self.telemetry.frame(&broker_status));
    }

    fn shutdown_control_mode(mut self) -> Result<()> {
        if DAEMON_SHUTDOWN_REQUESTED.load(Ordering::Relaxed) {
            self.control_mode.terminate();
        } else {
            self.control_mode.wait_for_exit()?;
        }
        Ok(())
    }
}

pub(crate) trait StartupActions {
    fn tmux_version(&self) -> Option<String>;
    fn initial_snapshot(&self, tmux_version: Option<&str>) -> Result<SnapshotEnvelope>;
    fn start_tmux_control_mode_client(&self) -> Result<StartedTmuxControlModeClient>;
}

#[derive(Default)]
struct DaemonStartup;

impl StartupActions for DaemonStartup {
    fn tmux_version(&self) -> Option<String> {
        tmux::tmux_version()
    }

    fn initial_snapshot(&self, tmux_version: Option<&str>) -> Result<SnapshotEnvelope> {
        snapshot::daemon_snapshot_from_tmux(tmux_version)
    }

    fn start_tmux_control_mode_client(&self) -> Result<StartedTmuxControlModeClient> {
        start_tmux_control_mode_client().map(StartedTmuxControlModeClient::from_real)
    }
}

pub(crate) trait TmuxReadProvider {
    fn list_all_panes(&mut self) -> Result<Vec<TmuxPaneRow>>;
    fn list_target_panes(&mut self, target: &str) -> Result<Option<Vec<TmuxPaneRow>>>;
    fn list_pane(&mut self, pane_id: &str) -> Result<Option<TmuxPaneRow>>;
}

#[derive(Clone, Copy, Debug)]
struct TmuxCommandReadProvider;

impl TmuxReadProvider for TmuxCommandReadProvider {
    fn list_all_panes(&mut self) -> Result<Vec<TmuxPaneRow>> {
        tmux::tmux_list_panes()
    }

    fn list_target_panes(&mut self, target: &str) -> Result<Option<Vec<TmuxPaneRow>>> {
        tmux::tmux_list_panes_target(target)
    }

    fn list_pane(&mut self, pane_id: &str) -> Result<Option<TmuxPaneRow>> {
        tmux::tmux_list_pane(pane_id)
    }
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

fn cleanup_detached_daemon_child(child: &mut std::process::Child) {
    if let Ok(None) = child.try_wait() {
        let _ = unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
        let deadline = Instant::now() + STARTUP_FAILURE_OBSERVABILITY_WINDOW * 5;
        while Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(_)) => {
                    let _ = child.wait();
                    return;
                }
                Ok(None) => std::thread::sleep(LIFECYCLE_POLL_INTERVAL),
                Err(_) => break,
            }
        }
        let _ = child.kill();
    }
    let _ = child.wait();
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ControlEventOutcome {
    changed: bool,
    fallback_to_full: bool,
    full_snapshot_refresh: bool,
}

#[derive(Debug, Default, Eq, PartialEq)]
struct ControlEventBatch {
    should_exit: bool,
    next_sequence: u64,
    resnapshot_sequence: Option<u64>,
    sessions: BTreeMap<String, u64>,
    windows: BTreeMap<String, u64>,
    panes: BTreeMap<String, u64>,
    titles: BTreeMap<String, SequencedTitle>,
}

#[derive(Debug, Eq, PartialEq)]
struct SequencedTitle {
    sequence: u64,
    title: String,
}

impl ControlEventBatch {
    fn from_lines(lines: &[String]) -> Self {
        let mut batch = Self::default();
        for line in lines {
            batch.push(control_event_from_line(line));
        }
        batch
    }

    fn push(&mut self, event: ControlEvent) {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        match event {
            ControlEvent::Exit => self.should_exit = true,
            ControlEvent::Ignored => {}
            ControlEvent::Resnapshot => self.resnapshot_sequence = Some(sequence),
            ControlEvent::SessionChanged(session_id) => {
                self.sessions.insert(session_id, sequence);
            }
            ControlEvent::WindowChanged(window_id) => {
                self.windows.insert(window_id, sequence);
            }
            ControlEvent::PaneChanged(pane_id) => {
                self.panes.insert(pane_id, sequence);
            }
            ControlEvent::TitleChanged { pane_id, title } => {
                self.titles
                    .insert(pane_id, SequencedTitle { sequence, title });
            }
        }
    }

    fn can_refresh_full_snapshot(&self) -> bool {
        self.resnapshot_sequence.is_some() || !self.sessions.is_empty() || !self.windows.is_empty()
    }

    fn publish_context(&self) -> Option<SnapshotPublishContext> {
        if self.resnapshot_sequence.is_some() {
            return ControlEvent::Resnapshot.publish_context();
        }

        let event_count = self.sessions.len()
            + self.windows.len()
            + self.panes.len()
            + self
                .titles
                .keys()
                .filter(|pane_id| !self.panes.contains_key(*pane_id))
                .count();
        if event_count != 1 {
            return (event_count > 1)
                .then(|| SnapshotPublishContext::new("control_event").with_detail("batch"));
        }

        if let Some((session_id, _)) = self.sessions.iter().next() {
            return ControlEvent::SessionChanged(session_id.clone()).publish_context();
        }
        if let Some((window_id, _)) = self.windows.iter().next() {
            return ControlEvent::WindowChanged(window_id.clone()).publish_context();
        }
        if let Some((pane_id, _)) = self.panes.iter().next() {
            return ControlEvent::PaneChanged(pane_id.clone()).publish_context();
        }
        if let Some((pane_id, title)) = self.titles.iter().next() {
            return ControlEvent::TitleChanged {
                pane_id: pane_id.clone(),
                title: title.title.clone(),
            }
            .publish_context();
        }

        None
    }
}

impl ControlEvent {
    fn publish_context(&self) -> Option<SnapshotPublishContext> {
        match self {
            ControlEvent::PaneChanged(pane_id) => Some(
                SnapshotPublishContext::new("control_event").with_detail(format!("pane:{pane_id}")),
            ),
            ControlEvent::TitleChanged { pane_id, .. } => Some(
                SnapshotPublishContext::new("control_event")
                    .with_detail(format!("title:{pane_id}")),
            ),
            ControlEvent::WindowChanged(window_id) => Some(
                SnapshotPublishContext::new("control_event")
                    .with_detail(format!("window:{window_id}")),
            ),
            ControlEvent::SessionChanged(session_id) => Some(
                SnapshotPublishContext::new("control_event")
                    .with_detail(format!("session:{session_id}")),
            ),
            ControlEvent::Resnapshot => {
                Some(SnapshotPublishContext::new("control_event").with_detail("resnapshot"))
            }
            ControlEvent::Exit | ControlEvent::Ignored => None,
        }
    }
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

fn apply_control_event_batch(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    batch: &ControlEventBatch,
    pane_output_cache: &mut scanner::PaneOutputStatusCache,
) -> Result<ControlEventOutcome> {
    let pane_scopes_before_refresh = pane_scopes_by_id(snapshot);
    let mut changed = false;
    let mut fallback_to_full = false;
    let mut full_snapshot_refresh = false;

    if batch.resnapshot_sequence.is_some() {
        let tmux_version = snapshot.source.tmux_version.clone();
        reconcile_full_snapshot(
            snapshot,
            tmux_reads,
            tmux_version.as_deref(),
            pane_output_cache,
        )?;
        changed = true;
        full_snapshot_refresh = true;
    }

    for (session_id, sequence) in &batch.sessions {
        if batch
            .resnapshot_sequence
            .is_some_and(|resnapshot_sequence| *sequence <= resnapshot_sequence)
        {
            continue;
        }
        changed = true;
        if let Err(error) =
            refresh_snapshot_session(snapshot, tmux_reads, session_id, pane_output_cache)
        {
            fallback_to_full_resnapshot(
                snapshot,
                tmux_reads,
                &format!("session:{session_id}"),
                error,
                pane_output_cache,
            )?;
            fallback_to_full = true;
            full_snapshot_refresh = true;
        }
    }

    for (window_id, sequence) in &batch.windows {
        if batch
            .resnapshot_sequence
            .is_some_and(|resnapshot_sequence| *sequence <= resnapshot_sequence)
        {
            continue;
        }
        changed = true;
        if let Err(error) =
            refresh_snapshot_window(snapshot, tmux_reads, window_id, pane_output_cache)
        {
            fallback_to_full_resnapshot(
                snapshot,
                tmux_reads,
                &format!("window:{window_id}"),
                error,
                pane_output_cache,
            )?;
            fallback_to_full = true;
            full_snapshot_refresh = true;
        }
    }

    let pane_scopes_after_scope_refresh = pane_scopes_by_id(snapshot);
    let mut pane_ids: BTreeSet<String> = batch.panes.keys().cloned().collect();
    pane_ids.extend(batch.titles.keys().filter_map(|pane_id| {
        title_override_after_latest_refresh(
            batch,
            &pane_scopes_before_refresh,
            &pane_scopes_after_scope_refresh,
            pane_id,
        )
        .map(|_| pane_id.clone())
    }));
    for pane_id in pane_ids {
        changed = true;
        refresh_snapshot_pane_with_title(
            snapshot,
            tmux_reads,
            &pane_id,
            title_override_after_latest_refresh(
                batch,
                &pane_scopes_before_refresh,
                &pane_scopes_after_scope_refresh,
                &pane_id,
            ),
            pane_output_cache,
        )?;
    }

    Ok(ControlEventOutcome {
        changed,
        fallback_to_full,
        full_snapshot_refresh,
    })
}

fn pane_scopes_by_id(
    snapshot: &SnapshotEnvelope,
) -> HashMap<String, (Option<String>, Option<String>)> {
    snapshot
        .panes
        .iter()
        .map(|pane| {
            (
                pane.pane_id.clone(),
                (pane.tmux.session_id.clone(), pane.tmux.window_id.clone()),
            )
        })
        .collect()
}

fn title_override_after_latest_refresh<'a>(
    batch: &'a ControlEventBatch,
    pane_scopes_before_refresh: &HashMap<String, (Option<String>, Option<String>)>,
    pane_scopes_after_scope_refresh: &HashMap<String, (Option<String>, Option<String>)>,
    pane_id: &str,
) -> Option<&'a str> {
    let title = batch.titles.get(pane_id)?;
    let mut latest_refresh_sequence = batch
        .resnapshot_sequence
        .into_iter()
        .chain(batch.panes.get(pane_id).copied())
        .max();

    for pane_scopes in [
        pane_scopes_before_refresh.get(pane_id),
        pane_scopes_after_scope_refresh.get(pane_id),
    ]
    .into_iter()
    .flatten()
    {
        latest_refresh_sequence =
            latest_refresh_sequence_for_scopes(batch, pane_scopes, latest_refresh_sequence);
    }

    latest_refresh_sequence
        .is_none_or(|latest_refresh_sequence| title.sequence > latest_refresh_sequence)
        .then_some(title.title.as_str())
}

fn latest_refresh_sequence_for_scopes(
    batch: &ControlEventBatch,
    pane_scopes: &(Option<String>, Option<String>),
    latest_refresh_sequence: Option<u64>,
) -> Option<u64> {
    let mut latest_refresh_sequence = latest_refresh_sequence;
    if let Some(sequence) = pane_scopes
        .0
        .as_deref()
        .and_then(|session_id| batch.sessions.get(session_id))
    {
        latest_refresh_sequence = Some(
            latest_refresh_sequence
                .map(|latest| latest.max(*sequence))
                .unwrap_or(*sequence),
        );
    }
    if let Some(sequence) = pane_scopes
        .1
        .as_deref()
        .and_then(|window_id| batch.windows.get(window_id))
    {
        latest_refresh_sequence = Some(
            latest_refresh_sequence
                .map(|latest| latest.max(*sequence))
                .unwrap_or(*sequence),
        );
    }
    latest_refresh_sequence
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
    if !payload_may_contain_terminal_title(payload) {
        return None;
    }
    let decoded = decode_tmux_control_payload(payload);
    terminal_title_from_decoded_output(&decoded)
}

fn payload_may_contain_terminal_title(payload: &str) -> bool {
    payload.contains("\\033]0;")
        || payload.contains("\\033]2;")
        || payload.contains("\u{1b}]0;")
        || payload.contains("\u{1b}]2;")
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

fn refresh_snapshot_pane_with_title(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    pane_id: &str,
    title_override: Option<&str>,
    pane_output_cache: &mut scanner::PaneOutputStatusCache,
) -> Result<()> {
    let pane = tmux_reads.list_pane(pane_id)?.map(|mut row| {
        if let Some(title) = title_override {
            row.pane_title_raw = title.to_string();
        }
        let mut pane = classify::pane_from_row(row);
        let proc_inspector = proc::ProcProcessInspector;
        classify::apply_proc_fallback(&mut pane, &proc_inspector);
        scanner::apply_pane_output_status_fallbacks_with_cache(
            std::slice::from_mut(&mut pane),
            pane_output_cache,
            Instant::now(),
        );
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

    snapshot::sort_snapshot_panes(snapshot);
    snapshot::mark_snapshot_as_daemon(snapshot)
}

fn refresh_snapshot_window(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    window_id: &str,
    pane_output_cache: &mut scanner::PaneOutputStatusCache,
) -> Result<()> {
    refresh_snapshot_scope(
        snapshot,
        tmux_reads,
        TargetScope::Window,
        window_id,
        pane_output_cache,
    )
}

fn refresh_snapshot_session(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    session_id: &str,
    pane_output_cache: &mut scanner::PaneOutputStatusCache,
) -> Result<()> {
    refresh_snapshot_scope(
        snapshot,
        tmux_reads,
        TargetScope::Session,
        session_id,
        pane_output_cache,
    )
}

fn refresh_snapshot_scope(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    scope: TargetScope,
    target_id: &str,
    pane_output_cache: &mut scanner::PaneOutputStatusCache,
) -> Result<()> {
    let rows = tmux_reads.list_target_panes(target_id)?;

    snapshot
        .panes
        .retain(|pane| !scope.matches(pane, target_id));

    if let Some(rows) = rows {
        let proc_inspector = proc::ProcProcessInspector;
        let mut panes = classify::panes_from_rows_with_proc_fallback(rows, &proc_inspector);
        scanner::apply_pane_output_status_fallbacks_with_cache(
            &mut panes,
            pane_output_cache,
            Instant::now(),
        );
        snapshot.panes.extend(panes.into_iter().map(|mut pane| {
            pane.diagnostics.cache_origin = "daemon_update".to_string();
            pane
        }));
    }

    snapshot::sort_snapshot_panes(snapshot);
    snapshot::mark_snapshot_as_daemon(snapshot)
}

fn fallback_to_full_resnapshot(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    event_context: &str,
    error: anyhow::Error,
    pane_output_cache: &mut scanner::PaneOutputStatusCache,
) -> Result<()> {
    eprintln!(
        "agentscan: targeted refresh failed for control-mode event {event_context:?}: {error:#}"
    );
    let tmux_version = snapshot.source.tmux_version.clone();
    reconcile_full_snapshot(
        snapshot,
        tmux_reads,
        tmux_version.as_deref(),
        pane_output_cache,
    )
}

fn reconcile_full_snapshot(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    tmux_version: Option<&str>,
    pane_output_cache: &mut scanner::PaneOutputStatusCache,
) -> Result<()> {
    *snapshot = daemon_snapshot_from_tmux_with_provider(
        tmux_reads,
        tmux_version,
        pane_output_cache,
        Instant::now(),
    )?;
    Ok(())
}

fn reconcile_refresh_outcome(
    previous: &SnapshotEnvelope,
    current: &SnapshotEnvelope,
    publish_context: SnapshotPublishContext,
) -> RefreshOutcome {
    if snapshots_are_materially_equal(previous, current) {
        RefreshOutcome::no_publish_and_reset_reconcile_timer()
    } else {
        RefreshOutcome::publish_and_reset_reconcile_timer(publish_context)
    }
}

fn snapshots_are_materially_equal(left: &SnapshotEnvelope, right: &SnapshotEnvelope) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    normalize_snapshot_for_material_comparison(&mut left);
    normalize_snapshot_for_material_comparison(&mut right);
    left == right
}

fn normalize_snapshot_for_material_comparison(snapshot: &mut SnapshotEnvelope) {
    snapshot.generated_at.clear();
    snapshot.source.daemon_generated_at = None;
    for pane in &mut snapshot.panes {
        pane.diagnostics.cache_origin.clear();
    }
}

fn daemon_snapshot_from_tmux_with_provider(
    tmux_reads: &mut impl TmuxReadProvider,
    tmux_version: Option<&str>,
    pane_output_cache: &mut scanner::PaneOutputStatusCache,
    now: Instant,
) -> Result<SnapshotEnvelope> {
    let rows = tmux_reads.list_all_panes()?;
    let proc_inspector = proc::ProcProcessInspector;
    let mut panes = classify::panes_from_rows_with_proc_fallback(rows, &proc_inspector);
    scanner::apply_pane_output_status_fallbacks_with_cache(&mut panes, pane_output_cache, now);

    let mut snapshot = SnapshotEnvelope {
        schema_version: CACHE_SCHEMA_VERSION,
        generated_at: snapshot::now_rfc3339()?,
        source: SnapshotSource {
            kind: SourceKind::Snapshot,
            tmux_version: tmux_version.map(str::to_string),
            daemon_generated_at: None,
        },
        panes,
    };
    snapshot::sort_snapshot_panes(&mut snapshot);
    for pane in &mut snapshot.panes {
        pane.diagnostics.cache_origin = "daemon_snapshot".to_string();
    }
    snapshot::mark_snapshot_as_daemon(&mut snapshot)?;
    Ok(snapshot)
}

fn deep_control_mode_telemetry_enabled() -> bool {
    env::var_os(DEEP_CONTROL_MODE_TELEMETRY_ENV_VAR)
        .as_deref()
        .is_some_and(deep_control_mode_telemetry_value_enabled)
}

fn deep_control_mode_telemetry_value_enabled(value: &std::ffi::OsStr) -> bool {
    let value = value.to_string_lossy();
    let value = value.trim();
    !value.is_empty()
        && !matches!(
            value.to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        )
}

#[cfg(test)]
pub(crate) fn test_refresh_snapshot_pane_with_provider(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    pane_id: &str,
) -> Result<()> {
    let mut pane_output_cache = scanner::PaneOutputStatusCache::new(PANE_OUTPUT_STATUS_CACHE_TTL);
    refresh_snapshot_pane_with_title(snapshot, tmux_reads, pane_id, None, &mut pane_output_cache)
}

#[cfg(test)]
pub(crate) fn test_apply_resnapshot_control_event_with_provider(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
) -> Result<(bool, bool)> {
    let mut pane_output_cache = scanner::PaneOutputStatusCache::new(PANE_OUTPUT_STATUS_CACHE_TTL);
    let mut batch = ControlEventBatch::default();
    batch.push(ControlEvent::Resnapshot);
    let outcome = apply_control_event_batch(snapshot, tmux_reads, &batch, &mut pane_output_cache)?;
    Ok((outcome.changed, outcome.full_snapshot_refresh))
}

#[cfg(test)]
pub(crate) fn test_apply_control_event_lines_with_provider(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    lines: &[String],
) -> Result<(bool, bool, bool)> {
    let mut pane_output_cache = scanner::PaneOutputStatusCache::new(PANE_OUTPUT_STATUS_CACHE_TTL);
    let batch = ControlEventBatch::from_lines(lines);
    let outcome = apply_control_event_batch(snapshot, tmux_reads, &batch, &mut pane_output_cache)?;
    Ok((
        outcome.changed,
        outcome.full_snapshot_refresh,
        outcome.fallback_to_full,
    ))
}

#[cfg(test)]
pub(crate) fn test_deep_control_mode_telemetry_value_enabled(value: &str) -> bool {
    deep_control_mode_telemetry_value_enabled(std::ffi::OsStr::new(value))
}

#[cfg(test)]
pub(crate) fn test_reconcile_refresh_publish_decision(
    previous: &SnapshotEnvelope,
    current: &SnapshotEnvelope,
) -> (bool, bool) {
    let outcome = reconcile_refresh_outcome(
        previous,
        current,
        SnapshotPublishContext::new("reconcile").with_detail("test"),
    );
    (
        outcome.publish_context.is_some(),
        outcome.reset_reconcile_timer,
    )
}

#[cfg(test)]
pub(crate) fn test_runtime_telemetry_after_reconcile_results(
    previous: &SnapshotEnvelope,
    noop_current: &SnapshotEnvelope,
    changed_current: &SnapshotEnvelope,
) -> ipc::RuntimeTelemetryFrame {
    let mut telemetry = RuntimeTelemetry::default();
    telemetry.record_control_event_refresh();
    telemetry.record_targeted_refresh_fallback_to_full();
    telemetry.record_reconcile_result(previous, noop_current);
    telemetry.record_reconcile_result(noop_current, changed_current);
    telemetry.frame(&ipc::ControlModeBrokerStatusFrame {
        mode: ipc::ControlModeBrokerMode::Fallback,
        disabled_reason: Some("test fallback".to_string()),
        reconnect_count: 1,
        fallback_count: Some(2),
    })
}

#[cfg(test)]
pub(crate) fn test_refresh_snapshot_pane_title_with_provider(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    pane_id: &str,
    title_override: &str,
) -> Result<()> {
    let mut pane_output_cache = scanner::PaneOutputStatusCache::new(PANE_OUTPUT_STATUS_CACHE_TTL);
    refresh_snapshot_pane_with_title(
        snapshot,
        tmux_reads,
        pane_id,
        Some(title_override),
        &mut pane_output_cache,
    )
}

#[cfg(test)]
pub(crate) fn test_refresh_snapshot_window_with_provider(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    window_id: &str,
) -> Result<()> {
    let mut pane_output_cache = scanner::PaneOutputStatusCache::new(PANE_OUTPUT_STATUS_CACHE_TTL);
    refresh_snapshot_window(snapshot, tmux_reads, window_id, &mut pane_output_cache)
}

#[cfg(test)]
pub(crate) fn test_refresh_snapshot_session_with_provider(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    session_id: &str,
) -> Result<()> {
    let mut pane_output_cache = scanner::PaneOutputStatusCache::new(PANE_OUTPUT_STATUS_CACHE_TTL);
    refresh_snapshot_session(snapshot, tmux_reads, session_id, &mut pane_output_cache)
}

#[cfg(test)]
pub(crate) fn test_reconcile_full_snapshot_with_provider(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    tmux_version: Option<&str>,
) -> Result<()> {
    let mut pane_output_cache = scanner::PaneOutputStatusCache::new(PANE_OUTPUT_STATUS_CACHE_TTL);
    reconcile_full_snapshot(snapshot, tmux_reads, tmux_version, &mut pane_output_cache)
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
