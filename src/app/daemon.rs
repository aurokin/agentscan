use super::*;
use std::collections::{HashMap, VecDeque};
use std::fs::File;
// Re-exported to the `daemon::lifecycle` submodules via their `use super::*`.
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
mod events;
mod lifecycle;
mod refresh;
mod runtime;
mod snapshot_store;
mod socket_server;
mod telemetry;

pub(crate) use control_mode::StartedTmuxControlModeClient;
#[cfg(test)]
use control_mode::control_mode_startup_response_from_line;
use control_mode::{
    ControlModeLine, DaemonClosingGuard, install_shutdown_signal_handlers,
    start_subscriber_control_mode_client, start_tmux_control_mode_client_for,
    startup_failure_message,
};
use events::{
    ControlEvent, ControlEventBatch, ControlEventOutcome, control_event_from_line,
    is_control_exit_line,
};
pub(crate) use lifecycle::{
    AutoStartPolicy, DaemonSnapshotError, LifecycleQuery, SubscriptionRowMode, daemon_restart,
    daemon_run, daemon_start, daemon_status, daemon_stop, emit_pane_focus_event_best_effort,
    query_lifecycle_status, snapshot_via_socket, snapshot_via_socket_path_with_start_command,
    spawn_subscription_worker, stream_subscription_events_json,
};
use lifecycle::{DaemonLifecycleGuard, LifecyclePaths, remove_stale_socket_if_present};
#[cfg(target_os = "macos")]
pub(crate) use lifecycle::{MacExecutableAssessment, assess_macos_executable_for_daemon_autostart};
#[cfg(test)]
pub(crate) use lifecycle::{
    daemon_status_with_socket_path, snapshot_via_socket_path, test_daemon_restart_with_steps,
    test_daemon_start_env_removes_from, test_daemon_start_tmux_envs_from,
    test_explicit_macos_daemon_start_preflight, test_implicit_consumer_macos_auto_start_preflight,
    test_macos_executable_assessment_for_outputs, test_macos_start_requires_trust_preflight,
    test_tui_macos_auto_start_preflight, test_write_subscription_keepalive,
};
use refresh::snapshots_are_materially_equal;
use snapshot_store::SnapshotStore;
pub(crate) use socket_server::DaemonSocketState;
use socket_server::bench_encode_snapshot_frame_len;
#[cfg(test)]
pub(crate) use socket_server::{
    DaemonBroadcast, SubscriberMailbox, handle_daemon_socket_client, is_transient_accept_error,
    refuse_server_busy, test_recv_client_event,
};
use socket_server::{DaemonSocketServer, PreparedSnapshot, SnapshotPublishContext};

#[allow(unused_imports)]
pub(crate) use runtime::{DaemonRuntime, RefreshOutcome, RefreshRequest};
use telemetry::ObservabilityDetail;
#[allow(unused_imports)]
pub(crate) use telemetry::{DaemonEventTrace, RefreshObservability, RuntimeTelemetry};

fn client_event_detail(event: &ipc::ClientEventFrame) -> String {
    match event {
        ipc::ClientEventFrame::PaneFocus { pane_id } => format!("pane_focus:{pane_id}"),
    }
}

const CONTROL_MODE_ACTIVE_RECONCILE_INTERVAL: Duration = Duration::from_secs(30);
const CONTROL_MODE_FALLBACK_RECONCILE_INTERVAL: Duration = Duration::from_secs(1);
// When the within-session redundancy reconcile is disabled, the periodic poll is
// reduced to an infrequent self-heal/drift backstop rather than a cross-session
// sweep: per-session subscriber clients now provide event coverage for every
// session, so this `list-panes -a` pass exists only to recover from rare event
// drift (a missed notification or a subscriber that failed to attach). It is not
// responsible for cross-session latency, so it can run rarely.
const CONTROL_MODE_SELF_HEAL_INTERVAL: Duration = Duration::from_secs(300);
const CONTROL_MODE_ACTIVE_RECONCILE_INTERVAL_ENV_VAR: &str =
    "AGENTSCAN_CONTROL_MODE_ACTIVE_RECONCILE_INTERVAL_MS";
const CONTROL_MODE_SELF_HEAL_INTERVAL_ENV_VAR: &str =
    "AGENTSCAN_CONTROL_MODE_SELF_HEAL_INTERVAL_MS";
