use std::sync::Arc;

use super::socket_server::{DaemonBroadcast, PreparedSnapshot, SnapshotUpdateTelemetry};
use super::*;

/// Headroom added to the running full-frame size bound. The bound itself is
/// sound for pane payloads (the diff frame carries every changed pane's full
/// serialization, so the full frame can grow by at most the diff's bytes); the
/// slack covers the few bytes of drift the diff does not mirror one-to-one:
/// `seq` gaining digits and timestamp-width variance in the envelope fields.
const FULL_FRAME_BOUND_SLACK: usize = 256;

#[derive(Default)]
pub(super) struct SnapshotStore {
    latest_snapshot: Option<Arc<SnapshotEnvelope>>,
    // Cached full snapshot frame for `latest_seq`. Eagerly set on the bootstrap
    // publish; invalidated (`None`) on every diff publish and lazily re-encoded
    // only when a bootstrap or one-shot snapshot query actually needs it, so the
    // steady-state publish path never re-serializes the whole snapshot.
    latest_full_frame: Option<EncodedDaemonFrame>,
    // Upper bound on the encoded size of `latest_snapshot`'s full frame,
    // maintained without encoding it: each committed diff can grow the full
    // frame by at most the diff frame's own byte count. Reset to the actual
    // encoded size whenever a real full encode happens. `publish_diff` uses it
    // to guarantee the committed snapshot always stays bootstrap-encodable
    // within the wire limit.
    full_frame_size_bound: usize,
    latest_seq: u64,
    latest_snapshot_update: Option<SnapshotUpdateTelemetry>,
    latest_observability: Option<ipc::SnapshotObservabilityFrame>,
}

impl SnapshotStore {
    /// Bootstrap publish: adopts the prepared full frame (already encoded at its
    /// seq) as both the fan-out frame and the cached bootstrap frame.
    pub(super) fn publish_initial(
        &mut self,
        prepared: PreparedSnapshot,
        telemetry: SnapshotUpdateTelemetry,
    ) -> EncodedDaemonFrame {
        let PreparedSnapshot {
            snapshot,
            frame,
            seq,
        } = prepared;
        self.latest_seq = seq;
        self.latest_observability = Some(snapshot_observability(&snapshot));
        self.latest_snapshot = Some(Arc::new(snapshot));
        self.full_frame_size_bound = frame.len();
        self.latest_full_frame = Some(frame.clone());
        self.latest_snapshot_update = Some(telemetry);
        frame
    }

    /// Post-bootstrap publish: assigns the next seq, encodes a `snapshot_diff`
    /// against the previously published snapshot, and returns the fan-out
    /// broadcast (diff primary + lazy full frame for coalesce safety). Returns
    /// `Err` when the diff frame exceeds the wire limit, or when committing the
    /// snapshot would leave its full frame unencodable within the wire limit (a
    /// snapshot must never grow past the limit through small diffs, or later
    /// bootstraps and one-shot queries could never be served again). In both
    /// cases the store is left untouched and the previous snapshot stays
    /// authoritative.
    pub(super) fn publish_diff(
        &mut self,
        snapshot: SnapshotEnvelope,
        telemetry: SnapshotUpdateTelemetry,
    ) -> Result<DaemonBroadcast> {
        let seq = self.latest_seq.saturating_add(1);
        let snapshot = Arc::new(snapshot);
        let broadcast = match &self.latest_snapshot {
            Some(previous) => super::socket_server::build_diff_broadcast(seq, previous, &snapshot)?,
            // No bootstrap yet (only reachable in isolated tests): fall back to a
            // full-frame broadcast so there is never a diff without a base.
            None => super::socket_server::build_full_broadcast(seq, &snapshot)?,
        };
        // Bootstrap-encodability guard: while the running bound stays under the
        // wire limit, commit without touching the full frame. Once the bound
        // crosses it, pay for one real encode — if the full frame fits, commit
        // (keeping the fresh encode as the cache) and reset the bound to the
        // actual size; if not, reject and keep serving the last good snapshot.
        // `omitted_pane_growth` covers volatile pane fields that grow the full
        // frame without appearing in the diff frame.
        let candidate_bound = self
            .full_frame_size_bound
            .saturating_add(broadcast.primary_len())
            .saturating_add(broadcast.omitted_pane_growth())
            .saturating_add(FULL_FRAME_BOUND_SLACK);
        let (new_bound, full_frame) = if candidate_bound <= ipc::DAEMON_FRAME_MAX_BYTES {
            (candidate_bound, None)
        } else {
            let frame = super::socket_server::encode_full_frame(&snapshot, seq)?;
            (frame.len(), Some(frame))
        };
        self.latest_seq = seq;
        self.latest_observability = Some(snapshot_observability(&snapshot));
        self.latest_snapshot = Some(snapshot);
        // Lazily re-encoded in `latest_frame` when `full_frame` is `None`.
        self.latest_full_frame = full_frame;
        self.full_frame_size_bound = new_bound;
        self.latest_snapshot_update = Some(telemetry);
        Ok(broadcast)
    }

    pub(super) fn latest_frame(&mut self) -> Option<EncodedDaemonFrame> {
        if self.latest_full_frame.is_none() {
            let snapshot = self.latest_snapshot.as_ref()?;
            // Encode-on-demand for a bootstrap or one-shot snapshot query. If it
            // exceeds the wire limit we cannot serve a full frame; leave the cache
            // empty and report absence so the caller surfaces "ready without a
            // snapshot" rather than sending a truncated frame.
            self.latest_full_frame =
                super::socket_server::encode_full_frame(snapshot, self.latest_seq).ok();
        }
        self.latest_full_frame.clone()
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
