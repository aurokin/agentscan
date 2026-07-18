use super::*;
use serde_json::{Map, Value, json};

/// A daemon snapshot older than this (seconds) is reported as stale by
/// `discovery.compare`.
const STALE_THRESHOLD_SECONDS: i64 = 60;

pub(super) fn binary_version_check() -> DoctorCheck {
    let version = env!("CARGO_PKG_VERSION");
    let executable = env::current_exe().ok();
    let canonical = executable
        .as_ref()
        .and_then(|path| fs::canonicalize(path).ok());
    let details = json!({
        "version": version,
        "executable": executable.as_ref().map(|path| path.display().to_string()),
        "executable_canonical": canonical.as_ref().map(|path| path.display().to_string()),
    });
    DoctorCheck::new(
        "binary.version",
        CheckStatus::Info,
        format!("agentscan {version}"),
        Some(details),
    )
}

#[cfg(target_os = "macos")]
pub(super) fn macos_trust_check() -> DoctorCheck {
    let Ok(path) = env::current_exe() else {
        return DoctorCheck::new(
            "binary.macos_trust",
            CheckStatus::Warn,
            "could not resolve the current executable to assess trust".to_string(),
            None,
        );
    };
    match daemon::assess_macos_executable_for_daemon_autostart(&path) {
        daemon::MacExecutableAssessment::Trusted => DoctorCheck::new(
            "binary.macos_trust",
            CheckStatus::Ok,
            "executable is trusted for detached daemon auto-start".to_string(),
            Some(json!({ "executable": path.display().to_string() })),
        ),
        daemon::MacExecutableAssessment::Untrusted(reason) => DoctorCheck::new(
            "binary.macos_trust",
            CheckStatus::Warn,
            format!("detached daemon auto-start would be refused: {reason}"),
            Some(json!({ "executable": path.display().to_string(), "reason": reason })),
        ),
    }
}

#[cfg(not(target_os = "macos"))]
pub(super) fn macos_trust_check() -> DoctorCheck {
    DoctorCheck::new(
        "binary.macos_trust",
        CheckStatus::Info,
        "macOS executable-trust check is not applicable on this platform".to_string(),
        None,
    )
}

pub(super) fn config_check() -> DoctorCheck {
    // The icon/picker portions and `disable_reconcile`/`disable_proc_fallback`
    // (and their env overrides) are validated from a single load of the config
    // file. Both must pass, or a command that reads either would reject the same
    // file — so neither error may be swallowed here.
    let (resolved, runtime) =
        match config::resolve_config_and_runtime_options(CliConfigOverrides::default()) {
            Ok(views) => views,
            Err(error) => return config_fail(error),
        };
    let key_order: String = resolved.picker_keys.keys().iter().collect();
    let details = json!({
        "config_path": resolved.config_path.as_ref().map(|path| path.display().to_string()),
        "icons": resolved.icons.as_str(),
        "picker_key_order": key_order,
        "picker_key_count": resolved.picker_keys.len(),
        "disable_reconcile": runtime.disable_reconcile,
        "disable_proc_fallback": runtime.disable_proc_fallback,
    });
    DoctorCheck::new(
        "config.valid",
        CheckStatus::Ok,
        "configuration parsed successfully".to_string(),
        Some(details),
    )
}

fn config_fail(error: anyhow::Error) -> DoctorCheck {
    DoctorCheck::new(
        "config.valid",
        CheckStatus::Fail,
        format!("configuration error: {error:#}"),
        None,
    )
}

