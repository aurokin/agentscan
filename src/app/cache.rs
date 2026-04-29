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

pub(super) fn refresh_cache_from_tmux() -> Result<SnapshotEnvelope> {
    let existing = read_existing_snapshot_if_valid();
    let mut snapshot = scanner::snapshot_from_tmux()?;
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

    let temp_path = unique_cache_temp_path(&path);
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

fn unique_cache_temp_path(path: &Path) -> PathBuf {
    let sequence = CACHE_WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut extension = match path.extension().and_then(|value| value.to_str()) {
        Some(existing) if !existing.is_empty() => {
            format!("{existing}.tmp.{}.{}", std::process::id(), sequence)
        }
        _ => format!("tmp.{}.{}", std::process::id(), sequence),
    };
    if extension.is_empty() {
        extension = format!("tmp.{}.{}", std::process::id(), sequence);
    }
    path.with_extension(extension)
}

pub(super) fn refresh_existing_cache_from_tmux() -> Result<()> {
    let path = cache_path()?;
    if !path.exists() {
        return Ok(());
    }

    let existing = read_existing_snapshot_if_valid();
    let mut snapshot = scanner::snapshot_from_tmux()?;
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

pub(crate) fn now_rfc3339() -> Result<String> {
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

pub(crate) fn cache_diagnostics(
    snapshot: &SnapshotEnvelope,
    max_age_seconds: Option<u64>,
) -> Result<CacheDiagnostics> {
    let summary = summarize_snapshot(snapshot)?;
    let cache_age_seconds = cache_age_seconds(summary.generated_at);
    let daemon_timestamp = daemon_timestamp_diagnostics(snapshot)?;
    let daemon_age_seconds = daemon_timestamp.age_seconds();
    let daemon_cache_status = if daemon_timestamp.is_malformed() {
        DaemonCacheStatus::Unavailable
    } else {
        daemon_cache_status(snapshot.source.kind, daemon_age_seconds, max_age_seconds)
    };
    let daemon_status_reason =
        daemon_status_reason(snapshot.source.kind, daemon_cache_status, max_age_seconds);

    Ok(CacheDiagnostics {
        cache_age_seconds,
        daemon_age_seconds,
        daemon_cache_status,
        daemon_status_reason,
    })
}

enum DaemonTimestampDiagnostics {
    Missing,
    Malformed,
    Present(u64),
}

impl DaemonTimestampDiagnostics {
    fn age_seconds(&self) -> Option<u64> {
        match self {
            Self::Present(age_seconds) => Some(*age_seconds),
            Self::Missing | Self::Malformed => None,
        }
    }

    fn is_malformed(&self) -> bool {
        matches!(self, Self::Malformed)
    }
}

pub(crate) fn daemon_cache_status(
    source_kind: SourceKind,
    age_seconds: Option<u64>,
    max_age_seconds: Option<u64>,
) -> DaemonCacheStatus {
    let Some(age_seconds) = age_seconds else {
        return match source_kind {
            SourceKind::Snapshot => DaemonCacheStatus::SnapshotOnly,
            SourceKind::Daemon => DaemonCacheStatus::Unavailable,
        };
    };

    if max_age_seconds.is_some_and(|max_age| age_seconds > max_age) {
        return DaemonCacheStatus::Stale;
    }

    DaemonCacheStatus::Healthy
}

pub(crate) fn daemon_status_reason(
    source_kind: SourceKind,
    status: DaemonCacheStatus,
    max_age_seconds: Option<u64>,
) -> String {
    match status {
        DaemonCacheStatus::Healthy => match source_kind {
            SourceKind::Daemon => "cache was last written by the daemon".to_string(),
            SourceKind::Snapshot => {
                "cache was last refreshed directly from tmux and preserves the previous daemon refresh time".to_string()
            }
        },
        DaemonCacheStatus::Stale => {
            let age_clause = max_age_seconds
                .map(|value| format!("older than the {}s threshold", value))
                .unwrap_or_else(|| "older than the allowed threshold".to_string());
            match source_kind {
                SourceKind::Daemon => format!("last daemon refresh is {age_clause}"),
                SourceKind::Snapshot => {
                    format!("cache was last refreshed directly from tmux, but the last daemon refresh is {age_clause}")
                }
            }
        }
        DaemonCacheStatus::SnapshotOnly => {
            "cache was written from a direct tmux snapshot and does not include a daemon refresh timestamp".to_string()
        }
        DaemonCacheStatus::Unavailable => {
            "cache does not include a usable daemon refresh timestamp".to_string()
        }
    }
}

fn daemon_timestamp_diagnostics(snapshot: &SnapshotEnvelope) -> Result<DaemonTimestampDiagnostics> {
    let Some(generated_at) = last_daemon_generated_at(snapshot) else {
        return Ok(DaemonTimestampDiagnostics::Missing);
    };

    let generated_at = match OffsetDateTime::parse(generated_at, &Rfc3339) {
        Ok(generated_at) => generated_at,
        Err(_) => return Ok(DaemonTimestampDiagnostics::Malformed),
    };
    Ok(DaemonTimestampDiagnostics::Present(cache_age_seconds(
        generated_at,
    )))
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
