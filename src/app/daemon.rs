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
    test_collect_control_mode_command_response,
    test_drain_control_mode_channel_clears_stale_frames,
    test_drain_control_mode_channel_preserves_subscriber_frames,
    test_recent_dead_subscriber_tombstone_persists_without_new_dead,
    test_reconnect_preserves_deferred_lines, test_subscriber_local_exit,
    test_subscriber_status_drops_recovered_dead_tombstone,
};
use control_mode::{
    ControlModeLine, DaemonClosingGuard, RunningTmuxControlModeClient, SubscriberAttachOutcome,
    SubscriberReconcileOutcome, install_shutdown_signal_handlers,
    start_subscriber_control_mode_client, start_tmux_control_mode_client_for,
    startup_failure_message,
};
use events::{
    ControlEvent, ControlEventBatch, ControlEventOutcome, batch_changed_session_set,
    control_event_from_line, is_control_exit_line,
};
// Test-only parser helpers stay behind this gate; production builds must not
// expose or resolve the `#[cfg(test)]` definitions in the child modules.
#[cfg(test)]
pub(crate) use events::{
    notification_name, output_title_change_pane_id, output_title_change_title,
    session_notification_target, should_resnapshot_from_notification, subscription_changed_pane_id,
    test_control_event_pane_kind, window_notification_target,
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
use refresh::{
    apply_control_event_batch, reconcile_full_snapshot, reconcile_refresh_outcome,
    refresh_snapshot_for_focused_pane, refresh_snapshot_pane_with_title, snapshot_diff,
    snapshots_are_materially_equal,
};
#[cfg(test)]
pub(crate) use refresh::{
    test_apply_control_event_lines_with_provider,
    test_apply_control_event_lines_with_provider_counts,
    test_apply_resnapshot_control_event_with_provider, test_reconcile_full_snapshot_with_provider,
    test_recover_targeted_pane_provider_with_inspector,
    test_refresh_snapshot_for_focused_pane_with_provider,
    test_refresh_snapshot_pane_title_with_provider, test_refresh_snapshot_pane_with_provider,
    test_refresh_snapshot_session_with_inspector, test_refresh_snapshot_session_with_provider,
    test_refresh_snapshot_window_with_provider,
};
use snapshot_store::SnapshotStore;
pub(crate) use socket_server::DaemonSocketState;
use socket_server::bench_encode_snapshot_frame_len;
#[cfg(test)]
pub(crate) use socket_server::{
    DaemonBroadcast, SubscriberMailbox, handle_daemon_socket_client, is_transient_accept_error,
    refuse_server_busy, test_recv_client_event,
};
use socket_server::{
    DaemonSocketServer, DaemonSocketServerHandle, PreparedSnapshot, SnapshotPublishContext,
};

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

struct DaemonRuntime<S> {
    startup: S,
    socket_state: DaemonSocketState,
    tmux_version: Option<String>,
    snapshot: SnapshotEnvelope,
    pane_output_cache: scanner::PaneOutputStatusCache,
    control_mode: RunningTmuxControlModeClient,
    next_reconcile_at: Instant,
    next_subscriber_monitor_at: Option<Instant>,
    // When set, a pane-output provider is believed busy and the daemon should re-read it once
    // the event stream goes quiet, to catch the idle transition (which emits no event).
    settle_recapture_at: Option<Instant>,
    telemetry: RuntimeTelemetry,
    deep_control_mode_telemetry: bool,
    disable_reconcile: bool,
    disable_proc_fallback: bool,
    event_trace: Option<DaemonEventTrace>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct RuntimeTelemetry {
    control_event_refresh_count: u64,
    control_event_batch_count: u64,
    control_event_line_count: u64,
    control_event_output_line_count: u64,
    control_event_output_byte_count: u64,
    control_event_pane_count: u64,
    control_event_title_count: u64,
    control_event_window_count: u64,
    control_event_session_count: u64,
    control_event_resnapshot_count: u64,
    control_event_ignored_count: u64,
    reconcile_attempt_count: u64,
    reconcile_noop_count: u64,
    reconcile_changed_snapshot_count: u64,
    targeted_title_update_count: u64,
    targeted_pane_refresh_count: u64,
    targeted_scope_refresh_count: u64,
    full_snapshot_refresh_count: u64,
    targeted_refresh_fallback_to_full_count: u64,
    subscriber_monitor_count: u64,
    subscriber_start_count: u64,
    subscriber_reattach_count: u64,
    subscriber_attach_failure_count: u64,
    subscriber_exit_count: u64,
}

impl RuntimeTelemetry {
    // Always-on, integer-only volume accounting. This runs for every control-mode
    // batch (including ignored-only `%output` firehose bursts) so the firehose is
    // measurable without enabling deep telemetry. Cost is a handful of saturating
    // integer adds and no allocation.
    fn record_control_event_volume(&mut self, batch: &ControlEventBatch) {
        self.control_event_batch_count = self.control_event_batch_count.saturating_add(1);
        self.control_event_line_count = self
            .control_event_line_count
            .saturating_add(batch.total_line_count);
        self.control_event_output_line_count = self
            .control_event_output_line_count
            .saturating_add(batch.output_line_count);
        self.control_event_output_byte_count = self
            .control_event_output_byte_count
            .saturating_add(batch.output_byte_count);
        self.control_event_ignored_count = self
            .control_event_ignored_count
            .saturating_add(batch.ignored_count);
    }

    fn record_control_event_kinds(&mut self, batch: &ControlEventBatch) {
        self.control_event_pane_count = self.control_event_pane_count.saturating_add(
            batch
                .panes
                .len()
                .saturating_add(batch.activities.len())
                .try_into()
                .unwrap_or(u64::MAX),
        );
        self.control_event_title_count = self
            .control_event_title_count
            .saturating_add(batch.titles.len().try_into().unwrap_or(u64::MAX));
        self.control_event_window_count = self
            .control_event_window_count
            .saturating_add(batch.windows.len().try_into().unwrap_or(u64::MAX));
        self.control_event_session_count = self
            .control_event_session_count
            .saturating_add(batch.sessions.len().try_into().unwrap_or(u64::MAX));
        if batch.resnapshot_sequence.is_some() {
            self.control_event_resnapshot_count =
                self.control_event_resnapshot_count.saturating_add(1);
        }
    }

    fn record_control_event_refresh(&mut self, outcome: &ControlEventOutcome) {
        self.control_event_refresh_count = self.control_event_refresh_count.saturating_add(1);
        self.targeted_title_update_count = self
            .targeted_title_update_count
            .saturating_add(outcome.targeted_title_updates);
        self.targeted_pane_refresh_count = self
            .targeted_pane_refresh_count
            .saturating_add(outcome.targeted_pane_refreshes);
        self.targeted_scope_refresh_count = self
            .targeted_scope_refresh_count
            .saturating_add(outcome.targeted_scope_refreshes);
        if outcome.full_snapshot_refresh {
            self.full_snapshot_refresh_count = self.full_snapshot_refresh_count.saturating_add(1);
        }
    }

    fn record_targeted_refresh_fallback_to_full(&mut self) {
        self.targeted_refresh_fallback_to_full_count = self
            .targeted_refresh_fallback_to_full_count
            .saturating_add(1);
    }

    fn record_subscriber_monitor(&mut self) {
        self.subscriber_monitor_count = self.subscriber_monitor_count.saturating_add(1);
    }

    fn record_subscriber_reconcile(&mut self, outcome: SubscriberReconcileOutcome) {
        self.subscriber_start_count = self
            .subscriber_start_count
            .saturating_add(outcome.started_count);
        self.subscriber_reattach_count = self
            .subscriber_reattach_count
            .saturating_add(outcome.reattached_count);
        self.subscriber_attach_failure_count = self
            .subscriber_attach_failure_count
            .saturating_add(outcome.attach_failure_count);
        self.subscriber_exit_count = self
            .subscriber_exit_count
            .saturating_add(outcome.pruned_dead_count);
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
        capture_stats: scanner::PaneOutputCaptureStats,
    ) -> ipc::RuntimeTelemetryFrame {
        ipc::RuntimeTelemetryFrame {
            control_event_refresh_count: self.control_event_refresh_count,
            control_event_batch_count: self.control_event_batch_count,
            control_event_line_count: self.control_event_line_count,
            control_event_output_line_count: self.control_event_output_line_count,
            control_event_output_byte_count: self.control_event_output_byte_count,
            control_event_pane_count: self.control_event_pane_count,
            control_event_title_count: self.control_event_title_count,
            control_event_window_count: self.control_event_window_count,
            control_event_session_count: self.control_event_session_count,
            control_event_resnapshot_count: self.control_event_resnapshot_count,
            control_event_ignored_count: self.control_event_ignored_count,
            reconcile_attempt_count: self.reconcile_attempt_count,
            reconcile_noop_count: self.reconcile_noop_count,
            reconcile_changed_snapshot_count: self.reconcile_changed_snapshot_count,
            targeted_title_update_count: self.targeted_title_update_count,
            targeted_pane_refresh_count: self.targeted_pane_refresh_count,
            targeted_scope_refresh_count: self.targeted_scope_refresh_count,
            full_snapshot_refresh_count: self.full_snapshot_refresh_count,
            targeted_refresh_fallback_to_full_count: self.targeted_refresh_fallback_to_full_count,
            subscriber_monitor_count: Some(self.subscriber_monitor_count),
            subscriber_start_count: Some(self.subscriber_start_count),
            subscriber_reattach_count: Some(self.subscriber_reattach_count),
            subscriber_attach_failure_count: Some(self.subscriber_attach_failure_count),
            subscriber_exit_count: Some(self.subscriber_exit_count),
            broker_fallback_count: broker_status.fallback_count.unwrap_or_default(),
            pane_output_capture_attempt_count: capture_stats.attempt_count,
            pane_output_capture_hit_count: capture_stats.hit_count,
            pane_output_capture_error_count: capture_stats.error_count,
        }
    }
}

struct DaemonEventTrace {
    path: PathBuf,
    limit: usize,
    written_since_truncate: usize,
    // The trace is written by the single daemon loop, so the open handle is held
    // across events and only reopened on rotation. Sequential writes to a
    // `File::create` handle advance the cursor, so this appends without O_APPEND.
    file: File,
}

impl DaemonEventTrace {
    fn from_socket_path(socket_path: &Path) -> Option<Self> {
        if !env_value_enabled(TRACE_EVENTS_ENV_VAR) {
            return None;
        }
        let path = socket_path.with_extension("sock.events.jsonl");
        let file = match File::create(&path) {
            Ok(file) => file,
            Err(error) => {
                eprintln!(
                    "agentscan: failed to initialize daemon event trace {}: {error}",
                    path.display()
                );
                return None;
            }
        };
        Some(Self {
            path,
            limit: env::var(TRACE_EVENT_LIMIT_ENV_VAR)
                .ok()
                .and_then(|value| value.trim().parse().ok())
                .filter(|limit| *limit > 0)
                .unwrap_or(DEFAULT_TRACE_EVENT_LIMIT),
            written_since_truncate: 0,
            file,
        })
    }

    fn write(&mut self, event: &ipc::DaemonObservabilityEventFrame) {
        if self.written_since_truncate >= self.limit {
            match File::create(&self.path) {
                Ok(file) => self.file = file,
                Err(error) => {
                    eprintln!(
                        "agentscan: failed to rotate daemon event trace {}: {error}",
                        self.path.display()
                    );
                    return;
                }
            }
            self.written_since_truncate = 0;
        }

        if serde_json::to_writer(&mut self.file, event).is_ok() && writeln!(self.file).is_ok() {
            self.written_since_truncate = self.written_since_truncate.saturating_add(1);
        }
    }
}

enum RefreshRequest<'a> {
    IntervalReconcile,
    TimeoutReconcile,
    ControlModeLines(&'a [ControlModeLine]),
    ClientEvent(&'a ipc::ClientEventFrame),
    SettleRecapture,
}

struct RefreshOutcome {
    should_exit: bool,
    publish_context: Option<SnapshotPublishContext>,
    reset_reconcile_timer: bool,
}

struct RefreshObservability {
    source: &'static str,
    detail: ObservabilityDetail,
    refresh: &'static str,
    should_record: bool,
    control_sources: Vec<ipc::ControlModeSourceFrame>,
    control_lines: Vec<String>,
}

/// Deferred observability detail. Building the human-readable detail string is
/// delayed until an event is actually recorded, so ignored-only `%output`
/// firehose batches (which carry only `Ignored`) never pay the allocation.
enum ObservabilityDetail {
    None,
    Static(&'static str),
    Owned(String),
    Ignored(u64),
}

impl ObservabilityDetail {
    fn into_detail(self) -> Option<String> {
        match self {
            ObservabilityDetail::None => None,
            ObservabilityDetail::Static(detail) => Some(detail.to_string()),
            ObservabilityDetail::Owned(detail) => Some(detail),
            ObservabilityDetail::Ignored(count) => Some(format!("ignored:{count}")),
        }
    }
}

fn client_event_detail(event: &ipc::ClientEventFrame) -> String {
    match event {
        ipc::ClientEventFrame::PaneFocus { pane_id } => format!("pane_focus:{pane_id}"),
    }
}

impl RefreshObservability {
    fn from_request(request: &RefreshRequest<'_>) -> Self {
        match request {
            RefreshRequest::IntervalReconcile => Self {
                source: "reconcile",
                detail: ObservabilityDetail::Static("interval"),
                refresh: "full_snapshot",
                should_record: true,
                control_sources: Vec::new(),
                control_lines: Vec::new(),
            },
            RefreshRequest::TimeoutReconcile => Self {
                source: "reconcile",
                detail: ObservabilityDetail::Static("timeout"),
                refresh: "full_snapshot",
                should_record: true,
                control_sources: Vec::new(),
                control_lines: Vec::new(),
            },
            RefreshRequest::ControlModeLines(lines) => {
                let batch = ControlEventBatch::from_control_lines(lines);
                let control_lines = if trace_control_lines_enabled() {
                    lines.iter().map(|frame| frame.line.clone()).collect()
                } else {
                    Vec::new()
                };
                Self {
                    source: "control_event",
                    detail: batch.observability_detail(),
                    refresh: batch.observability_refresh(),
                    should_record: batch.has_telemetry_event() || !control_lines.is_empty(),
                    control_sources: batch.control_sources,
                    control_lines,
                }
            }
            RefreshRequest::ClientEvent(event) => Self {
                source: "client_event",
                detail: ObservabilityDetail::Owned(client_event_detail(event)),
                refresh: "full_snapshot",
                should_record: true,
                control_sources: Vec::new(),
                control_lines: Vec::new(),
            },
            RefreshRequest::SettleRecapture => Self {
                source: "pane_output_settle",
                detail: ObservabilityDetail::Static("busy_recheck"),
                refresh: "targeted_pane",
                should_record: true,
                control_sources: Vec::new(),
                control_lines: Vec::new(),
            },
        }
    }

    fn should_capture_snapshot_diff(&self) -> bool {
        self.refresh != "none"
    }
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

// Reconcile the set of event-only subscriber clients against the live sessions:
// attach a subscriber for every non-primary session that lacks one and drop
// subscribers whose sessions have closed. Run at startup and on every
// `%sessions-changed`, so sessions created or destroyed at runtime get event
// coverage without relying on the periodic reconcile. Best-effort: failures are
// logged and skipped (the primary session is always covered by the primary
// client, and a failed subscriber falls back to self-heal reconcile latency).
// Bound the subscriber set so a pathological session count cannot spawn an
// unbounded number of `tmux -C` clients. The selection is deterministic (sorted)
// so the same sessions keep their clients across reconciles instead of churning;
// the dropped remainder relies on the self-heal reconcile for cross-session
// coverage.
// Numeric ordering key for a tmux session id (`$12` -> 12). Ids that do not fit
// the `$<number>` shape sort last (by `u64::MAX`) and then fall back to the
// lexical tiebreak in the caller, so selection stays deterministic.
fn subscriber_session_sort_key(session_id: &str) -> u64 {
    session_id
        .strip_prefix('$')
        .and_then(|digits| digits.parse::<u64>().ok())
        .unwrap_or(u64::MAX)
}

fn capped_subscriber_session_ids(mut session_ids: Vec<String>) -> Vec<String> {
    if session_ids.len() > MAX_CONTROL_MODE_SUBSCRIBERS {
        // Sort by numeric session index, not lexically: tmux session ids are
        // unpadded (`$2` sorts after `$19` as strings), so a plain string sort
        // would mis-select which sessions keep their event clients. Keeping the
        // lowest indices is deterministic and stable across reconciles.
        session_ids.sort_by_key(|id| (subscriber_session_sort_key(id), id.clone()));
        eprintln!(
            "agentscan: {} non-primary sessions exceed the subscriber cap ({}); \
             {} sessions fall back to the self-heal reconcile for cross-session coverage",
            session_ids.len(),
            MAX_CONTROL_MODE_SUBSCRIBERS,
            session_ids.len() - MAX_CONTROL_MODE_SUBSCRIBERS,
        );
        session_ids.truncate(MAX_CONTROL_MODE_SUBSCRIBERS);
    }
    session_ids
}

fn reconcile_subscribers<S: StartupActions>(
    startup: &S,
    control_mode: &mut RunningTmuxControlModeClient,
) -> SubscriberReconcileOutcome {
    // Drop subscribers whose client process died so the loop below re-attaches
    // them; a closed session is handled separately by retain (it leaves the set).
    let mut outcome = SubscriberReconcileOutcome {
        pruned_dead_count: control_mode
            .prune_dead_subscribers()
            .try_into()
            .unwrap_or(u64::MAX),
        ..Default::default()
    };
    let desired_session_ids = match startup.additional_subscriber_session_ids() {
        Ok(session_ids) => session_ids,
        Err(error) => {
            eprintln!(
                "agentscan: failed to enumerate sessions for subscriber clients; \
                 keeping the active reconcile until coverage is re-established: {error:#}"
            );
            // We could not verify subscriber coverage this pass (and may have
            // started none yet, e.g. at startup where the flag defaults to true),
            // so mark coverage incomplete to keep the active reconcile poll rather
            // than relaxing to the self-heal backstop. A later reconcile retries.
            control_mode.set_subscriber_coverage(Vec::new(), Vec::new(), false);
            return outcome;
        }
    };
    let under_cap = desired_session_ids.len() <= MAX_CONTROL_MODE_SUBSCRIBERS;
    let capped_session_ids = capped_subscriber_session_ids(desired_session_ids.clone());
    control_mode.retain_subscriber_sessions(&capped_session_ids);
    for session_id in &capped_session_ids {
        if control_mode.has_subscriber(session_id) {
            continue;
        }
        match startup.start_subscriber_client(session_id) {
            Ok(started) => match control_mode.attach_subscriber(session_id.clone(), started) {
                Ok(SubscriberAttachOutcome::AlreadyPresent) => {}
                Ok(SubscriberAttachOutcome::Attached { reattached }) => {
                    outcome.started_count = outcome.started_count.saturating_add(1);
                    if reattached {
                        outcome.reattached_count = outcome.reattached_count.saturating_add(1);
                    }
                }
                Err(error) => {
                    outcome.attach_failure_count = outcome.attach_failure_count.saturating_add(1);
                    eprintln!(
                        "agentscan: failed to attach subscriber client for session {session_id}: {error:#}"
                    );
                }
            },
            Err(error) => {
                outcome.attach_failure_count = outcome.attach_failure_count.saturating_add(1);
                eprintln!(
                    "agentscan: failed to start subscriber client for session {session_id}: {error:#}"
                );
            }
        }
    }
    // Coverage is complete only when nothing was dropped by the cap *and* every
    // desired session actually ended up with a live subscriber. A failed attach
    // (transient tmux error, resource limit) leaves that session event-uncovered,
    // so coverage is incomplete and the reconcile poll stays active (see
    // `reconcile_interval_for`) until a later reconcile re-attaches it, rather than
    // relaxing to the self-heal backstop and starving the session.
    let coverage_complete = subscriber_coverage_complete(under_cap, &desired_session_ids, |id| {
        control_mode.has_subscriber(id)
    });
    let missing_session_ids = desired_session_ids
        .iter()
        .filter(|session_id| !control_mode.has_subscriber(session_id))
        .cloned()
        .collect();
    control_mode.set_subscriber_coverage(
        desired_session_ids,
        missing_session_ids,
        coverage_complete,
    );
    outcome
}

// Subscriber coverage is complete only if the cap dropped nothing (`under_cap`)
// and every desired session currently has a subscriber. Pure for testability.
fn subscriber_coverage_complete(
    under_cap: bool,
    desired: &[String],
    has_subscriber: impl Fn(&str) -> bool,
) -> bool {
    under_cap && desired.iter().all(|id| has_subscriber(id))
}

#[cfg(test)]
pub(crate) fn test_subscriber_coverage_complete(
    under_cap: bool,
    desired: &[String],
    present: &[String],
) -> bool {
    subscriber_coverage_complete(under_cap, desired, |id| {
        present.iter().any(|candidate| candidate == id)
    })
}

impl<S: StartupActions> DaemonRuntime<S> {
    fn from_started(
        startup: S,
        socket_state: DaemonSocketState,
        tmux_version: Option<String>,
        pending_snapshot: PreparedSnapshot,
        tmux_client: StartedTmuxControlModeClient,
        runtime_options: config::ResolvedRuntimeOptions,
        event_trace: Option<DaemonEventTrace>,
    ) -> Result<Self> {
        let snapshot = pending_snapshot.snapshot.clone();
        socket_state.publish_prepared_snapshot(pending_snapshot);
        let mut control_mode = RunningTmuxControlModeClient::from_started(
            tmux_client,
            startup.primary_session_id_for_status(),
        )?;
        socket_state.set_client_event_sender(control_mode.event_sender());
        let mut telemetry = RuntimeTelemetry::default();
        let subscriber_reconcile = reconcile_subscribers(&startup, &mut control_mode);
        telemetry.record_subscriber_reconcile(subscriber_reconcile);
        let pane_output_cache = scanner::PaneOutputStatusCache::new(PANE_OUTPUT_STATUS_CACHE_TTL);
        let now = Instant::now();
        let next_reconcile_at = now
            + reconcile_interval_for(
                control_mode.broker_enabled(),
                runtime_options.disable_reconcile,
                control_mode.subscriber_coverage_complete(),
            );
        let next_subscriber_monitor_at = next_subscriber_monitor_deadline(&control_mode, now);
        let broker_status = control_mode.broker_status_frame_with_deadlines(
            next_subscriber_monitor_at.map(duration_until_millis),
            Some(duration_until_millis(next_reconcile_at)),
        );
        socket_state.update_control_mode_broker_status(broker_status.clone());
        socket_state.update_runtime_telemetry(
            telemetry.frame(&broker_status, pane_output_cache.capture_stats()),
        );
        Ok(Self {
            startup,
            socket_state,
            tmux_version,
            snapshot,
            pane_output_cache,
            control_mode,
            next_reconcile_at,
            next_subscriber_monitor_at,
            settle_recapture_at: None,
            telemetry,
            deep_control_mode_telemetry: deep_control_mode_telemetry_enabled(),
            disable_reconcile: runtime_options.disable_reconcile,
            disable_proc_fallback: runtime_options.disable_proc_fallback,
            event_trace,
        })
    }

    fn run(&mut self, server_handle: &DaemonSocketServerHandle) -> Result<()> {
        // Arm the settle re-check from the boot snapshot: a pane already classified `Busy` from
        // pane output at startup that then goes quiet would otherwise never get a busy->idle
        // re-check (the deadline is only refreshed after a refresh request runs), leaving it
        // stuck busy until the next reconcile. `update_settle_deadline` is set-when-None, so this
        // is a no-op when nothing is busy.
        self.update_settle_deadline();
        // The socket-identity check is a coarse self-heal backstop; stat the path on a
        // fixed cadence rather than every wakeup. Seed it so the first loop iteration runs
        // the check immediately.
        let mut next_socket_identity_check_at = Instant::now();
        loop {
            if DAEMON_SHUTDOWN_REQUESTED.load(Ordering::Relaxed) {
                break;
            }
            if Instant::now() >= next_socket_identity_check_at {
                next_socket_identity_check_at = Instant::now() + SOCKET_IDENTITY_CHECK_INTERVAL;
                if !server_handle.socket_still_matches() {
                    eprintln!(
                        "agentscan: daemon socket path no longer matches this daemon; exiting"
                    );
                    DAEMON_SHUTDOWN_REQUESTED.store(true, Ordering::Relaxed);
                    break;
                }
            }
            if !server_handle.accept_thread_alive() {
                // The acceptor stopped without a recorded shutdown reason (e.g. a
                // panic in the accept loop). A daemon that no longer accepts is deaf;
                // exit so the next client auto-starts a healthy one.
                eprintln!("agentscan: daemon socket acceptor stopped unexpectedly; exiting");
                DAEMON_SHUTDOWN_REQUESTED.store(true, Ordering::Relaxed);
                break;
            }
            if Instant::now() >= self.next_reconcile_at {
                self.apply_refresh_request(RefreshRequest::IntervalReconcile)?;
                // The periodic reconcile is also the self-heal backstop for the
                // subscriber set: prune subscribers whose client died and re-attach
                // any missing sessions, even without a `%sessions-changed` event.
                self.reconcile_subscriber_clients();
            }

            if self
                .next_subscriber_monitor_at
                .is_some_and(|at| Instant::now() >= at)
            {
                self.monitor_subscriber_clients();
            }

            // A pane-output provider's idle transition emits no tmux event, so poll any pane
            // believed busy on the settle cadence. Clear the deadline before firing so the
            // post-refresh re-arm reflects the fresh result (re-armed if still busy, else
            // cleared) rather than the stale past instant.
            if self
                .settle_recapture_at
                .is_some_and(|at| Instant::now() >= at)
            {
                self.settle_recapture_at = None;
                self.apply_refresh_request(RefreshRequest::SettleRecapture)?;
            }

            let timeout = self.next_control_mode_wait();
            match self.control_mode.recv_timeout(timeout) {
                Ok(line) => {
                    let line = line?;
                    if let Some(event) = line.emitted_client_event() {
                        if self.apply_refresh_request(RefreshRequest::ClientEvent(&event))? {
                            break;
                        }
                        continue;
                    }
                    let lines = self.collect_control_mode_batch(line)?;
                    let session_set_changed = batch_changed_session_set(&lines);
                    if self.apply_refresh_request(RefreshRequest::ControlModeLines(&lines))? {
                        break;
                    }
                    if session_set_changed {
                        self.reconcile_subscriber_clients();
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // The retained sender means the channel never reports
                    // `Disconnected`, so poll the primary child directly to catch a
                    // primary that died without a `%exit` (e.g. the tmux server was
                    // SIGKILLed). MAX_WAIT bounds this to sub-second detection.
                    if self.control_mode.primary_child_exited() {
                        eprintln!(
                            "agentscan: tmux control-mode primary client exited; daemon stopping"
                        );
                        break;
                    }
                    // A subscriber client that died while its session is still alive
                    // leaves coverage stale (reported complete, so the interval stays
                    // at the 300s self-heal). Detect it here, bounded by MAX_WAIT, and
                    // reconcile to prune + re-attach and recompute coverage promptly.
                    if self.control_mode.has_dead_subscriber() {
                        self.reconcile_subscriber_clients();
                    }
                    if Instant::now() >= self.next_reconcile_at {
                        self.apply_refresh_request(RefreshRequest::TimeoutReconcile)?;
                        self.reconcile_subscriber_clients();
                    }
                }
                // Best-effort backstop only: with the retained sender the channel
                // does not disconnect on its own; primary death is detected by the
                // `%exit` event, a forwarded read error, or the liveness poll above.
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        Ok(())
    }

    fn next_control_mode_wait(&self) -> Duration {
        next_control_mode_wait_for(
            self.next_reconcile_at,
            self.next_subscriber_monitor_at,
            self.settle_recapture_at,
            Instant::now(),
        )
    }

    // Re-derive the subscriber client set from the live sessions. Called when a
    // `%sessions-changed` notification indicates a session was created or
    // destroyed, so runtime session changes get event coverage immediately
    // rather than waiting for the self-heal reconcile.
    fn reconcile_subscriber_clients(&mut self) {
        let outcome = reconcile_subscribers(&self.startup, &mut self.control_mode);
        self.telemetry.record_subscriber_reconcile(outcome);
        self.next_subscriber_monitor_at =
            next_subscriber_monitor_deadline(&self.control_mode, Instant::now());
        // Coverage may have just become incomplete (pushed over the cap), which
        // shortens the reconcile interval. Pull the next reconcile in so we do not
        // wait out an older, longer self-heal deadline before polling the
        // un-subscribed sessions. Never push the deadline out (min only).
        self.next_reconcile_at = self
            .next_reconcile_at
            .min(Instant::now() + self.reconcile_interval());
        // Republish broker status after deadline adjustment so telemetry reflects
        // the subscriber coverage state and the actual next reconcile deadline.
        let broker_status = self.broker_status_frame();
        self.socket_state
            .update_control_mode_broker_status(broker_status.clone());
        self.socket_state.update_runtime_telemetry(
            self.telemetry
                .frame(&broker_status, self.pane_output_cache.capture_stats()),
        );
    }

    fn monitor_subscriber_clients(&mut self) {
        self.telemetry.record_subscriber_monitor();
        if self.control_mode.has_dead_subscriber() {
            self.reconcile_subscriber_clients();
        } else {
            self.next_subscriber_monitor_at =
                next_subscriber_monitor_deadline(&self.control_mode, Instant::now());
            self.update_runtime_telemetry();
        }
    }

    fn collect_control_mode_batch(
        &mut self,
        first_line: ControlModeLine,
    ) -> Result<Vec<ControlModeLine>> {
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
                Ok(line) => {
                    let line = line?;
                    if line.is_client_event() {
                        self.control_mode.defer_line(line);
                        break;
                    }
                    lines.push(line);
                }
                Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {
                    break;
                }
            }
        }
        Ok(lines)
    }

    fn apply_refresh_request(&mut self, request: RefreshRequest<'_>) -> Result<bool> {
        let started_at = Instant::now();
        let observability = RefreshObservability::from_request(&request);
        // Single pre-refresh clone shared by every consumer that needs the before-state:
        // the observability diff below and the reconcile/publish gates inside each refresh
        // method (threaded in as `pre_refresh` so they no longer each re-clone the snapshot).
        let previous_snapshot = observability
            .should_capture_snapshot_diff()
            .then(|| self.snapshot.clone());
        let pre_refresh = previous_snapshot.as_ref();
        let mut outcome = match request {
            RefreshRequest::IntervalReconcile => self.apply_reconcile_refresh(
                SnapshotPublishContext::new("reconcile").with_detail("interval"),
                pre_refresh,
            )?,
            RefreshRequest::TimeoutReconcile => self.apply_reconcile_refresh(
                SnapshotPublishContext::new("reconcile").with_detail("timeout"),
                pre_refresh,
            )?,
            RefreshRequest::ControlModeLines(lines) => {
                self.apply_control_mode_refresh(lines, pre_refresh)?
            }
            RefreshRequest::ClientEvent(event) => {
                self.apply_client_event_refresh(event, pre_refresh)?
            }
            RefreshRequest::SettleRecapture => self.apply_settle_recapture_refresh(pre_refresh)?,
        };
        let publish_context = outcome.publish_context.take();
        let published = if let Some(publish_context) = publish_context {
            self.publish_current_snapshot(publish_context)
        } else {
            false
        };
        // The current snapshot is `self.snapshot` itself; `record_observability_event`
        // borrows it directly rather than taking a redundant clone here.
        self.record_observability_event(
            observability,
            previous_snapshot.as_ref(),
            &outcome,
            published,
            started_at.elapsed(),
        );
        if outcome.reset_reconcile_timer {
            self.next_reconcile_at = Instant::now() + self.reconcile_interval();
        }
        // Re-arm (or clear) the settle deadline from the current snapshot: any refresh that
        // leaves a pane-output provider busy means we must re-read it once the event stream
        // goes quiet. Activity-bearing refreshes keep pushing the deadline out; the pass only
        // fires after the turn's output stops.
        self.update_settle_deadline();
        Ok(outcome.should_exit)
    }

    /// Maintain `settle_recapture_at` as a steady re-check deadline whenever any pane reads
    /// busy from captured pane output. Such a status has no tmux event to refresh it when the
    /// turn ends, so the daemon polls it: the deadline is armed once when a busy pane-output
    /// pane appears and is left alone while set, so unrelated panes' activity cannot push it
    /// out (which would starve the re-check). It is re-armed after each fire and cleared once
    /// no pane-output pane is busy.
    fn update_settle_deadline(&mut self) {
        let has_busy_pane_output = self.snapshot.panes.iter().any(|pane| {
            pane.status.source == StatusSource::PaneOutput && pane.status.kind == StatusKind::Busy
        });
        self.settle_recapture_at = next_settle_deadline(
            has_busy_pane_output,
            self.settle_recapture_at,
            Instant::now(),
            PANE_OUTPUT_SETTLE_DELAY,
        );
    }

    /// Re-read pane-output providers currently believed busy, to catch an idle transition that
    /// emitted no tmux event. The cache entry is invalidated first so the re-read forces a
    /// fresh capture (a `Busy` pane is otherwise not a fallback candidate).
    fn apply_settle_recapture_refresh(
        &mut self,
        pre_refresh: Option<&SnapshotEnvelope>,
    ) -> Result<RefreshOutcome> {
        let busy_ids: Vec<String> = self
            .snapshot
            .panes
            .iter()
            .filter(|pane| {
                pane.status.source == StatusSource::PaneOutput
                    && pane.status.kind == StatusKind::Busy
            })
            .map(|pane| pane.pane_id.clone())
            .collect();
        if busy_ids.is_empty() {
            return Ok(RefreshOutcome::no_publish());
        }

        let owned_previous;
        let previous_snapshot = match pre_refresh {
            Some(previous) => previous,
            None => {
                owned_previous = self.snapshot.clone();
                &owned_previous
            }
        };
        let mut tmux_reads = self.control_mode.read_provider();
        // One lazily-captured process table for the whole settle pass, matching
        // the control-event batch path.
        let proc_inspector = proc::ProcProcessInspector;
        let proc_snapshot = proc::LazyProcessSnapshot::new(&proc_inspector);
        for pane_id in &busy_ids {
            self.pane_output_cache.invalidate(pane_id);
            refresh_snapshot_pane_with_title(
                &mut self.snapshot,
                &mut tmux_reads,
                pane_id,
                None,
                &mut self.pane_output_cache,
                &proc_snapshot,
                self.disable_proc_fallback,
            )?;
        }

        if snapshots_are_materially_equal(previous_snapshot, &self.snapshot) {
            self.update_runtime_telemetry();
            Ok(RefreshOutcome::no_publish())
        } else {
            Ok(RefreshOutcome::publish(
                SnapshotPublishContext::new("pane_output_settle").with_detail("busy_recheck"),
            ))
        }
    }

    fn record_observability_event(
        &mut self,
        observability: RefreshObservability,
        previous_snapshot: Option<&SnapshotEnvelope>,
        outcome: &RefreshOutcome,
        published: bool,
        duration: Duration,
    ) {
        if !observability.should_record && !outcome.reset_reconcile_timer && !published {
            return;
        }
        // The current snapshot is `self.snapshot`; borrow it for the diff instead of
        // cloning. The borrow ends once `event` is built, before the `&mut self` writes.
        let current_snapshot = &self.snapshot;
        let changed = previous_snapshot
            .is_some_and(|previous| !snapshots_are_materially_equal(previous, current_snapshot));
        let event = ipc::DaemonObservabilityEventFrame {
            at: snapshot::now_rfc3339().unwrap_or_else(|_| "unknown".to_string()),
            source: observability.source.to_string(),
            detail: observability.detail.into_detail(),
            refresh: observability.refresh.to_string(),
            control_sources: observability.control_sources,
            control_lines: observability.control_lines,
            changed,
            published,
            duration_ms: Some(duration_millis_u64(duration)),
            diff: previous_snapshot
                .and_then(|previous| changed.then(|| snapshot_diff(previous, current_snapshot))),
        };
        self.socket_state.record_observability_event(event.clone());
        if let Some(trace) = &mut self.event_trace {
            trace.write(&event);
        }
    }

    fn reconcile_interval(&self) -> Duration {
        reconcile_interval_for(
            self.control_mode.broker_enabled(),
            self.disable_reconcile,
            self.control_mode.subscriber_coverage_complete(),
        )
    }

    fn apply_control_mode_refresh(
        &mut self,
        lines: &[ControlModeLine],
        pre_refresh: Option<&SnapshotEnvelope>,
    ) -> Result<RefreshOutcome> {
        let batch = ControlEventBatch::from_control_lines(lines);
        self.telemetry.record_control_event_volume(&batch);
        let should_record_batch_telemetry =
            batch.has_telemetry_event() || self.deep_control_mode_telemetry;
        let has_subscriber_line = lines.iter().any(ControlModeLine::is_subscriber);
        if should_record_batch_telemetry {
            self.telemetry.record_control_event_kinds(&batch);
        }
        let should_exit = batch.should_exit;
        let event_publish_context = batch.publish_context();
        // The before-state for the reconcile telemetry and the publish gate below is
        // `pre_refresh`, the single pre-refresh clone taken in `apply_refresh_request`. It is
        // present on every path that reaches those uses: both are gated on the batch having
        // materially refreshed (`can_refresh_full_snapshot`/`event_outcome.changed`), which in
        // turn forces `observability_refresh() != "none"` and hence `should_capture_snapshot_diff`.
        let broker_enabled_before_refresh = self.control_mode.broker_enabled();
        let mut event_tmux_reads = self.control_mode.read_provider();
        let event_outcome = apply_control_event_batch(
            &mut self.snapshot,
            &mut event_tmux_reads,
            &batch,
            &mut self.pane_output_cache,
            self.disable_proc_fallback,
        )?;
        if !event_outcome.changed {
            let (reconnected, reset_reconcile_timer) =
                if control_event_should_recover_broker(should_exit) {
                    let reconnected = self.recover_broker_and_reconcile_if_needed()?;
                    let reset_reconcile_timer = control_event_refresh_should_reset_reconcile_timer(
                        broker_enabled_before_refresh,
                        reconnected,
                        self.control_mode.broker_enabled(),
                    );
                    (reconnected, reset_reconcile_timer)
                } else {
                    (false, false)
                };
            if should_record_batch_telemetry || has_subscriber_line {
                self.update_runtime_telemetry();
            }
            let mut outcome = if reconnected {
                RefreshOutcome::publish(
                    SnapshotPublishContext::new("reconcile").with_detail("broker_reconnect"),
                )
            } else if reset_reconcile_timer {
                RefreshOutcome::no_publish_and_reset_reconcile_timer()
            } else {
                RefreshOutcome::no_publish()
            };
            outcome.should_exit = should_exit;
            outcome.reset_reconcile_timer = reset_reconcile_timer;
            return Ok(outcome);
        }
        self.telemetry.record_control_event_refresh(&event_outcome);
        if event_outcome.full_snapshot_refresh
            && batch.can_refresh_full_snapshot()
            && let Some(previous_snapshot) = pre_refresh
        {
            self.telemetry
                .record_reconcile_result(previous_snapshot, &self.snapshot);
        }
        if event_outcome.fallback_to_full {
            self.telemetry.record_targeted_refresh_fallback_to_full();
        }

        let reconnected = self.recover_broker_and_reconcile_if_needed()?;
        let mut outcome = if reconnected {
            RefreshOutcome::publish(
                SnapshotPublishContext::new("reconcile").with_detail("broker_reconnect"),
            )
        } else if pre_refresh
            .is_some_and(|before| snapshots_are_materially_equal(before, &self.snapshot))
        {
            // The refresh ran but produced no material change (for example, a pane-output
            // activity tick whose status stayed busy); skip the redundant publish.
            self.update_runtime_telemetry();
            RefreshOutcome::no_publish()
        } else {
            RefreshOutcome::publish(event_publish_context.unwrap_or_else(|| {
                SnapshotPublishContext::new("control_event").with_detail("unknown")
            }))
        };
        outcome.should_exit = should_exit;
        if control_event_refresh_should_reset_reconcile_timer(
            broker_enabled_before_refresh,
            reconnected,
            self.control_mode.broker_enabled(),
        ) {
            outcome.reset_reconcile_timer = true;
        }
        Ok(outcome)
    }

    fn apply_reconcile_refresh(
        &mut self,
        publish_context: SnapshotPublishContext,
        pre_refresh: Option<&SnapshotEnvelope>,
    ) -> Result<RefreshOutcome> {
        let owned_previous;
        let previous_snapshot = match pre_refresh {
            Some(previous) => previous,
            None => {
                owned_previous = self.snapshot.clone();
                &owned_previous
            }
        };
        let mut reconcile_tmux_reads = self.control_mode.read_provider();
        reconcile_full_snapshot(
            &mut self.snapshot,
            &mut reconcile_tmux_reads,
            self.tmux_version.as_deref(),
            &mut self.pane_output_cache,
            self.disable_proc_fallback,
        )?;
        self.telemetry
            .record_reconcile_result(previous_snapshot, &self.snapshot);
        self.recover_broker_and_reconcile_if_needed()?;
        let outcome = reconcile_refresh_outcome(previous_snapshot, &self.snapshot, publish_context);
        if outcome.publish_context.is_none() {
            self.update_runtime_telemetry();
        }
        Ok(outcome)
    }

    fn apply_client_event_refresh(
        &mut self,
        event: &ipc::ClientEventFrame,
        pre_refresh: Option<&SnapshotEnvelope>,
    ) -> Result<RefreshOutcome> {
        match event {
            ipc::ClientEventFrame::PaneFocus { pane_id } => {
                let owned_previous;
                let previous_snapshot = match pre_refresh {
                    Some(previous) => previous,
                    None => {
                        owned_previous = self.snapshot.clone();
                        &owned_previous
                    }
                };
                let mut event_tmux_reads = self.control_mode.read_provider();
                refresh_snapshot_for_focused_pane(
                    &mut self.snapshot,
                    &mut event_tmux_reads,
                    pane_id,
                    self.tmux_version.as_deref(),
                    &mut self.pane_output_cache,
                    self.disable_proc_fallback,
                )?;
                self.telemetry
                    .record_reconcile_result(previous_snapshot, &self.snapshot);
                self.recover_broker_and_reconcile_if_needed()?;
                Ok(RefreshOutcome::publish(
                    SnapshotPublishContext::new("client_event")
                        .with_detail(client_event_detail(event)),
                ))
            }
        }
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
                self.disable_proc_fallback,
            )?;
            self.telemetry
                .record_reconcile_result(&previous_snapshot, &self.snapshot);
        }
        Ok(reconnected)
    }

    fn publish_current_snapshot(&self, publish_context: SnapshotPublishContext) -> bool {
        self.update_runtime_telemetry();
        // TODO(alloc): `publish_later_snapshot_with_context` takes the snapshot by value and
        // stores it in `PreparedSnapshot` (owned by the socket state), so the daemon must keep
        // its own copy — this clone is required by the current socket_server API boundary.
        // `encode_snapshot_frame` then clones it a second time to build the wire frame. Both
        // clones live behind socket_server (owned by another workstream); collapsing them needs
        // an `Arc<SnapshotEnvelope>` handoff there, not a change here.
        self.socket_state
            .publish_later_snapshot_with_context(self.snapshot.clone(), publish_context)
    }

    fn broker_status_frame(&self) -> ipc::ControlModeBrokerStatusFrame {
        self.control_mode.broker_status_frame_with_deadlines(
            self.next_subscriber_monitor_at.map(duration_until_millis),
            Some(duration_until_millis(self.next_reconcile_at)),
        )
    }

    fn update_runtime_telemetry(&self) {
        let broker_status = self.broker_status_frame();
        self.socket_state
            .update_control_mode_broker_status(broker_status.clone());
        self.socket_state.update_runtime_telemetry(
            self.telemetry
                .frame(&broker_status, self.pane_output_cache.capture_stats()),
        );
    }

    fn terminate_control_mode(mut self) {
        self.control_mode.terminate();
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

fn control_event_refresh_should_reset_reconcile_timer(
    broker_enabled_before_refresh: bool,
    reconnected: bool,
    broker_enabled: bool,
) -> bool {
    reconnected || (broker_enabled_before_refresh && !broker_enabled)
}

fn control_event_should_recover_broker(should_exit: bool) -> bool {
    !should_exit
}

/// Decide the next pane-output busy re-check deadline.
///
/// While a pane-output provider is busy the deadline is armed once and then left untouched
/// until it fires, so activity from *other* panes (which arrives continuously when any agent
/// is streaming) cannot keep pushing it out and starve the re-check. It clears as soon as no
/// pane-output pane is busy.
fn next_settle_deadline(
    has_busy_pane_output: bool,
    current: Option<Instant>,
    now: Instant,
    delay: Duration,
) -> Option<Instant> {
    if !has_busy_pane_output {
        return None;
    }
    current.or(Some(now + delay))
}

fn next_control_mode_wait_for(
    next_reconcile_at: Instant,
    next_subscriber_monitor_at: Option<Instant>,
    settle_recapture_at: Option<Instant>,
    now: Instant,
) -> Duration {
    // Wake for whichever comes first: the next reconcile, subscriber health
    // monitor, or pending settle re-capture.
    let mut next_wake = next_subscriber_monitor_at
        .map(|monitor_at| next_reconcile_at.min(monitor_at))
        .unwrap_or(next_reconcile_at);
    if let Some(settle_at) = settle_recapture_at {
        next_wake = next_wake.min(settle_at);
    }
    next_wake
        .saturating_duration_since(now)
        .max(CONTROL_MODE_MIN_WAIT)
        .min(CONTROL_MODE_MAX_WAIT)
}

fn next_subscriber_monitor_deadline(
    control_mode: &RunningTmuxControlModeClient,
    now: Instant,
) -> Option<Instant> {
    (control_mode.subscriber_count() > 0).then_some(now + SUBSCRIBER_MONITOR_POLL_INTERVAL)
}

fn reconcile_interval_for(
    broker_enabled: bool,
    disable_reconcile: bool,
    subscriber_coverage_complete: bool,
) -> Duration {
    if !broker_enabled {
        // No event stream at all: the reconcile poll is the sole update path, so
        // it stays fast regardless of `disable_reconcile`.
        return CONTROL_MODE_FALLBACK_RECONCILE_INTERVAL;
    }
    if disable_reconcile && subscriber_coverage_complete {
        // Every session is event-driven via its own subscriber client; the
        // reconcile is reduced to an infrequent self-heal/drift backstop.
        //
        // Known, intentional trade-off (default `disable_reconcile = true`): a
        // provider whose status comes from captured pane output
        // (`status.source = "pane_output"`, i.e. no pane-metadata or tmux-title
        // signal) only refreshes on a snapshot-changing event or a reconcile pass.
        // With `%output` paused, a pure busy/idle content change emits no event,
        // so such a provider's status can lag by up to this self-heal interval.
        // Metadata/title-driven providers are unaffected (they are event-driven).
        // This is accepted under the event-driven-first default; run with
        // `disable_reconcile = false` for 30s status refresh. See
        // docs/daemon-operations.md.
        return control_mode_self_heal_interval();
    }
    // Either redundancy reconcile is explicitly enabled, or subscriber coverage
    // is incomplete (more sessions than the cap) so the poll must stay active to
    // cover the sessions that have no event client.
    control_mode_active_reconcile_interval()
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
pub(crate) fn test_deep_control_mode_telemetry_value_enabled(value: &str) -> bool {
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
pub(crate) fn test_snapshot_observability(
    snapshot: &SnapshotEnvelope,
) -> ipc::SnapshotObservabilityFrame {
    snapshot_store::snapshot_observability(snapshot)
}

#[cfg(test)]
/// Returns `(total_line_count, output_line_count, output_byte_count, ignored_count)`
/// for a parsed control-mode batch.
pub(crate) fn test_control_event_batch_volume(lines: &[String]) -> (u64, u64, u64, u64) {
    let batch = ControlEventBatch::from_lines(lines);
    (
        batch.total_line_count,
        batch.output_line_count,
        batch.output_byte_count,
        batch.ignored_count,
    )
}

#[cfg(test)]
/// Records a single control-mode batch into otherwise-default telemetry, mirroring
/// the always-on volume path the daemon runs even for ignored-only batches.
pub(crate) fn test_runtime_telemetry_after_control_event_volume(
    lines: &[String],
) -> ipc::RuntimeTelemetryFrame {
    let mut telemetry = RuntimeTelemetry::default();
    telemetry.record_control_event_volume(&ControlEventBatch::from_lines(lines));
    telemetry.frame(
        &ipc::ControlModeBrokerStatusFrame {
            mode: ipc::ControlModeBrokerMode::Active,
            disabled_reason: None,
            reconnect_count: 0,
            fallback_count: Some(0),
            subscriber_count: None,
            primary_session_id: None,
            subscriber_coverage_complete: None,
            desired_subscriber_count: None,
            active_subscriber_count: None,
            missing_subscriber_session_ids: None,
            dead_subscriber_count: None,
            subscribers: None,
            last_subscriber_reconcile_at: None,
            next_subscriber_monitor_in_ms: None,
            next_reconcile_in_ms: None,
        },
        scanner::PaneOutputCaptureStats::default(),
    )
}

#[cfg(test)]
pub(crate) fn test_control_event_observability_for_lines(
    lines: &[String],
) -> (bool, bool, String, Option<String>) {
    let control_lines = lines
        .iter()
        .cloned()
        .map(|line| {
            ControlModeLine::new(
                control_mode::ControlModeLineSource::Primary { session_id: None },
                line,
            )
        })
        .collect::<Vec<_>>();
    let request = RefreshRequest::ControlModeLines(&control_lines);
    let observability = RefreshObservability::from_request(&request);
    (
        observability.should_record,
        observability.should_capture_snapshot_diff(),
        observability.refresh.to_string(),
        observability.detail.into_detail(),
    )
}

#[cfg(test)]
pub(crate) fn test_control_event_source_summary_for_lines(
    lines: &[(&str, Option<&str>, &str)],
) -> Vec<ipc::ControlModeSourceFrame> {
    let control_lines = lines
        .iter()
        .map(|(source, session_id, line)| {
            let source = match *source {
                "primary" => control_mode::ControlModeLineSource::Primary {
                    session_id: session_id.map(Arc::<str>::from),
                },
                "subscriber" => control_mode::ControlModeLineSource::Subscriber {
                    session_id: Arc::<str>::from(session_id.unwrap_or("unknown")),
                },
                other => panic!("unsupported control-mode line source {other}"),
            };
            ControlModeLine::new(source, (*line).to_string())
        })
        .collect::<Vec<_>>();
    ControlEventBatch::from_control_lines(&control_lines).control_sources
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
pub(crate) fn test_control_event_refresh_should_reset_reconcile_timer(
    broker_enabled_before_refresh: bool,
    reconnected: bool,
    broker_enabled: bool,
) -> bool {
    control_event_refresh_should_reset_reconcile_timer(
        broker_enabled_before_refresh,
        reconnected,
        broker_enabled,
    )
}

#[cfg(test)]
pub(crate) fn test_control_event_should_recover_broker(should_exit: bool) -> bool {
    control_event_should_recover_broker(should_exit)
}

#[cfg(test)]
pub(crate) fn test_reconcile_interval_for(
    broker_enabled: bool,
    disable_reconcile: bool,
    subscriber_coverage_complete: bool,
) -> Duration {
    reconcile_interval_for(
        broker_enabled,
        disable_reconcile,
        subscriber_coverage_complete,
    )
}

#[cfg(test)]
pub(crate) fn test_next_settle_deadline(
    has_busy_pane_output: bool,
    current: Option<Instant>,
    now: Instant,
    delay: Duration,
) -> Option<Instant> {
    next_settle_deadline(has_busy_pane_output, current, now, delay)
}

#[cfg(test)]
pub(crate) fn test_next_control_mode_wait_for(
    next_reconcile_after: Duration,
    next_subscriber_monitor_after: Option<Duration>,
    settle_recapture_after: Option<Duration>,
) -> Duration {
    let now = Instant::now();
    next_control_mode_wait_for(
        now + next_reconcile_after,
        next_subscriber_monitor_after.map(|duration| now + duration),
        settle_recapture_after.map(|duration| now + duration),
        now,
    )
}

#[cfg(test)]
pub(crate) fn test_capped_subscriber_session_ids(session_ids: Vec<String>) -> Vec<String> {
    capped_subscriber_session_ids(session_ids)
}

#[cfg(test)]
pub(crate) fn test_runtime_telemetry_after_reconcile_results(
    previous: &SnapshotEnvelope,
    noop_current: &SnapshotEnvelope,
    changed_current: &SnapshotEnvelope,
) -> ipc::RuntimeTelemetryFrame {
    let mut telemetry = RuntimeTelemetry::default();
    telemetry.record_control_event_volume(&ControlEventBatch::from_lines(&[
        "%unknown-a".to_string(),
        "%unknown-b".to_string(),
    ]));
    telemetry.record_control_event_volume(&ControlEventBatch::from_lines(&[
        "%unknown-c".to_string(),
        "%unknown-d".to_string(),
        "%unknown-e".to_string(),
    ]));
    telemetry.record_control_event_refresh(&ControlEventOutcome {
        changed: true,
        fallback_to_full: true,
        full_snapshot_refresh: true,
        targeted_title_updates: 1,
        targeted_pane_refreshes: 2,
        targeted_scope_refreshes: 1,
    });
    telemetry.record_targeted_refresh_fallback_to_full();
    telemetry.record_reconcile_result(previous, noop_current);
    telemetry.record_reconcile_result(noop_current, changed_current);
    telemetry.frame(
        &ipc::ControlModeBrokerStatusFrame {
            mode: ipc::ControlModeBrokerMode::Fallback,
            disabled_reason: Some("test fallback".to_string()),
            reconnect_count: 1,
            fallback_count: Some(2),
            subscriber_count: None,
            primary_session_id: None,
            subscriber_coverage_complete: None,
            desired_subscriber_count: None,
            active_subscriber_count: None,
            missing_subscriber_session_ids: None,
            dead_subscriber_count: None,
            subscribers: None,
            last_subscriber_reconcile_at: None,
            next_subscriber_monitor_in_ms: None,
            next_reconcile_in_ms: None,
        },
        scanner::PaneOutputCaptureStats::default(),
    )
}
