use super::*;

mod pane_output;

pub(crate) use pane_output::{
    PaneOutputCaptureStats, PaneOutputStatusCache, apply_pane_output_status_fallbacks,
    apply_pane_output_status_fallbacks_with_cache,
};

pub(crate) fn snapshot_from_tmux() -> Result<SnapshotEnvelope> {
    snapshot_from_tmux_with_version(tmux::tmux_version())
}

pub(crate) fn snapshot_from_tmux_with_version(
    tmux_version: Option<String>,
) -> Result<SnapshotEnvelope> {
    let runtime_options = config::resolve_runtime_options()?;
    let rows = tmux::tmux_list_panes()?;
    let proc_inspector = proc::ProcProcessInspector;
    let mut panes = classify::panes_from_rows_with_proc_fallback_options(
        rows,
        &proc_inspector,
        runtime_options.disable_proc_fallback,
    );
    let capture_stats = apply_pane_output_status_fallbacks(&mut panes);
    if capture_stats.error_count > 0 {
        // The direct-scan path has no telemetry sink (the daemon feeds these
        // counts into lifecycle-status telemetry), so surface transient
        // capture failures on stderr; stdout stays machine-readable.
        eprintln!(
            "agentscan: {} pane output capture(s) failed; affected pane statuses may be unknown",
            capture_stats.error_count
        );
    }

    let mut snapshot = SnapshotEnvelope {
        schema_version: CACHE_SCHEMA_VERSION,
        generated_at: snapshot::now_rfc3339()?,
        source: SnapshotSource {
            kind: SourceKind::Snapshot,
            tmux_version,
            daemon_generated_at: None,
        },
        panes,
    };
    snapshot::sort_snapshot_panes(&mut snapshot);
    Ok(snapshot)
}