const TRACE_EVENTS_ENV_VAR: &str = "AGENTSCAN_TRACE_EVENTS";
const TRACE_EVENT_LIMIT_ENV_VAR: &str = "AGENTSCAN_TRACE_EVENT_LIMIT";
const TRACE_CONTROL_LINES_ENV_VAR: &str = "AGENTSCAN_TRACE_CONTROL_LINES";
const DEFAULT_TRACE_EVENT_LIMIT: usize = 1000;
const CONTROL_MODE_EVENT_BATCH_WINDOW: Duration = Duration::from_millis(100);
const CONTROL_MODE_MIN_WAIT: Duration = Duration::from_millis(1);
// Upper bound on the idle run-loop wait. With no settle/reconcile/subscriber-monitor
// deadline pending, this is the only thing that wakes the loop, so it doubles as the
// detection latency for two rare conditions that only surface on a timeout wake:
//   * a primary tmux client that died without emitting `%exit` (e.g. the tmux server
//     was SIGKILLed and the pipe closed at silent EOF) — caught by
//     `primary_child_exited()`; the reader thread does not signal the shared channel on
//     EOF, so this genuinely relies on the poll rather than a channel wake;
//   * a `daemon stop`/SIGTERM whose handler only sets `DAEMON_SHUTDOWN_REQUESTED` and
//     cannot wake the mpsc wait, so shutdown is noticed at most one cap later.
// A clean primary exit (`%exit`) or a forwarded read error wakes the loop immediately,
// so those do not depend on this bound. Kept comfortably under `DAEMON_STOP_TIMEOUT`
// (3s) so a stop is always noticed and torn down before the SIGKILL escalation fires.
// Raised from 500ms to cut fully-idle (no-subscriber) wakeups from ~2/s to ~0.5/s.
const CONTROL_MODE_MAX_WAIT: Duration = Duration::from_secs(2);
// How often the run loop re-stats the socket path to confirm it still owns it. This is
// a slow-drift/self-heal backstop (external socket replacement), so a coarse cadence is
// plenty; running the stat on every wakeup was pure overhead.
const SOCKET_IDENTITY_CHECK_INTERVAL: Duration = Duration::from_secs(5);
const PANE_OUTPUT_STATUS_CACHE_TTL: Duration = Duration::from_secs(2);
// How long after a pane-output provider's last activity event to re-capture it once more.
// `window_activity` ticks drive busy detection while a turn produces output, but an idle
// transition emits no further activity, so a pane classified `Busy` from pane output would
// otherwise stay stuck busy. Each activity-bearing refresh re-arms this deadline; when the
// event stream finally goes quiet (turn ended), the settle pass re-reads the pane once to
// catch the idle frame. Kept slightly above the capture cache TTL so the entry is expired.
const PANE_OUTPUT_SETTLE_DELAY: Duration = Duration::from_millis(2200);
const STARTUP_FAILURE_OBSERVABILITY_WINDOW: Duration = Duration::from_millis(200);
const CONTROL_MODE_STARTUP_TIMEOUT: Duration = Duration::from_secs(2);
const CONTROL_MODE_COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
const CLIENT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);
const CLIENT_WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const SUBSCRIBER_WRITE_TIMEOUT: Duration = Duration::from_millis(250);
// Health backstop for subscriber clients that die without emitting `%exit`
// (event-driven exits are handled immediately). 1s keeps idle wakeups rare —
// the menubar app holds a subscriber open, so this poll runs whenever the
// desktop is up — while the reconcile self-heal bounds the worst case.
const SUBSCRIBER_MONITOR_POLL_INTERVAL: Duration = Duration::from_secs(1);
pub(crate) const MAX_PENDING_HANDSHAKES: usize = 8;
pub(crate) const MAX_SUBSCRIBERS: usize = 64;
// Upper bound on per-session control-mode subscriber clients. Each subscriber is
// a real `tmux -C attach-session` process, so a pathological session count must
// not spawn unbounded clients/fds. Sessions beyond the cap fall back to the
// self-heal reconcile for cross-session coverage instead of an event client.
pub(crate) const MAX_CONTROL_MODE_SUBSCRIBERS: usize = 64;
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
    let runtime_options = config::resolve_runtime_options()?;

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
        runtime_options,
        DaemonEventTrace::from_socket_path(socket_path),
    )?;
    let run_result = runtime.run(&server_handle);

    // Mark closing before control-mode teardown on both exits, so clients see
    // the closing state rather than an abruptly dropped socket. The teardown
    // itself must differ by exit: after an error the tmux server is usually
    // still alive, so the graceful path's `wait_for_exit` would block forever
    // on a healthy client — kill the control-mode children instead (what the
    // pre-error `Drop` did) and surface the run error.
    closing_guard.mark_closing();
    match run_result {
        Ok(()) => runtime.shutdown_control_mode(),
        Err(error) => {
            runtime.terminate_control_mode();
            Err(error)
        }
    }
}

