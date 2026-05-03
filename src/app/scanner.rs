use super::*;

pub(crate) fn snapshot_from_tmux() -> Result<SnapshotEnvelope> {
    let rows = tmux::tmux_list_panes()?;
    let proc_inspector = proc::ProcProcessInspector;
    let mut panes = classify::panes_from_rows_with_proc_fallback(rows, &proc_inspector);
    apply_pane_output_status_fallbacks(&mut panes);

    let mut snapshot = SnapshotEnvelope {
        schema_version: CACHE_SCHEMA_VERSION,
        generated_at: snapshot::now_rfc3339()?,
        source: SnapshotSource {
            kind: SourceKind::Snapshot,
            tmux_version: tmux::tmux_version(),
            daemon_generated_at: None,
        },
        panes,
    };
    snapshot::sort_snapshot_panes(&mut snapshot);
    Ok(snapshot)
}

pub(crate) fn apply_pane_output_status_fallbacks(panes: &mut [PaneRecord]) {
    const PANE_OUTPUT_STATUS_LINES: usize = 30;

    for pane in panes {
        if !classify::pane_output_status_fallback_candidate(pane) {
            continue;
        }

        if let Ok(Some(output)) =
            tmux::tmux_capture_pane_tail(&pane.pane_id, PANE_OUTPUT_STATUS_LINES)
        {
            classify::apply_pane_output_status_fallback(pane, &output);
        }
    }
}
