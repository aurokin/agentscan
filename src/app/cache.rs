use super::*;

pub(super) fn snapshot_from_tmux() -> Result<SnapshotEnvelope> {
    let rows = tmux::tmux_list_panes()?;
    let panes = rows.into_iter().map(classify::pane_from_row).collect();

    let mut snapshot = SnapshotEnvelope {
        schema_version: CACHE_SCHEMA_VERSION,
        generated_at: now_rfc3339()?,
        source: SnapshotSource {
            kind: SourceKind::Snapshot,
            tmux_version: tmux::tmux_version(),
            daemon_generated_at: None,
        },
        panes,
    };
    sort_snapshot_panes(&mut snapshot);
    Ok(snapshot)
}

pub(super) fn daemon_snapshot_from_tmux() -> Result<SnapshotEnvelope> {
    let mut snapshot = snapshot_from_tmux()?;
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

pub(super) fn refresh_cache_from_tmux() -> Result<SnapshotEnvelope> {
    let existing = read_existing_snapshot_if_valid();
    let mut snapshot = snapshot_from_tmux()?;
    preserve_last_daemon_refresh(&mut snapshot, existing.as_ref());
    write_snapshot_to_cache(&snapshot)?;
    Ok(snapshot)
}

pub(super) fn load_snapshot(refresh: bool) -> Result<SnapshotEnvelope> {
    if refresh {
        return refresh_cache_from_tmux();
    }

    read_snapshot_from_cache()
}

pub(super) fn read_snapshot_from_cache() -> Result<SnapshotEnvelope> {
    let path = cache_path()?;
    let contents = fs::read_to_string(&path).with_context(|| {
        format!(
            "failed to read cache at {}. Run `agentscan daemon run` first or rerun with `-f` to refresh directly from tmux",
            path.display()
        )
    })?;

    let snapshot: SnapshotEnvelope = serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse cache at {}", path.display()))?;
    validate_snapshot(&snapshot, None)
        .with_context(|| format!("cache validation failed for {}", path.display()))?;
    Ok(snapshot)
}

pub(super) fn write_snapshot_to_cache(snapshot: &SnapshotEnvelope) -> Result<()> {
    let path = cache_path()?;
    let parent = path
        .parent()
        .context("cache path did not have a parent directory")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create cache directory {}", parent.display()))?;

    let temp_path = path.with_extension("tmp");
    let contents =
        serde_json::to_vec_pretty(snapshot).context("failed to serialize cache snapshot")?;
    fs::write(&temp_path, contents)
        .with_context(|| format!("failed to write temporary cache {}", temp_path.display()))?;
    fs::rename(&temp_path, &path).with_context(|| {
        format!(
            "failed to move temporary cache {} into place at {}",
            temp_path.display(),
            path.display()
        )
    })?;

    Ok(())
}

pub(super) fn refresh_existing_cache_from_tmux() -> Result<()> {
    let path = cache_path()?;
    if !path.exists() {
        return Ok(());
    }

    let existing = read_existing_snapshot_if_valid();
    let mut snapshot = snapshot_from_tmux()?;
    preserve_last_daemon_refresh(&mut snapshot, existing.as_ref());
    write_snapshot_to_cache(&snapshot)
}

pub(super) fn read_existing_snapshot_if_valid() -> Option<SnapshotEnvelope> {
    let path = cache_path().ok()?;
    path.exists().then_some(())?;
    read_snapshot_from_cache().ok()
}

fn preserve_last_daemon_refresh(
    snapshot: &mut SnapshotEnvelope,
    existing: Option<&SnapshotEnvelope>,
) {
    snapshot.source.daemon_generated_at = existing
        .and_then(last_daemon_generated_at)
        .map(str::to_string);
}

fn last_daemon_generated_at(snapshot: &SnapshotEnvelope) -> Option<&str> {
    snapshot.source.daemon_generated_at.as_deref().or_else(|| {
        (snapshot.source.kind == SourceKind::Daemon).then_some(snapshot.generated_at.as_str())
    })
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

pub(crate) fn popup_entries(panes: &[PaneRecord]) -> Vec<PopupEntry> {
    panes
        .iter()
        .map(|pane| PopupEntry {
            pane_id: pane.pane_id.clone(),
            provider: pane.provider,
            status: pane.status.kind,
            location_tag: pane.location.tag(),
            session_name: pane.location.session_name.clone(),
            window_index: pane.location.window_index,
            pane_index: pane.location.pane_index,
            display_label: pane.display.label.clone(),
        })
        .collect()
}

fn now_rfc3339() -> Result<String> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .context("failed to format current time")
}

pub(crate) fn validate_snapshot(
    snapshot: &SnapshotEnvelope,
    max_age_seconds: Option<u64>,
) -> Result<CacheSummary> {
    if snapshot.schema_version != CACHE_SCHEMA_VERSION {
        bail!(
            "unsupported cache schema version {} (expected {})",
            snapshot.schema_version,
            CACHE_SCHEMA_VERSION
        );
    }

    let summary = summarize_snapshot(snapshot)?;
    if let Some(max_age_seconds) = max_age_seconds {
        let age_seconds = cache_age_seconds(summary.generated_at);
        if age_seconds > max_age_seconds {
            bail!(
                "cache is stale: age {}s exceeds max {}s",
                age_seconds,
                max_age_seconds
            );
        }
    }

    Ok(summary)
}

pub(crate) fn summarize_snapshot(snapshot: &SnapshotEnvelope) -> Result<CacheSummary> {
    let generated_at = OffsetDateTime::parse(&snapshot.generated_at, &Rfc3339)
        .context("generated_at was not valid RFC3339")?;

    let pane_count = snapshot.panes.len();
    let agent_pane_count = snapshot
        .panes
        .iter()
        .filter(|pane| pane.provider.is_some())
        .count();

    let provider_counts = [
        Provider::Codex,
        Provider::Claude,
        Provider::Gemini,
        Provider::Opencode,
    ]
    .into_iter()
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

    Ok(CacheSummary {
        generated_at,
        pane_count,
        agent_pane_count,
        provider_counts,
        status_counts,
    })
}

pub(crate) fn cache_age_seconds(generated_at: OffsetDateTime) -> u64 {
    let age_seconds = (OffsetDateTime::now_utc() - generated_at).whole_seconds();
    if age_seconds.is_negative() {
        0
    } else {
        age_seconds as u64
    }
}

pub(crate) fn daemon_cache_status(
    age_seconds: Option<u64>,
    max_age_seconds: Option<u64>,
) -> DaemonCacheStatus {
    let Some(age_seconds) = age_seconds else {
        return DaemonCacheStatus::Unavailable;
    };

    if max_age_seconds.is_some_and(|max_age| age_seconds > max_age) {
        return DaemonCacheStatus::Stale;
    }

    DaemonCacheStatus::Healthy
}

pub(crate) fn daemon_age_seconds(snapshot: &SnapshotEnvelope) -> Result<Option<u64>> {
    let Some(generated_at) = last_daemon_generated_at(snapshot) else {
        return Ok(None);
    };

    let generated_at = OffsetDateTime::parse(generated_at, &Rfc3339)
        .context("daemon_generated_at was not valid RFC3339")?;
    Ok(Some(cache_age_seconds(generated_at)))
}

pub(super) fn set_snapshot_cache_origin(snapshot: &mut SnapshotEnvelope, cache_origin: &str) {
    for pane in &mut snapshot.panes {
        pane.diagnostics.cache_origin = cache_origin.to_string();
    }
}

pub(crate) fn cache_path() -> Result<PathBuf> {
    if let Some(path) = env::var_os(CACHE_ENV_VAR) {
        return Ok(PathBuf::from(path));
    }

    if let Some(cache_home) = env::var_os("XDG_CACHE_HOME") {
        return Ok(PathBuf::from(cache_home).join(CACHE_RELATIVE_PATH));
    }

    let home = env::var_os("HOME").context("HOME is not set and no cache override was provided")?;
    Ok(Path::new(&home).join(".cache").join(CACHE_RELATIVE_PATH))
}
