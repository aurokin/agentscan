use super::socket_server::{PreparedSnapshot, SnapshotUpdateTelemetry};
use super::*;

#[derive(Default)]
pub(super) struct SnapshotStore {
    latest_snapshot: Option<SnapshotEnvelope>,
    latest_snapshot_frame: Option<EncodedDaemonFrame>,
    latest_snapshot_update: Option<SnapshotUpdateTelemetry>,
    latest_observability: Option<ipc::SnapshotObservabilityFrame>,
}

impl SnapshotStore {
    pub(super) fn publish(
        &mut self,
        prepared: PreparedSnapshot,
        telemetry: SnapshotUpdateTelemetry,
    ) -> EncodedDaemonFrame {
        let frame = prepared.frame.clone();
        self.latest_observability = Some(snapshot_observability(&prepared.snapshot));
        self.latest_snapshot = Some(prepared.snapshot);
        self.latest_snapshot_frame = Some(frame.clone());
        self.latest_snapshot_update = Some(telemetry);
        frame
    }

    pub(super) fn latest_frame(&self) -> Option<EncodedDaemonFrame> {
        self.latest_snapshot_frame.clone()
    }

    pub(super) fn latest_generated_at(&self) -> Option<String> {
        self.latest_snapshot
            .as_ref()
            .map(|snapshot| snapshot.generated_at.clone())
    }

    pub(super) fn latest_pane_count(&self) -> Option<usize> {
        self.latest_snapshot
            .as_ref()
            .map(|snapshot| snapshot.panes.len())
    }

    pub(super) fn latest_update(&self) -> Option<&SnapshotUpdateTelemetry> {
        self.latest_snapshot_update.as_ref()
    }

    pub(super) fn latest_observability(&self) -> Option<ipc::SnapshotObservabilityFrame> {
        self.latest_observability.clone()
    }
}

pub(super) fn snapshot_observability(
    snapshot: &SnapshotEnvelope,
) -> ipc::SnapshotObservabilityFrame {
    let mut observability = ipc::SnapshotObservabilityFrame::default();
    for pane in &snapshot.panes {
        if pane.provider.is_some() {
            observability.provider_known_count += 1;
        } else {
            observability.provider_unknown_count += 1;
        }

        match pane.status.source {
            StatusSource::PaneMetadata => observability.status_source_pane_metadata_count += 1,
            StatusSource::TmuxTitle => observability.status_source_tmux_title_count += 1,
            StatusSource::PaneOutput => observability.status_source_pane_output_count += 1,
            StatusSource::NotChecked => observability.status_source_not_checked_count += 1,
        }

        match pane.diagnostics.proc_fallback.outcome {
            ProcFallbackOutcome::NotRun => observability.proc_fallback_not_run_count += 1,
            ProcFallbackOutcome::Skipped => observability.proc_fallback_skipped_count += 1,
            ProcFallbackOutcome::NoMatch => observability.proc_fallback_no_match_count += 1,
            ProcFallbackOutcome::Error => observability.proc_fallback_error_count += 1,
            ProcFallbackOutcome::Resolved => observability.proc_fallback_resolved_count += 1,
        }

        accumulate_provider_path_stats(&mut observability, pane);
    }
    observability
}

/// Buckets a pane's identity match-kind and status-source into the per-provider
/// breakdown. Unclassified panes accumulate under `unknown` so the buckets sum to
/// the snapshot pane count.
fn accumulate_provider_path_stats(
    observability: &mut ipc::SnapshotObservabilityFrame,
    pane: &PaneRecord,
) {
    let key = pane
        .provider
        .map(|provider| provider.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let stats = observability.per_provider.entry(key).or_default();
    stats.pane_count += 1;

    match pane.classification.matched_by {
        Some(ClassificationMatchKind::PaneMetadata) => stats.matched_pane_metadata_count += 1,
        Some(ClassificationMatchKind::PaneCurrentCommand) => {
            stats.matched_pane_current_command_count += 1
        }
        Some(ClassificationMatchKind::PaneTitle) => stats.matched_pane_title_count += 1,
        Some(ClassificationMatchKind::ProcProcessTree) => {
            stats.matched_proc_process_tree_count += 1
        }
        None => {}
    }

    match pane.status.source {
        StatusSource::PaneMetadata => stats.status_source_pane_metadata_count += 1,
        StatusSource::TmuxTitle => stats.status_source_tmux_title_count += 1,
        StatusSource::PaneOutput => stats.status_source_pane_output_count += 1,
        StatusSource::NotChecked => stats.status_source_not_checked_count += 1,
    }
}