pub(super) fn tmux_check() -> DoctorCheck {
    let inside_tmux = env::var_os("TMUX").is_some();
    let harness_socket = env::var_os(TMUX_SOCKET_ENV_VAR).is_some();
    match tmux::tmux_version() {
        Some(version) => {
            let supports_subscriptions = tmux::tmux_version_supports_subscriptions(&version);
            let (status, message) = match supports_subscriptions {
                Some(false) => (
                    CheckStatus::Warn,
                    format!(
                        "tmux {version} is reachable but older than 3.2; live pane updates require \
                         tmux 3.2+ (control-mode `refresh-client -B` subscriptions). One-shot \
                         snapshots still work — upgrade tmux for live status."
                    ),
                ),
                // `Some(true)` is healthy; `None` means the version string was
                // unparseable, so we don't warn rather than risk a false alarm.
                _ => (CheckStatus::Ok, format!("tmux {version} is reachable")),
            };
            DoctorCheck::new(
                "tmux.reachable",
                status,
                message,
                Some(json!({
                    "version": version,
                    "min_subscription_version": "3.2",
                    "supports_subscriptions": supports_subscriptions,
                    "inside_tmux": inside_tmux,
                    "agentscan_tmux_socket": harness_socket,
                })),
            )
        }
        None => DoctorCheck::new(
            "tmux.reachable",
            CheckStatus::Fail,
            "tmux is not reachable (is tmux installed and on PATH?)".to_string(),
            Some(json!({
                "inside_tmux": inside_tmux,
                "agentscan_tmux_socket": harness_socket,
            })),
        ),
    }
}

pub(super) fn daemon_health_check(include_events: bool) -> DoctorCheck {
    let socket_path = match ipc::resolve_socket_path() {
        Ok(path) => path,
        Err(error) => {
            return DoctorCheck::new(
                "daemon.health",
                CheckStatus::Warn,
                format!("could not resolve the daemon socket path: {error:#}"),
                None,
            );
        }
    };
    match daemon::query_lifecycle_status(&socket_path) {
        Ok(query) => daemon_health_from_query(query, &socket_path, include_events),
        Err(error) => DoctorCheck::new(
            "daemon.health",
            CheckStatus::Warn,
            format!("could not query the daemon: {error:#}"),
            Some(json!({ "socket_path": socket_path.display().to_string() })),
        ),
    }
}

fn daemon_health_from_query(
    query: daemon::LifecycleQuery,
    socket_path: &Path,
    include_events: bool,
) -> DoctorCheck {
    match query {
        daemon::LifecycleQuery::NotRunning(reason) => DoctorCheck::new(
            "daemon.health",
            CheckStatus::Warn,
            format!("daemon is not running: {reason}"),
            Some(json!({
                "daemon_state": "not_running",
                "socket_path": socket_path.display().to_string(),
            })),
        ),
        daemon::LifecycleQuery::Status(status) => {
            daemon_health_from_status(&status, include_events)
        }
        daemon::LifecycleQuery::Incompatible { message, .. } => DoctorCheck::new(
            "daemon.health",
            CheckStatus::Fail,
            format!(
                "incompatible daemon detected: {message}; stop it and run `agentscan daemon start`"
            ),
            Some(json!({ "socket_path": socket_path.display().to_string() })),
        ),
        daemon::LifecycleQuery::Busy(message) => DoctorCheck::new(
            "daemon.health",
            CheckStatus::Warn,
            message,
            Some(json!({ "socket_path": socket_path.display().to_string() })),
        ),
    }
}