pub(crate) trait StartupActions {
    fn tmux_version(&self) -> Option<String>;
    fn initial_snapshot(&self, tmux_version: Option<&str>) -> Result<SnapshotEnvelope>;
    fn start_tmux_control_mode_client(&self) -> Result<StartedTmuxControlModeClient>;

    fn primary_session_id_for_status(&self) -> Option<String> {
        None
    }

    // Sessions other than the primary's that should get an event-only subscriber
    // client. Defaults to none so test startups stay single-session unless they
    // opt in.
    fn additional_subscriber_session_ids(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    fn start_subscriber_client(&self, session_id: &str) -> Result<StartedTmuxControlModeClient> {
        let _ = session_id;
        bail!("subscriber control clients are not supported by this startup")
    }
}

#[derive(Default)]
struct DaemonStartup {
    // The session the primary control client attaches to, resolved once (lazily)
    // and cached. Caching it (rather than recomputing `default_session_target()`
    // each reconcile) keeps the primary attach and the subscriber-exclusion set in
    // agreement: `default_session_target()` follows the launching tmux client's
    // current session and would drift if that client switched sessions, which
    // could leave the switched-to session with no event client. Resolution is
    // lazy so it happens on the first `start_tmux_control_mode_client` call, which
    // runs inside the daemon startup-failure reporting region — a resolution error
    // then surfaces as an observable `startup_failed` status, not a silent exit.
    primary_session_id: std::sync::OnceLock<String>,
}

impl DaemonStartup {
    fn primary_session_id(&self) -> Result<&str> {
        if let Some(session_id) = self.primary_session_id.get() {
            return Ok(session_id.as_str());
        }
        let session_id = tmux::default_session_target()?;
        let _ = self.primary_session_id.set(session_id);
        Ok(self
            .primary_session_id
            .get()
            .expect("primary session id was just set")
            .as_str())
    }
}

impl StartupActions for DaemonStartup {
    fn tmux_version(&self) -> Option<String> {
        tmux::tmux_version()
    }

    fn initial_snapshot(&self, tmux_version: Option<&str>) -> Result<SnapshotEnvelope> {
        snapshot::daemon_snapshot_from_tmux(tmux_version)
    }

    fn start_tmux_control_mode_client(&self) -> Result<StartedTmuxControlModeClient> {
        start_tmux_control_mode_client_for(self.primary_session_id()?)
            .map(StartedTmuxControlModeClient::from_real)
    }

    fn primary_session_id_for_status(&self) -> Option<String> {
        self.primary_session_id().ok().map(str::to_string)
    }

    fn additional_subscriber_session_ids(&self) -> Result<Vec<String>> {
        let primary = self.primary_session_id()?;
        Ok(tmux::list_session_ids()?
            .into_iter()
            .filter(|session_id| session_id.as_str() != primary)
            .collect())
    }

    fn start_subscriber_client(&self, session_id: &str) -> Result<StartedTmuxControlModeClient> {
        start_subscriber_control_mode_client(session_id)
            .map(StartedTmuxControlModeClient::from_real)
    }
}

pub(crate) trait TmuxReadProvider {
    fn list_all_panes(&mut self) -> Result<Vec<TmuxPaneRow>>;
    fn list_target_panes(
        &mut self,
        scope: tmux::PaneListScope,
        target: &str,
    ) -> Result<Option<Vec<TmuxPaneRow>>>;
    fn list_pane(&mut self, pane_id: &str) -> Result<Option<TmuxPaneRow>>;
}

#[derive(Clone, Copy, Debug)]
struct TmuxCommandReadProvider;

impl TmuxReadProvider for TmuxCommandReadProvider {
    fn list_all_panes(&mut self) -> Result<Vec<TmuxPaneRow>> {
        tmux::tmux_list_panes()
    }

