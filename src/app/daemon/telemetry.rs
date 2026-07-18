use std::env;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::Arc;

use crate::app::{SnapshotEnvelope, ipc, scanner};

use super::control_mode::SubscriberReconcileOutcome;
#[cfg(test)]
use super::control_mode::{self, ControlModeLine};
use super::events::{ControlEventBatch, ControlEventOutcome};
use super::refresh::snapshots_are_materially_equal;
use super::runtime::RefreshRequest;
use super::{
    DEFAULT_TRACE_EVENT_LIMIT, TRACE_EVENT_LIMIT_ENV_VAR, TRACE_EVENTS_ENV_VAR,
    client_event_detail, env_value_enabled, trace_control_lines_enabled,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct RuntimeTelemetry {
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
    pub(super) fn record_control_event_volume(&mut self, batch: &ControlEventBatch) {
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

    pub(super) fn record_control_event_kinds(&mut self, batch: &ControlEventBatch) {
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

    pub(super) fn record_control_event_refresh(&mut self, outcome: &ControlEventOutcome) {
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

    pub(super) fn record_targeted_refresh_fallback_to_full(&mut self) {
        self.targeted_refresh_fallback_to_full_count = self
            .targeted_refresh_fallback_to_full_count
            .saturating_add(1);
    }

    pub(super) fn record_subscriber_monitor(&mut self) {
        self.subscriber_monitor_count = self.subscriber_monitor_count.saturating_add(1);
    }

    pub(super) fn record_subscriber_reconcile(&mut self, outcome: SubscriberReconcileOutcome) {
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

    pub(super) fn record_reconcile_result(
        &mut self,
        previous: &SnapshotEnvelope,
        current: &SnapshotEnvelope,
    ) {
        self.reconcile_attempt_count = self.reconcile_attempt_count.saturating_add(1);
        if snapshots_are_materially_equal(previous, current) {
            self.reconcile_noop_count = self.reconcile_noop_count.saturating_add(1);
        } else {
            self.reconcile_changed_snapshot_count =
                self.reconcile_changed_snapshot_count.saturating_add(1);
        }
    }

    pub(super) fn frame(
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

pub(crate) struct DaemonEventTrace {
    path: PathBuf,
    limit: usize,
    written_since_truncate: usize,
    // The trace is written by the single daemon loop, so the open handle is held
    // across events and only reopened on rotation. Sequential writes to a
    // `File::create` handle advance the cursor, so this appends without O_APPEND.
    file: File,
}

impl DaemonEventTrace {
    pub(super) fn from_socket_path(socket_path: &Path) -> Option<Self> {
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

    pub(super) fn write(&mut self, event: &ipc::DaemonObservabilityEventFrame) {
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

pub(crate) struct RefreshObservability {
    pub(super) source: &'static str,
    pub(super) detail: ObservabilityDetail,
    pub(super) refresh: &'static str,
    pub(super) should_record: bool,
    pub(super) control_sources: Vec<ipc::ControlModeSourceFrame>,
    pub(super) control_lines: Vec<String>,
}

/// Deferred observability detail. Building the human-readable detail string is
/// delayed until an event is actually recorded, so ignored-only `%output`
/// firehose batches (which carry only `Ignored`) never pay the allocation.
pub(super) enum ObservabilityDetail {
    None,
    Static(&'static str),
    Owned(String),
    Ignored(u64),
}

impl ObservabilityDetail {
    pub(super) fn into_detail(self) -> Option<String> {
        match self {
            ObservabilityDetail::None => None,
            ObservabilityDetail::Static(detail) => Some(detail.to_string()),
            ObservabilityDetail::Owned(detail) => Some(detail),
            ObservabilityDetail::Ignored(count) => Some(format!("ignored:{count}")),
        }
    }
}

impl RefreshObservability {
    pub(super) fn from_request(request: &RefreshRequest<'_>) -> Self {
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

    pub(super) fn should_capture_snapshot_diff(&self) -> bool {
        self.refresh != "none"
    }
}

#[cfg(test)]
/// Returns `(total_line_count, output_line_count, output_byte_count, ignored_count)`
/// for a parsed control-mode batch.
pub(super) fn run_control_event_batch_volume(lines: &[String]) -> (u64, u64, u64, u64) {
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
pub(super) fn run_runtime_telemetry_after_control_event_volume(
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
pub(super) fn run_control_event_observability_for_lines(
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
pub(super) fn run_control_event_source_summary_for_lines(
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
pub(super) fn run_runtime_telemetry_after_reconcile_results(
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
#[cfg(test)]
mod migrated_tests {
    use super::super::migrated_tests::empty_socket_snapshot;
    use crate::app::tests::proc_fallback_pane;

    #[test]
    fn control_event_batch_counts_output_firehose_volume() {
        let lines = vec![
            "%output %1 \\033]0;Claude Code | repo\\007".to_string(),
            "%output %2 ordinary streaming bytes".to_string(),
            "%subscription-changed agentscan $1 @1 0 %1 : %1:claude:::::".to_string(),
        ];

        let (total, output_lines, output_bytes, ignored) =
            super::run_control_event_batch_volume(&lines);

        // Every line is counted, both `%output` lines are sized (title-bearing or not),
        // and only the non-title `%output` line lands in the ignored bucket.
        assert_eq!(total, 3);
        assert_eq!(output_lines, 2);
        assert_eq!(output_bytes, (lines[0].len() + lines[1].len()) as u64);
        assert_eq!(ignored, 1);
    }

    #[test]
    fn runtime_telemetry_records_volume_for_ignored_only_output_batch() {
        // A pure `%output` firehose burst with no title and no metadata change still
        // updates the always-on volume counters, while leaving the gated kind counters
        // (pane/title/window/session) at zero.
        let lines = vec![
            "%output %1 streaming tokens".to_string(),
            "%output %1 more tokens".to_string(),
        ];

        let frame = super::run_runtime_telemetry_after_control_event_volume(&lines);

        assert_eq!(frame.control_event_batch_count, 1);
        assert_eq!(frame.control_event_line_count, 2);
        assert_eq!(frame.control_event_output_line_count, 2);
        assert_eq!(
            frame.control_event_output_byte_count,
            (lines[0].len() + lines[1].len()) as u64
        );
        assert_eq!(frame.control_event_ignored_count, 2);
        assert_eq!(frame.control_event_pane_count, 0);
        assert_eq!(frame.control_event_title_count, 0);
    }

    #[test]
    fn daemon_observability_skips_snapshot_diff_for_ignored_control_output() {
        let lines = vec!["%output %1 ordinary pane bytes".to_string()];

        let (should_record, should_capture_snapshot_diff, refresh, detail) =
            super::run_control_event_observability_for_lines(&lines);

        assert!(!should_record);
        assert!(!should_capture_snapshot_diff);
        assert_eq!(refresh, "none");
        assert_eq!(detail.as_deref(), Some("ignored:1"));
    }

    #[test]
    fn daemon_control_event_source_summary_counts_lines_and_events_per_client() {
        let sources = super::run_control_event_source_summary_for_lines(&[
            (
                "primary",
                Some("$0"),
                "%subscription-changed agentscan $0 @1 0 %1 : %1:codex:::::",
            ),
            ("subscriber", Some("$2"), "%output %7 ordinary bytes"),
            (
                "subscriber",
                Some("$2"),
                "%subscription-changed agentscan $2 @4 0 %7 : %7:codex:::::",
            ),
        ]);

        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].source, "primary");
        assert_eq!(sources[0].session_id.as_deref(), Some("$0"));
        assert_eq!(sources[0].line_count, 1);
        assert_eq!(sources[0].event_count, 1);
        assert_eq!(sources[1].source, "subscriber");
        assert_eq!(sources[1].session_id.as_deref(), Some("$2"));
        assert_eq!(sources[1].line_count, 2);
        assert_eq!(sources[1].event_count, 1);
    }

    #[test]
    fn daemon_runtime_telemetry_counts_reconcile_results_and_fallbacks() {
        let previous = empty_socket_snapshot("2026-05-23T18:00:00Z");
        let mut noop_current = previous.clone();
        noop_current.generated_at = "2026-05-23T18:00:01Z".to_string();
        noop_current.source.daemon_generated_at = Some("2026-05-23T18:00:01Z".to_string());

        let mut changed_current = noop_current.clone();
        changed_current
            .panes
            .push(proc_fallback_pane(42, "claude", "claude"));

        let telemetry = super::run_runtime_telemetry_after_reconcile_results(
            &previous,
            &noop_current,
            &changed_current,
        );

        assert_eq!(telemetry.control_event_refresh_count, 1);
        assert_eq!(telemetry.control_event_batch_count, 2);
        assert_eq!(telemetry.control_event_line_count, 5);
        assert_eq!(telemetry.targeted_title_update_count, 1);
        assert_eq!(telemetry.targeted_pane_refresh_count, 2);
        assert_eq!(telemetry.targeted_scope_refresh_count, 1);
        assert_eq!(telemetry.full_snapshot_refresh_count, 1);
        assert_eq!(telemetry.targeted_refresh_fallback_to_full_count, 1);
        assert_eq!(telemetry.reconcile_attempt_count, 2);
        assert_eq!(telemetry.reconcile_noop_count, 1);
        assert_eq!(telemetry.reconcile_changed_snapshot_count, 1);
        assert_eq!(telemetry.broker_fallback_count, 2);
    }
}