fn daemon_health_from_status(
    status: &ipc::LifecycleStatusFrame,
    include_events: bool,
) -> DoctorCheck {
    let (check_status, message) = match status.state {
        ipc::LifecycleDaemonState::Ready => (CheckStatus::Ok, "daemon is ready".to_string()),
        ipc::LifecycleDaemonState::Initializing => (
            CheckStatus::Warn,
            "daemon is still initializing".to_string(),
        ),
        ipc::LifecycleDaemonState::Closing => {
            (CheckStatus::Warn, "daemon is shutting down".to_string())
        }
        ipc::LifecycleDaemonState::StartupFailed => (
            CheckStatus::Fail,
            status
                .message
                .clone()
                .unwrap_or_else(|| "daemon startup failed".to_string()),
        ),
    };
    let mut details = json!({
        "daemon_state": status.state,
        "pid": status.identity.pid,
        "socket_path": status.identity.socket_path,
        "executable": status.identity.executable,
        "executable_canonical": status.identity.executable_canonical,
        "protocol_version": status.identity.protocol_version,
        "snapshot_schema_version": status.identity.snapshot_schema_version,
        "subscriber_count": status.subscriber_count,
        "latest_snapshot_generated_at": status.latest_snapshot_generated_at,
        "latest_snapshot_age_seconds": status
            .latest_snapshot_generated_at
            .as_deref()
            .and_then(rfc3339_age_seconds),
        "latest_snapshot_pane_count": status.latest_snapshot_pane_count,
        "control_mode_broker_mode": status.control_mode_broker.as_ref().map(|broker| broker.mode),
        "control_mode_broker_subscriber_coverage_complete": status
            .control_mode_broker
            .as_ref()
            .and_then(|broker| broker.subscriber_coverage_complete),
    });
    if include_events {
        details["recent_events"] = json!(status.recent_events);
    }
    DoctorCheck::new("daemon.health", check_status, message, Some(details))
}

pub(super) fn discovery_summary_check(
    primary: Result<&SnapshotEnvelope, &str>,
    refresh: bool,
) -> DoctorCheck {
    match primary {
        Ok(snapshot) => {
            let total = snapshot.panes.len();
            let agents = agent_pane_count(snapshot);
            let source = match snapshot.source.kind {
                SourceKind::Daemon => "daemon",
                SourceKind::Snapshot => "direct_tmux",
            };
            let details = json!({
                "source": source,
                "generated_at": snapshot.generated_at,
                "pane_count": total,
                "agent_pane_count": agents,
                "provider_counts": provider_counts_json(snapshot),
                "status_kind_counts": status_kind_counts_json(snapshot),
                "status_source_counts": status_source_counts_json(snapshot),
            });
            DoctorCheck::new(
                "discovery.summary",
                CheckStatus::Ok,
                format!("{agents} agent pane(s) detected of {total} pane(s) total"),
                Some(details),
            )
        }
        Err(reason) if refresh => DoctorCheck::new(
            "discovery.summary",
            CheckStatus::Warn,
            format!("could not read a direct tmux snapshot: {reason}"),
            None,
        ),
        Err(reason) => DoctorCheck::new(
            "discovery.summary",
            CheckStatus::Info,
            format!(
                "no daemon snapshot available ({reason}); run with --refresh for a direct tmux snapshot"
            ),
            None,
        ),
    }
}

