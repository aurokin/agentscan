use super::*;

#[derive(Default)]
pub(super) struct SnapshotStore {
    latest_snapshot: Option<SnapshotEnvelope>,
    latest_snapshot_frame: Option<EncodedDaemonFrame>,
    latest_snapshot_update: Option<SnapshotUpdateTelemetry>,
}

impl SnapshotStore {
    pub(super) fn publish(
        &mut self,
        prepared: PreparedSnapshot,
        telemetry: SnapshotUpdateTelemetry,
    ) -> EncodedDaemonFrame {
        let frame = prepared.frame.clone();
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
}