    fn list_target_panes(
        &mut self,
        scope: tmux::PaneListScope,
        target: &str,
    ) -> Result<Option<Vec<TmuxPaneRow>>> {
        tmux::tmux_list_panes_target(scope, target)
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

fn deep_control_mode_telemetry_enabled() -> bool {
    env_value_enabled(DEEP_CONTROL_MODE_TELEMETRY_ENV_VAR)
}

fn trace_control_lines_enabled() -> bool {
    env_value_enabled(TRACE_CONTROL_LINES_ENV_VAR)
}

fn control_mode_active_reconcile_interval() -> Duration {
    env::var(CONTROL_MODE_ACTIVE_RECONCILE_INTERVAL_ENV_VAR)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|millis| *millis > 0)
        .map(Duration::from_millis)
        .unwrap_or(CONTROL_MODE_ACTIVE_RECONCILE_INTERVAL)
}

fn control_mode_self_heal_interval() -> Duration {
    env::var(CONTROL_MODE_SELF_HEAL_INTERVAL_ENV_VAR)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|millis| *millis > 0)
        .map(Duration::from_millis)
        .unwrap_or(CONTROL_MODE_SELF_HEAL_INTERVAL)
}

#[cfg_attr(not(test), allow(dead_code))]
fn deep_control_mode_telemetry_value_enabled(value: &std::ffi::OsStr) -> bool {
    env_os_value_enabled(value)
}

fn env_value_enabled(name: &str) -> bool {
    env::var_os(name)
        .as_deref()
        .is_some_and(env_os_value_enabled)
}

fn env_os_value_enabled(value: &std::ffi::OsStr) -> bool {
    let value = value.to_string_lossy();
    let value = value.trim();
    !value.is_empty()
        && !matches!(
            value.to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        )
}

pub(super) fn duration_millis_u64(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

fn duration_until_millis(deadline: Instant) -> u64 {
    duration_millis_u64(deadline.saturating_duration_since(Instant::now()))
}

#[cfg(test)]
fn run_deep_control_mode_telemetry_value_enabled(value: &str) -> bool {
    deep_control_mode_telemetry_value_enabled(std::ffi::OsStr::new(value))
}

/// Parses a control-mode batch and folds its volume counters together. Used by the
/// `core_paths` benchmark to track per-batch parse cost on `%output` firehose bursts;
/// the fold keeps the parse from being optimized away.
#[doc(hidden)]
pub(crate) fn bench_control_event_batch_volume(lines: &[String]) -> u64 {
    let batch = ControlEventBatch::from_lines(lines);
    batch.total_line_count ^ batch.output_line_count ^ batch.output_byte_count ^ batch.ignored_count
}

pub(crate) fn bench_snapshots_are_materially_equal(
    left: &SnapshotEnvelope,
    right: &SnapshotEnvelope,
) -> bool {
    snapshots_are_materially_equal(left, right)
}

pub(crate) fn bench_encode_snapshot_frame_bytes(snapshot: &SnapshotEnvelope) -> Result<usize> {
    bench_encode_snapshot_frame_len(snapshot)
}

pub(crate) fn bench_encode_diff_frame_bytes(
    previous: &SnapshotEnvelope,
    current: &SnapshotEnvelope,
) -> Result<usize> {
    socket_server::bench_encode_diff_frame_len(previous, current)
}

#[cfg(test)]
fn run_snapshot_observability(snapshot: &SnapshotEnvelope) -> ipc::SnapshotObservabilityFrame {
    snapshot_store::snapshot_observability(snapshot)
}

#[cfg(test)]
mod migrated_tests {
    use super::*;

    pub(super) fn empty_socket_snapshot(generated_at: &str) -> SnapshotEnvelope {
        SnapshotEnvelope {
            schema_version: CACHE_SCHEMA_VERSION,
            generated_at: generated_at.to_string(),
            source: SnapshotSource {
                kind: SourceKind::Daemon,
                tmux_version: Some("3.4".to_string()),
                daemon_generated_at: Some(generated_at.to_string()),
            },
            panes: Vec::new(),
        }
    }

    fn daemon_refresh_row(
        pane_id: &str,
        session_id: &str,
        window_id: &str,
        pane_index: u32,
        title: &str,
    ) -> TmuxPaneRow {
        let window_index = window_id
            .trim_start_matches('@')
            .parse::<u32>()
            .expect("window id should be numeric");
        TmuxPaneRow {
            session_name: format!("session-{session_id}"),
            window_index,
            pane_index,
            pane_id: pane_id.to_string(),
            pane_pid: 42_000 + pane_index,
            pane_current_command: "codex".to_string(),
            pane_title_raw: title.to_string(),
            pane_tty: format!("/dev/ttys{pane_index}"),
            pane_current_path: "/tmp/agentscan".to_string(),
            window_name: format!("window-{window_id}"),
            session_id: Some(session_id.to_string()),
            window_id: Some(window_id.to_string()),
            agent_provider: None,
            agent_label: None,
            agent_cwd: None,
            agent_state: None,
            agent_session_id: None,
            agent_pid: None,
            agent_version: None,
            agent_model: None,
            pane_active: false,
            window_active: false,
        }
    }

    fn daemon_refresh_snapshot(rows: Vec<TmuxPaneRow>) -> SnapshotEnvelope {
        let mut snapshot = empty_socket_snapshot("2026-05-03T00:00:00Z");
        snapshot.panes = rows.into_iter().map(classify::pane_from_row).collect();
        snapshot::sort_snapshot_panes(&mut snapshot);
        snapshot
    }
    #[test]
    fn daemon_deep_control_mode_telemetry_env_value_parser() {
        assert!(super::run_deep_control_mode_telemetry_value_enabled("1"));
        assert!(super::run_deep_control_mode_telemetry_value_enabled("true"));
        assert!(super::run_deep_control_mode_telemetry_value_enabled(
            " yes "
        ));
        assert!(!super::run_deep_control_mode_telemetry_value_enabled(""));
        assert!(!super::run_deep_control_mode_telemetry_value_enabled("0"));
        assert!(!super::run_deep_control_mode_telemetry_value_enabled(
            "false"
        ));
        assert!(!super::run_deep_control_mode_telemetry_value_enabled("off"));
    }

    #[test]
    fn snapshot_observability_breaks_down_paths_per_provider() {
        // Two command-classified codex panes plus one wholly unclassified pane.
        let mut unknown_row = daemon_refresh_row("%3", "$1", "@1", 2, "scratch");
        unknown_row.pane_current_command = "bash".to_string();
        unknown_row.pane_tty = "not a tty".to_string();
        let snapshot = daemon_refresh_snapshot(vec![
            daemon_refresh_row("%1", "$1", "@1", 0, "codex"),
            daemon_refresh_row("%2", "$1", "@1", 1, "codex"),
            unknown_row,
        ]);

        let observability = super::run_snapshot_observability(&snapshot);

        let codex = observability
            .per_provider
            .get("codex")
            .expect("codex bucket should be present");
        assert_eq!(codex.pane_count, 2);
        assert_eq!(codex.matched_pane_current_command_count, 2);
        assert_eq!(codex.matched_proc_process_tree_count, 0);

        let unknown = observability
            .per_provider
            .get("unknown")
            .expect("unclassified panes bucket under `unknown`");
        assert_eq!(unknown.pane_count, 1);
        assert_eq!(unknown.matched_pane_current_command_count, 0);

        // Per-provider pane counts reconcile with the snapshot total.
        let bucketed: usize = observability
            .per_provider
            .values()
            .map(|stats| stats.pane_count)
            .sum();
        assert_eq!(bucketed, snapshot.panes.len());
    }

    #[test]
    fn daemon_subscription_format_includes_wrapper_metadata_fields() {
        // Single-brace `#{...}` directives: the string is sent to tmux verbatim, so
        // doubled braces would render every field as a literal `}` (see the constant's
        // doc comment). These assertions guard against regressing back to that.
        assert!(DAEMON_SUBSCRIPTION_FORMAT.contains("#{pane_current_command}"));
        assert!(DAEMON_SUBSCRIPTION_FORMAT.contains("#{pane_title}"));
        assert!(DAEMON_SUBSCRIPTION_FORMAT.contains("#{@agent.provider}"));
        assert!(DAEMON_SUBSCRIPTION_FORMAT.contains("#{@agent.state}"));
        assert!(DAEMON_SUBSCRIPTION_FORMAT.contains("#{@agent.session_id}"));
        assert!(!DAEMON_SUBSCRIPTION_FORMAT.contains("#{window_activity}"));
        assert!(DAEMON_ACTIVITY_SUBSCRIPTION_FORMAT.contains("#{window_activity}"));
        assert!(DAEMON_ACTIVITY_SUBSCRIPTION_FORMAT.starts_with("agentscan-activity:%*:"));
        // Explicitly reject the doubled-brace form that broke the subscription.
        assert!(!DAEMON_SUBSCRIPTION_FORMAT.contains("#{{"));
    }
}