pub(super) fn discovery_compare_check(
    direct: &SnapshotEnvelope,
    daemon: &SnapshotEnvelope,
) -> DoctorCheck {
    let direct_panes = direct.panes.len();
    let daemon_panes = daemon.panes.len();
    let direct_agents = agent_pane_count(direct);
    let daemon_agents = agent_pane_count(daemon);
    // Compare pane-id sets, not just counts: a stale pane replaced by a new one
    // keeps the counts equal while the contents differ.
    let only_in_direct = pane_ids_only_in(direct, daemon);
    let only_in_daemon = pane_ids_only_in(daemon, direct);
    // For panes present in both, compare provider identity. Provider classification
    // is stable across reads, so a disagreement means the daemon is carrying stale
    // metadata for an existing pane — the case a count/id-set check alone misses.
    // Transient busy/idle status is deliberately excluded: it legitimately differs
    // between two reads taken moments apart, and per-field diffing belongs to the
    // daemon's own snapshot diff surfaced by `daemon status`.
    let provider_mismatch = provider_mismatched_pane_ids(direct, daemon);
    let age = rfc3339_age_seconds(&daemon.generated_at);
    let stale = age.is_some_and(|seconds| seconds > STALE_THRESHOLD_SECONDS);
    let count_mismatch = direct_panes != daemon_panes
        || direct_agents != daemon_agents
        || !only_in_direct.is_empty()
        || !only_in_daemon.is_empty();
    let details = json!({
        "direct_pane_count": direct_panes,
        "daemon_pane_count": daemon_panes,
        "direct_agent_pane_count": direct_agents,
        "daemon_agent_pane_count": daemon_agents,
        "pane_ids_only_in_direct": only_in_direct,
        "pane_ids_only_in_daemon": only_in_daemon,
        "pane_ids_provider_mismatch": provider_mismatch,
        "daemon_snapshot_age_seconds": age,
        "stale_threshold_seconds": STALE_THRESHOLD_SECONDS,
        "stale": stale,
    });
    let (status, message) = if count_mismatch {
        (
            CheckStatus::Warn,
            format!(
                "daemon and direct snapshots disagree (daemon {daemon_panes} pane(s)/{daemon_agents} agent(s) vs direct {direct_panes} pane(s)/{direct_agents} agent(s))"
            ),
        )
    } else if !provider_mismatch.is_empty() {
        (
            CheckStatus::Warn,
            format!(
                "daemon snapshot has stale provider metadata for {} pane(s)",
                provider_mismatch.len()
            ),
        )
    } else if stale {
        (
            CheckStatus::Warn,
            "daemon snapshot matches the direct read but is stale".to_string(),
        )
    } else {
        (
            CheckStatus::Ok,
            "daemon snapshot agrees with the direct tmux read".to_string(),
        )
    };
    DoctorCheck::new("discovery.compare", status, message, Some(details))
}

/// Pane ids present in `left` but not in `right`, sorted for stable output.
fn pane_ids_only_in(left: &SnapshotEnvelope, right: &SnapshotEnvelope) -> Vec<String> {
    let right_ids: std::collections::HashSet<&str> = right
        .panes
        .iter()
        .map(|pane| pane.pane_id.as_str())
        .collect();
    let mut ids: Vec<String> = left
        .panes
        .iter()
        .filter(|pane| !right_ids.contains(pane.pane_id.as_str()))
        .map(|pane| pane.pane_id.clone())
        .collect();
    ids.sort();
    ids
}

/// Pane ids present in both snapshots whose provider identity disagrees, sorted
/// for stable output.
fn provider_mismatched_pane_ids(
    direct: &SnapshotEnvelope,
    daemon: &SnapshotEnvelope,
) -> Vec<String> {
    let daemon_providers: std::collections::HashMap<&str, Option<Provider>> = daemon
        .panes
        .iter()
        .map(|pane| (pane.pane_id.as_str(), pane.provider))
        .collect();
    let mut ids: Vec<String> = direct
        .panes
        .iter()
        .filter(|pane| {
            daemon_providers
                .get(pane.pane_id.as_str())
                .is_some_and(|daemon_provider| *daemon_provider != pane.provider)
        })
        .map(|pane| pane.pane_id.clone())
        .collect();
    ids.sort();
    ids
}

