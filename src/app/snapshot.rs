use super::*;

pub(super) fn daemon_snapshot_from_tmux() -> Result<SnapshotEnvelope> {
    let mut snapshot = scanner::snapshot_from_tmux()?;
    set_snapshot_cache_origin(&mut snapshot, "daemon_snapshot");
    mark_snapshot_as_daemon(&mut snapshot)?;
    Ok(snapshot)
}

pub(super) fn mark_snapshot_as_daemon(snapshot: &mut SnapshotEnvelope) -> Result<()> {
    snapshot.generated_at = now_rfc3339()?;
    snapshot.source.kind = SourceKind::Daemon;
    snapshot.source.daemon_generated_at = Some(snapshot.generated_at.clone());
    Ok(())
}

pub(crate) fn filter_snapshot(snapshot: &mut SnapshotEnvelope, include_all: bool) {
    if !include_all {
        snapshot.panes.retain(|pane| pane.provider.is_some());
    }
}

pub(crate) fn sort_snapshot_panes(snapshot: &mut SnapshotEnvelope) {
    snapshot.panes.sort_by(|left, right| {
        (
            &left.location.session_name,
            left.location.window_index,
            left.location.pane_index,
            &left.pane_id,
        )
            .cmp(&(
                &right.location.session_name,
                right.location.window_index,
                right.location.pane_index,
                &right.pane_id,
            ))
    });
}

pub(crate) fn now_rfc3339() -> Result<String> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .context("failed to format current time")
}

pub(crate) fn validate_snapshot(snapshot: &SnapshotEnvelope) -> Result<SnapshotSummary> {
    if snapshot.schema_version != CACHE_SCHEMA_VERSION {
        bail!(
            "unsupported snapshot schema version {} (expected {})",
            snapshot.schema_version,
            CACHE_SCHEMA_VERSION
        );
    }

    summarize_snapshot(snapshot)
}

pub(crate) fn summarize_snapshot(snapshot: &SnapshotEnvelope) -> Result<SnapshotSummary> {
    OffsetDateTime::parse(&snapshot.generated_at, &Rfc3339)
        .context("generated_at was not valid RFC3339")?;

    let pane_count = snapshot.panes.len();
    let agent_pane_count = snapshot
        .panes
        .iter()
        .filter(|pane| pane.provider.is_some())
        .count();

    let provider_counts = provider_summary_order()
        .filter_map(|provider| {
            let count = snapshot
                .panes
                .iter()
                .filter(|pane| pane.provider == Some(provider))
                .count();
            (count > 0).then_some((provider, count))
        })
        .collect();

    let status_counts = [StatusKind::Busy, StatusKind::Idle, StatusKind::Unknown]
        .into_iter()
        .filter_map(|status| {
            let count = snapshot
                .panes
                .iter()
                .filter(|pane| pane.status.kind == status)
                .count();
            (count > 0).then_some((status, count))
        })
        .collect();

    Ok(SnapshotSummary {
        pane_count,
        agent_pane_count,
        provider_counts,
        status_counts,
    })
}

pub(super) fn set_snapshot_cache_origin(snapshot: &mut SnapshotEnvelope, cache_origin: &str) {
    for pane in &mut snapshot.panes {
        pane.diagnostics.cache_origin = cache_origin.to_string();
    }
}