pub(super) fn picker_contract_check(snapshot: Option<&SnapshotEnvelope>) -> DoctorCheck {
    let picker_config = match config::resolve_picker_config() {
        Ok(config) => config,
        Err(error) => {
            return DoctorCheck::new(
                "picker.contract",
                CheckStatus::Info,
                format!("could not resolve picker config: {error:#}"),
                None,
            );
        }
    };
    let picker_keys = picker_config.picker_keys;
    let key_order: String = picker_keys.keys().iter().collect();
    let capacity = picker_keys.len();
    let Some(snapshot) = snapshot else {
        return DoctorCheck::new(
            "picker.contract",
            CheckStatus::Info,
            "no snapshot available to build the picker".to_string(),
            Some(json!({ "key_order": key_order, "key_capacity": capacity })),
        );
    };
    let agent_panes: Vec<PaneRecord> = snapshot
        .panes
        .iter()
        .filter(|pane| pane.provider.is_some())
        .cloned()
        .collect();
    let focus = tmux::tmux_focus_state().unwrap_or_default();
    let rows = picker::picker_rows(
        &agent_panes,
        focus.focused_session.as_deref(),
        u32::try_from(focus.attached_client_count).unwrap_or(u32::MAX),
        picker_config.picker_group_by,
        &picker_keys,
    );
    // `picker_rows` zips panes with keys, so it assigns at most `capacity` rows.
    // When agent panes exceed the key set, the surplus panes get no hotkey and
    // cannot be selected from the flat picker — flag that rather than report a
    // clean assignment.
    let agent_count = agent_panes.len();
    let assigned = rows.len();
    let unassigned = agent_count.saturating_sub(assigned);
    let details = json!({
        "key_order": key_order,
        "key_capacity": capacity,
        "agent_pane_count": agent_count,
        "rows_assigned": assigned,
        "unassigned_agent_panes": unassigned,
    });
    let (status, message) = picker_assignment_status(capacity, assigned, unassigned);
    DoctorCheck::new("picker.contract", status, message, Some(details))
}

fn picker_assignment_status(
    capacity: usize,
    assigned: usize,
    unassigned: usize,
) -> (CheckStatus, String) {
    if unassigned > 0 {
        (
            CheckStatus::Warn,
            format!(
                "{unassigned} agent pane(s) exceed the {capacity} picker slots and cannot be selected"
            ),
        )
    } else {
        (
            CheckStatus::Ok,
            format!("{assigned} of {capacity} picker slot(s) assigned"),
        )
    }
}

fn agent_pane_count(snapshot: &SnapshotEnvelope) -> usize {
    snapshot
        .panes
        .iter()
        .filter(|pane| pane.provider.is_some())
        .count()
}

fn provider_counts_json(snapshot: &SnapshotEnvelope) -> Value {
    let mut counts = Map::new();
    for provider in provider_summary_order() {
        let count = snapshot
            .panes
            .iter()
            .filter(|pane| pane.provider == Some(provider))
            .count();
        if count > 0 {
            counts.insert(provider.to_string(), json!(count));
        }
    }
    Value::Object(counts)
}

fn status_kind_counts_json(snapshot: &SnapshotEnvelope) -> Value {
    let mut busy = 0;
    let mut waiting = 0;
    let mut idle = 0;
    let mut unknown = 0;
    for pane in &snapshot.panes {
        match pane.status.kind {
            StatusKind::Busy => busy += 1,
            StatusKind::Waiting => waiting += 1,
            StatusKind::Idle => idle += 1,
            StatusKind::Unknown => unknown += 1,
        }
    }
    json!({ "busy": busy, "waiting": waiting, "idle": idle, "unknown": unknown })
}

fn status_source_counts_json(snapshot: &SnapshotEnvelope) -> Value {
    let mut pane_metadata = 0;
    let mut tmux_title = 0;
    let mut pane_output = 0;
    let mut not_checked = 0;
    for pane in &snapshot.panes {
        match pane.status.source {
            StatusSource::PaneMetadata => pane_metadata += 1,
            StatusSource::TmuxTitle => tmux_title += 1,
            StatusSource::PaneOutput => pane_output += 1,
            StatusSource::NotChecked => not_checked += 1,
        }
    }
    json!({
        "pane_metadata": pane_metadata,
        "tmux_title": tmux_title,
        "pane_output": pane_output,
        "not_checked": not_checked,
    })
}

fn rfc3339_age_seconds(timestamp: &str) -> Option<i64> {
    let parsed = OffsetDateTime::parse(timestamp, &Rfc3339).ok()?;
    Some((OffsetDateTime::now_utc() - parsed).whole_seconds())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_running_daemon_maps_to_warn() {
        let socket = PathBuf::from("/tmp/agentscan-test.sock");
        let result = daemon_health_from_query(
            daemon::LifecycleQuery::NotRunning("no socket".to_string()),
            &socket,
            false,
        );
        assert_eq!(result.id, "daemon.health");
        assert_eq!(result.status, CheckStatus::Warn);
    }

    #[test]
    fn incompatible_daemon_maps_to_fail() {
        let socket = PathBuf::from("/tmp/agentscan-test.sock");
        let result = daemon_health_from_query(
            daemon::LifecycleQuery::Incompatible {
                message: "protocol mismatch".to_string(),
                peer_pid: Some(42),
                can_signal: true,
            },
            &socket,
            false,
        );
        assert_eq!(result.status, CheckStatus::Fail);
    }

    #[test]
    fn busy_daemon_maps_to_warn() {
        let socket = PathBuf::from("/tmp/agentscan-test.sock");
        let result = daemon_health_from_query(
            daemon::LifecycleQuery::Busy("server busy".to_string()),
            &socket,
            false,
        );
        assert_eq!(result.status, CheckStatus::Warn);
    }

    #[test]
    fn status_source_tally_counts_each_kind() {
        let mut snapshot = empty_snapshot();
        snapshot.panes.push(pane_with_status(
            "%1",
            PaneStatus::metadata(StatusKind::Busy),
            Some(Provider::Codex),
        ));
        snapshot.panes.push(pane_with_status(
            "%2",
            PaneStatus::title(StatusKind::Idle),
            Some(Provider::Claude),
        ));
        snapshot.panes.push(pane_with_status(
            "%3",
            PaneStatus::pane_output(StatusKind::Busy),
            Some(Provider::Codex),
        ));
        snapshot.panes.push(pane_with_status(
            "%5",
            PaneStatus::pane_output(StatusKind::Waiting),
            Some(Provider::Opencode),
        ));
        snapshot
            .panes
            .push(pane_with_status("%4", PaneStatus::not_checked(), None));

        let tally = status_source_counts_json(&snapshot);
        assert_eq!(tally["pane_metadata"], json!(1));
        assert_eq!(tally["tmux_title"], json!(1));
        assert_eq!(tally["pane_output"], json!(2));
        assert_eq!(tally["not_checked"], json!(1));

        let providers = provider_counts_json(&snapshot);
        assert_eq!(providers["codex"], json!(2));
        assert_eq!(providers["claude"], json!(1));
        assert_eq!(providers["opencode"], json!(1));
        assert_eq!(agent_pane_count(&snapshot), 4);

        let status_kinds = status_kind_counts_json(&snapshot);
        assert_eq!(status_kinds["busy"], json!(2));
        assert_eq!(status_kinds["waiting"], json!(1));
        assert_eq!(status_kinds["idle"], json!(1));
        assert_eq!(status_kinds["unknown"], json!(1));
    }

    #[test]
    fn compare_warns_when_pane_sets_differ_at_equal_counts() {
        // Same pane count and agent count, but a stale pane (%1) was replaced by a
        // new one (%2): a count-only comparison would report a false agreement.
        let mut direct = empty_snapshot();
        direct.panes.push(pane_with_status(
            "%2",
            PaneStatus::metadata(StatusKind::Idle),
            Some(Provider::Codex),
        ));
        let mut daemon = empty_snapshot();
        daemon.generated_at = snapshot::now_rfc3339().expect("rfc3339 timestamp");
        daemon.panes.push(pane_with_status(
            "%1",
            PaneStatus::metadata(StatusKind::Idle),
            Some(Provider::Codex),
        ));

        let check = discovery_compare_check(&direct, &daemon);
        assert_eq!(check.status, CheckStatus::Warn);
        let details = check.details.expect("compare details");
        assert_eq!(details["pane_ids_only_in_direct"], json!(["%2"]));
        assert_eq!(details["pane_ids_only_in_daemon"], json!(["%1"]));
    }

    #[test]
    fn compare_warns_on_stale_provider_metadata_for_existing_pane() {
        // Same pane id, same counts, fresh timestamp — but the daemon still has the
        // pane unclassified while the direct read sees a provider.
        let mut direct = empty_snapshot();
        direct.panes.push(pane_with_status(
            "%1",
            PaneStatus::metadata(StatusKind::Idle),
            Some(Provider::Codex),
        ));
        let mut daemon = empty_snapshot();
        daemon.generated_at = snapshot::now_rfc3339().expect("rfc3339 timestamp");
        daemon
            .panes
            .push(pane_with_status("%1", PaneStatus::not_checked(), None));

        let check = discovery_compare_check(&direct, &daemon);
        assert_eq!(check.status, CheckStatus::Warn);
        let details = check.details.expect("compare details");
        assert_eq!(details["pane_ids_provider_mismatch"], json!(["%1"]));
        // Counts and id-sets agree; only the per-pane provider disagrees.
        assert_eq!(details["pane_ids_only_in_direct"], json!([] as [&str; 0]));
        assert_eq!(details["pane_ids_only_in_daemon"], json!([] as [&str; 0]));
    }

    #[test]
    fn picker_status_warns_on_overflow_and_ok_when_within_capacity() {
        // Overflow: 16 keys, 18 agent panes, 16 assignable → 2 cannot be selected.
        let (status, message) = picker_assignment_status(16, 16, 2);
        assert_eq!(status, CheckStatus::Warn);
        assert!(message.contains("2 agent pane(s) exceed"), "got: {message}");

        // Within capacity: no unassigned panes.
        let (status, message) = picker_assignment_status(16, 6, 0);
        assert_eq!(status, CheckStatus::Ok);
        assert!(message.contains("6 of 16"), "got: {message}");
    }

    #[test]
    fn compare_ok_when_fresh_snapshots_match() {
        let mut direct = empty_snapshot();
        direct.panes.push(pane_with_status(
            "%1",
            PaneStatus::metadata(StatusKind::Idle),
            Some(Provider::Codex),
        ));
        let mut daemon = empty_snapshot();
        daemon.generated_at = snapshot::now_rfc3339().expect("rfc3339 timestamp");
        daemon.panes.push(pane_with_status(
            "%1",
            PaneStatus::metadata(StatusKind::Idle),
            Some(Provider::Codex),
        ));

        assert_eq!(
            discovery_compare_check(&direct, &daemon).status,
            CheckStatus::Ok
        );
    }

    fn empty_snapshot() -> SnapshotEnvelope {
        SnapshotEnvelope {
            schema_version: CACHE_SCHEMA_VERSION,
            generated_at: "2026-06-06T00:00:00Z".to_string(),
            source: SnapshotSource {
                kind: SourceKind::Snapshot,
                tmux_version: None,
                daemon_generated_at: None,
            },
            panes: Vec::new(),
        }
    }

    fn pane_with_status(
        pane_id: &str,
        status: PaneStatus,
        provider: Option<Provider>,
    ) -> PaneRecord {
        PaneRecord {
            pane_id: pane_id.to_string(),
            location: PaneLocation {
                session_name: "session".to_string(),
                window_index: 0,
                pane_index: 0,
                window_name: "window".to_string(),
            },
            tmux: TmuxPaneMetadata {
                pane_pid: 0,
                pane_tty: "/dev/null".to_string(),
                pane_current_path: "/".to_string(),
                pane_current_command: "zsh".to_string(),
                pane_title_raw: String::new(),
                session_id: None,
                window_id: None,
                pane_active: false,
                window_active: false,
            },
            display: DisplayMetadata {
                label: "label".to_string(),
                activity_label: None,
            },
            provider,
            status,
            classification: PaneClassification {
                matched_by: None,
                confidence: None,
                reasons: Vec::new(),
            },
            agent_metadata: AgentMetadata::default(),
            diagnostics: PaneDiagnostics {
                cache_origin: "test".to_string(),
                proc_fallback: ProcFallbackDiagnostics::default(),
            },
            last_focus_seq: None,
        }
    }
}
