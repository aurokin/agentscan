use std::collections::HashMap;

#[derive(Default)]
struct FakeTmuxReadProvider {
    all_panes: Vec<super::TmuxPaneRow>,
    target_panes: HashMap<String, Option<Vec<super::TmuxPaneRow>>>,
    pane_rows: HashMap<String, Option<super::TmuxPaneRow>>,
    list_all_count: usize,
    list_target_count: usize,
    list_pane_count: usize,
}

impl FakeTmuxReadProvider {
    fn with_all_panes(mut self, rows: Vec<super::TmuxPaneRow>) -> Self {
        self.all_panes = rows;
        self
    }

    fn with_target_panes(
        mut self,
        target: &str,
        rows: Option<Vec<super::TmuxPaneRow>>,
    ) -> Self {
        self.target_panes.insert(target.to_string(), rows);
        self
    }

    fn with_pane(mut self, pane_id: &str, row: Option<super::TmuxPaneRow>) -> Self {
        self.pane_rows.insert(pane_id.to_string(), row);
        self
    }
}

impl daemon::TmuxReadProvider for FakeTmuxReadProvider {
    fn list_all_panes(&mut self) -> anyhow::Result<Vec<super::TmuxPaneRow>> {
        self.list_all_count += 1;
        Ok(self.all_panes.clone())
    }

    fn list_target_panes(
        &mut self,
        target: &str,
    ) -> anyhow::Result<Option<Vec<super::TmuxPaneRow>>> {
        self.list_target_count += 1;
        Ok(self.target_panes.get(target).cloned().unwrap_or(None))
    }

    fn list_pane(&mut self, pane_id: &str) -> anyhow::Result<Option<super::TmuxPaneRow>> {
        self.list_pane_count += 1;
        Ok(self.pane_rows.get(pane_id).cloned().unwrap_or(None))
    }
}

fn daemon_refresh_row(
    pane_id: &str,
    session_id: &str,
    window_id: &str,
    pane_index: u32,
    title: &str,
) -> super::TmuxPaneRow {
    let window_index = window_id
        .trim_start_matches('@')
        .parse::<u32>()
        .expect("window id should be numeric");
    super::TmuxPaneRow {
        session_name: format!("session-{session_id}"),
        window_index,
        pane_index,
        pane_id: pane_id.to_string(),
        pane_pid: 42_000 + pane_index,
        pane_current_command: "codex".to_string(),
        pane_title_raw: title.to_string(),
        pane_tty: format!("/dev/ttys{pane_index}"),
        pane_current_path: "/tmp/agentscan".to_string(),
        window_name: format!("window-{window_id}"),
        session_id: Some(session_id.to_string()),
        window_id: Some(window_id.to_string()),
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
        pane_active: false,
        window_active: false,
    }
}

fn daemon_refresh_snapshot(rows: Vec<super::TmuxPaneRow>) -> SnapshotEnvelope {
    let mut snapshot = empty_socket_snapshot("2026-05-03T00:00:00Z");
    snapshot.panes = rows.into_iter().map(classify::pane_from_row).collect();
    snapshot::sort_snapshot_panes(&mut snapshot);
    snapshot
}

#[test]
fn daemon_refresh_pane_updates_existing_pane_from_provider() {
    let old_row = daemon_refresh_row("%1", "$1", "@1", 0, "old");
    let new_row = daemon_refresh_row("%1", "$1", "@1", 0, "new");
    let mut snapshot = daemon_refresh_snapshot(vec![old_row]);
    let mut provider = FakeTmuxReadProvider::default().with_pane("%1", Some(new_row));

    daemon::test_refresh_snapshot_pane_with_provider(&mut snapshot, &mut provider, "%1")
        .expect("pane refresh should succeed");

    assert_eq!(snapshot.panes.len(), 1);
    assert_eq!(snapshot.panes[0].pane_id, "%1");
    assert_eq!(snapshot.panes[0].tmux.pane_title_raw, "new");
    assert_eq!(snapshot.panes[0].diagnostics.cache_origin, "daemon_update");
    assert_eq!(
        snapshot.panes[0].diagnostics.proc_fallback.outcome,
        ProcFallbackOutcome::NotRun
    );
    assert_eq!(snapshot.source.kind, SourceKind::Daemon);
}

#[test]
fn daemon_refresh_pane_removes_missing_pane() {
    let mut snapshot =
        daemon_refresh_snapshot(vec![daemon_refresh_row("%1", "$1", "@1", 0, "gone")]);
    let mut provider = FakeTmuxReadProvider::default().with_pane("%1", None);

    daemon::test_refresh_snapshot_pane_with_provider(&mut snapshot, &mut provider, "%1")
        .expect("missing pane refresh should succeed");

    assert!(snapshot.panes.is_empty());
    assert_eq!(snapshot.source.kind, SourceKind::Daemon);
}

#[test]
fn daemon_refresh_pane_title_prefers_control_mode_title() {
    let row = daemon_refresh_row("%1", "$1", "@1", 0, "stale");
    let mut snapshot = daemon_refresh_snapshot(vec![row.clone()]);
    let mut provider = FakeTmuxReadProvider::default().with_pane("%1", Some(row));

    daemon::test_refresh_snapshot_pane_title_with_provider(
        &mut snapshot,
        &mut provider,
        "%1",
        "from-control-mode",
    )
    .expect("title refresh should succeed");

    assert_eq!(snapshot.panes[0].tmux.pane_title_raw, "from-control-mode");
    assert_eq!(
        snapshot.panes[0].diagnostics.proc_fallback.outcome,
        ProcFallbackOutcome::NotRun
    );
}

#[test]
fn daemon_refresh_window_replaces_only_matching_scope() {
    let old_window_pane = daemon_refresh_row("%1", "$1", "@1", 0, "old-window");
    let other_window_pane = daemon_refresh_row("%2", "$1", "@2", 0, "other-window");
    let new_window_pane = daemon_refresh_row("%3", "$1", "@1", 1, "new-window");
    let mut snapshot = daemon_refresh_snapshot(vec![old_window_pane, other_window_pane]);
    let mut provider = FakeTmuxReadProvider::default()
        .with_target_panes("@1", Some(vec![new_window_pane]));

    daemon::test_refresh_snapshot_window_with_provider(&mut snapshot, &mut provider, "@1")
        .expect("window refresh should succeed");

    let pane_ids: Vec<_> = snapshot
        .panes
        .iter()
        .map(|pane| pane.pane_id.as_str())
        .collect();
    assert_eq!(pane_ids, vec!["%3", "%2"]);
    assert!(
        snapshot
            .panes
            .iter()
            .filter(|pane| pane.location.window_name == "window-@1")
            .all(|pane| pane.diagnostics.cache_origin == "daemon_update")
    );
    assert!(
        snapshot
            .panes
            .iter()
            .filter(|pane| pane.location.window_name == "window-@1")
            .all(|pane| pane.diagnostics.proc_fallback.outcome == ProcFallbackOutcome::NotRun)
    );
}

#[test]
fn daemon_refresh_window_preserves_proc_identity_for_same_pid() {
    let old_row = daemon_refresh_row("%42", "$1", "@1", 0, "old-window");
    let mut proc_pane = classify::pane_from_row(old_row);
    proc_pane.provider = Some(Provider::Claude);
    proc_pane.diagnostics.proc_fallback.outcome = ProcFallbackOutcome::Resolved;
    proc_pane.diagnostics.proc_fallback.reason =
        "resolved provider from process evidence".to_string();
    proc_pane.diagnostics.proc_fallback.commands = vec!["claude".to_string()];
    let mut snapshot = empty_socket_snapshot("2026-05-03T00:00:00Z");
    snapshot.panes = vec![proc_pane];

    let mut new_row = daemon_refresh_row("%42", "$1", "@1", 0, "Claude Code | Working");
    new_row.pane_current_command = "node".to_string();
    let mut provider = FakeTmuxReadProvider::default().with_target_panes("@1", Some(vec![new_row]));

    daemon::test_refresh_snapshot_window_with_provider(&mut snapshot, &mut provider, "@1")
        .expect("window refresh should preserve proc identity");

    assert_eq!(snapshot.panes[0].provider, Some(Provider::Claude));
    assert_eq!(snapshot.panes[0].status.kind, StatusKind::Busy);
    assert_eq!(snapshot.panes[0].display.label, "Working");
    assert_eq!(
        snapshot.panes[0].diagnostics.proc_fallback.outcome,
        ProcFallbackOutcome::Resolved
    );
}

#[test]
fn daemon_refresh_window_preserves_proc_identity_for_moved_pane() {
    let old_row = daemon_refresh_row("%42", "$1", "@2", 0, "old-window");
    let mut proc_pane = classify::pane_from_row(old_row);
    proc_pane.provider = Some(Provider::Claude);
    proc_pane.diagnostics.proc_fallback.outcome = ProcFallbackOutcome::Resolved;
    proc_pane.diagnostics.proc_fallback.reason =
        "resolved provider from process evidence".to_string();
    proc_pane.diagnostics.proc_fallback.commands = vec!["claude".to_string()];
    let mut snapshot = empty_socket_snapshot("2026-05-03T00:00:00Z");
    snapshot.panes = vec![proc_pane];

    let mut moved_row = daemon_refresh_row("%42", "$1", "@1", 0, "Claude Code | Working");
    moved_row.pane_current_command = "node".to_string();
    let mut provider =
        FakeTmuxReadProvider::default().with_target_panes("@1", Some(vec![moved_row]));

    daemon::test_refresh_snapshot_window_with_provider(&mut snapshot, &mut provider, "@1")
        .expect("window refresh should preserve moved proc identity");

    assert_eq!(snapshot.panes.len(), 1);
    assert_eq!(snapshot.panes[0].tmux.window_id.as_deref(), Some("@1"));
    assert_eq!(snapshot.panes[0].provider, Some(Provider::Claude));
    assert_eq!(snapshot.panes[0].status.kind, StatusKind::Busy);
    assert_eq!(
        snapshot.panes[0].diagnostics.proc_fallback.outcome,
        ProcFallbackOutcome::Resolved
    );
}

#[test]
fn daemon_refresh_window_prefers_new_metadata_over_old_proc_identity() {
    let old_row = daemon_refresh_row("%42", "$1", "@1", 0, "old-window");
    let mut proc_pane = classify::pane_from_row(old_row);
    proc_pane.provider = Some(Provider::Claude);
    proc_pane.diagnostics.proc_fallback.outcome = ProcFallbackOutcome::Resolved;
    proc_pane.diagnostics.proc_fallback.reason =
        "resolved provider from process evidence".to_string();
    proc_pane.diagnostics.proc_fallback.commands = vec!["claude".to_string()];
    let mut snapshot = empty_socket_snapshot("2026-05-03T00:00:00Z");
    snapshot.panes = vec![proc_pane];

    let mut new_row = daemon_refresh_row("%42", "$1", "@1", 0, "Claude Code | Working");
    new_row.pane_current_command = "node".to_string();
    new_row.agent_provider = Some("codex".to_string());
    new_row.agent_label = Some("Explicit Codex Task".to_string());
    let mut provider = FakeTmuxReadProvider::default().with_target_panes("@1", Some(vec![new_row]));

    daemon::test_refresh_snapshot_window_with_provider(&mut snapshot, &mut provider, "@1")
        .expect("window refresh should prefer fresh metadata");

    assert_eq!(snapshot.panes[0].provider, Some(Provider::Codex));
    assert_eq!(
        snapshot.panes[0].agent_metadata.provider.as_deref(),
        Some("codex")
    );
    assert_eq!(snapshot.panes[0].display.label, "Explicit Codex Task");
    assert_eq!(
        snapshot.panes[0].diagnostics.proc_fallback.outcome,
        ProcFallbackOutcome::NotRun
    );
}

#[test]
fn daemon_refresh_window_prefers_conflicting_title_provider_over_old_proc_identity() {
    let old_row = daemon_refresh_row("%42", "$1", "@1", 0, "old-window");
    let mut proc_pane = classify::pane_from_row(old_row);
    proc_pane.provider = Some(Provider::Claude);
    proc_pane.diagnostics.proc_fallback.outcome = ProcFallbackOutcome::Resolved;
    proc_pane.diagnostics.proc_fallback.reason =
        "resolved provider from process evidence".to_string();
    proc_pane.diagnostics.proc_fallback.commands = vec!["claude".to_string()];
    let mut snapshot = empty_socket_snapshot("2026-05-03T00:00:00Z");
    snapshot.panes = vec![proc_pane];

    let mut new_row =
        daemon_refresh_row("%42", "$1", "@1", 0, "pi - refactor - agentscan: codex");
    new_row.pane_current_command = "node".to_string();
    let mut provider = FakeTmuxReadProvider::default().with_target_panes("@1", Some(vec![new_row]));

    daemon::test_refresh_snapshot_window_with_provider(&mut snapshot, &mut provider, "@1")
        .expect("window refresh should prefer fresh title provider");

    assert_eq!(snapshot.panes[0].provider, Some(Provider::Codex));
    assert_eq!(
        snapshot.panes[0].diagnostics.proc_fallback.outcome,
        ProcFallbackOutcome::NotRun
    );
}

#[test]
fn daemon_refresh_window_clears_proc_identity_without_fresh_provider_signal() {
    let old_row = daemon_refresh_row("%42", "$1", "@1", 0, "old-window");
    let mut proc_pane = classify::pane_from_row(old_row);
    proc_pane.provider = Some(Provider::Claude);
    proc_pane.diagnostics.proc_fallback.outcome = ProcFallbackOutcome::Resolved;
    proc_pane.diagnostics.proc_fallback.reason =
        "resolved provider from process evidence".to_string();
    proc_pane.diagnostics.proc_fallback.commands = vec!["claude".to_string()];
    let mut snapshot = empty_socket_snapshot("2026-05-03T00:00:00Z");
    snapshot.panes = vec![proc_pane];

    let mut new_row = daemon_refresh_row("%42", "$1", "@1", 0, "shell");
    new_row.pane_current_command = "node".to_string();
    let mut provider = FakeTmuxReadProvider::default().with_target_panes("@1", Some(vec![new_row]));

    daemon::test_refresh_snapshot_window_with_provider(&mut snapshot, &mut provider, "@1")
        .expect("window refresh should clear stale proc identity");

    assert_eq!(snapshot.panes[0].provider, None);
    assert_eq!(
        snapshot.panes[0].diagnostics.proc_fallback.outcome,
        ProcFallbackOutcome::NotRun
    );
}

#[test]
fn daemon_refresh_window_preserves_proc_identity_for_unchanged_generic_row() {
    let mut old_row = daemon_refresh_row("%42", "$1", "@1", 0, "generic");
    old_row.pane_current_command = "node".to_string();
    let mut proc_pane = classify::pane_from_row(old_row.clone());
    proc_pane.provider = Some(Provider::Claude);
    proc_pane.diagnostics.proc_fallback.outcome = ProcFallbackOutcome::Resolved;
    proc_pane.diagnostics.proc_fallback.reason =
        "resolved provider from process evidence".to_string();
    proc_pane.diagnostics.proc_fallback.commands = vec!["claude".to_string()];
    let mut snapshot = empty_socket_snapshot("2026-05-03T00:00:00Z");
    snapshot.panes = vec![proc_pane];

    let mut provider = FakeTmuxReadProvider::default().with_target_panes("@1", Some(vec![old_row]));

    daemon::test_refresh_snapshot_window_with_provider(&mut snapshot, &mut provider, "@1")
        .expect("unchanged pane refresh should preserve proc identity");

    assert_eq!(snapshot.panes[0].provider, Some(Provider::Claude));
    assert_eq!(
        snapshot.panes[0].diagnostics.proc_fallback.outcome,
        ProcFallbackOutcome::Resolved
    );
}

#[test]
fn daemon_refresh_window_preserves_fresh_metadata_with_old_proc_identity() {
    let old_row = daemon_refresh_row("%42", "$1", "@1", 0, "old-window");
    let mut proc_pane = classify::pane_from_row(old_row);
    proc_pane.provider = Some(Provider::Claude);
    proc_pane.diagnostics.proc_fallback.outcome = ProcFallbackOutcome::Resolved;
    proc_pane.diagnostics.proc_fallback.reason =
        "resolved provider from process evidence".to_string();
    proc_pane.diagnostics.proc_fallback.commands = vec!["claude".to_string()];
    let mut snapshot = empty_socket_snapshot("2026-05-03T00:00:00Z");
    snapshot.panes = vec![proc_pane];

    let mut new_row = daemon_refresh_row("%42", "$1", "@1", 0, "Claude Code | Working");
    new_row.pane_current_command = "node".to_string();
    new_row.agent_label = Some("Fresh Label".to_string());
    new_row.agent_state = Some("busy".to_string());
    let mut provider = FakeTmuxReadProvider::default().with_target_panes("@1", Some(vec![new_row]));

    daemon::test_refresh_snapshot_window_with_provider(&mut snapshot, &mut provider, "@1")
        .expect("window refresh should preserve fresh metadata");

    assert_eq!(snapshot.panes[0].provider, Some(Provider::Claude));
    assert_eq!(snapshot.panes[0].agent_metadata.provider, None);
    assert_eq!(
        snapshot.panes[0].agent_metadata.label.as_deref(),
        Some("Fresh Label")
    );
    assert_eq!(
        snapshot.panes[0].agent_metadata.state.as_deref(),
        Some("busy")
    );
    assert_eq!(snapshot.panes[0].display.label, "Fresh Label");
    assert_eq!(
        snapshot.panes[0].diagnostics.proc_fallback.outcome,
        ProcFallbackOutcome::Resolved
    );
}

#[test]
fn daemon_refresh_session_removes_missing_scope() {
    let removed_session_pane = daemon_refresh_row("%1", "$1", "@1", 0, "removed");
    let retained_session_pane = daemon_refresh_row("%2", "$2", "@2", 0, "retained");
    let mut snapshot = daemon_refresh_snapshot(vec![removed_session_pane, retained_session_pane]);
    let mut provider = FakeTmuxReadProvider::default().with_target_panes("$1", None);

    daemon::test_refresh_snapshot_session_with_provider(&mut snapshot, &mut provider, "$1")
        .expect("missing session refresh should succeed");

    assert_eq!(snapshot.panes.len(), 1);
    assert_eq!(snapshot.panes[0].pane_id, "%2");
}

#[test]
fn daemon_full_reconcile_replaces_snapshot_from_provider() {
    let old_row = daemon_refresh_row("%1", "$1", "@1", 0, "old");
    let new_row = daemon_refresh_row("%2", "$2", "@2", 0, "new");
    let mut snapshot = daemon_refresh_snapshot(vec![old_row]);
    let mut provider = FakeTmuxReadProvider::default().with_all_panes(vec![new_row]);

    daemon::test_reconcile_full_snapshot_with_provider(&mut snapshot, &mut provider, Some("3.4"))
        .expect("full reconcile should succeed");

    assert_eq!(snapshot.source.kind, SourceKind::Daemon);
    assert_eq!(snapshot.source.tmux_version.as_deref(), Some("3.4"));
    assert_eq!(snapshot.panes.len(), 1);
    assert_eq!(snapshot.panes[0].pane_id, "%2");
    assert_eq!(snapshot.panes[0].diagnostics.cache_origin, "daemon_snapshot");
    assert_ne!(
        snapshot.panes[0].diagnostics.proc_fallback.outcome,
        ProcFallbackOutcome::NotRun
    );
}

#[test]
fn daemon_resnapshot_control_event_marks_full_snapshot_refresh() {
    let old_row = daemon_refresh_row("%1", "$1", "@1", 0, "old");
    let new_row = daemon_refresh_row("%2", "$2", "@2", 0, "new");
    let mut snapshot = daemon_refresh_snapshot(vec![old_row]);
    let mut provider = FakeTmuxReadProvider::default().with_all_panes(vec![new_row]);

    let (changed, full_snapshot_refresh) =
        daemon::test_apply_resnapshot_control_event_with_provider(&mut snapshot, &mut provider)
            .expect("resnapshot control event should succeed");

    assert!(changed);
    assert!(full_snapshot_refresh);
    assert_eq!(snapshot.panes.len(), 1);
    assert_eq!(snapshot.panes[0].pane_id, "%2");
}

#[test]
fn daemon_control_event_batch_ignores_raw_output_without_reads() {
    let row = daemon_refresh_row("%1", "$1", "@1", 0, "unchanged");
    let mut snapshot = daemon_refresh_snapshot(vec![row.clone()]);
    let mut provider = FakeTmuxReadProvider::default().with_pane("%1", Some(row));
    let lines = vec![
        "%output %1 plain shell output".to_string(),
        "%output %1 printf '\\134033]0;Working\\134007'".to_string(),
    ];

    let (changed, full_snapshot_refresh, fallback_to_full) =
        daemon::test_apply_control_event_lines_with_provider(&mut snapshot, &mut provider, &lines)
            .expect("ignored control output should not refresh panes");

    assert!(!changed);
    assert!(!full_snapshot_refresh);
    assert!(!fallback_to_full);
    assert_eq!(provider.list_all_count, 0);
    assert_eq!(provider.list_target_count, 0);
    assert_eq!(provider.list_pane_count, 0);
    assert_eq!(snapshot.panes[0].tmux.pane_title_raw, "unchanged");
}

#[test]
fn daemon_control_event_batch_coalesces_latest_title_per_pane() {
    let row = daemon_refresh_row("%1", "$1", "@1", 0, "stale");
    let mut snapshot = daemon_refresh_snapshot(vec![row.clone()]);
    let mut provider = FakeTmuxReadProvider::default().with_pane("%1", Some(row));
    let lines = vec![
        "%output %1 \\033]0;first\\007".to_string(),
        "%subscription-changed agentscan $1 @1 0 %1 : %1:Codex:codex::::".to_string(),
        "%output %1 \\033]2;second\\033\\\\".to_string(),
    ];

    let (
        changed,
        full_snapshot_refresh,
        fallback_to_full,
        targeted_title_updates,
        targeted_pane_refreshes,
        targeted_scope_refreshes,
    ) = daemon::test_apply_control_event_lines_with_provider_counts(
        &mut snapshot,
        &mut provider,
        &lines,
    )
    .expect("batched pane control events should refresh once");

    assert!(changed);
    assert!(!full_snapshot_refresh);
    assert!(!fallback_to_full);
    assert_eq!(targeted_title_updates, 1);
    assert_eq!(targeted_pane_refreshes, 1);
    assert_eq!(targeted_scope_refreshes, 0);
    assert_eq!(provider.list_all_count, 0);
    assert_eq!(provider.list_target_count, 0);
    assert_eq!(provider.list_pane_count, 1);
    assert_eq!(snapshot.panes[0].tmux.pane_title_raw, "second");
}

#[test]
fn daemon_control_event_batch_applies_standalone_title() {
    let row = daemon_refresh_row("%1", "$1", "@1", 0, "stale");
    let mut snapshot = daemon_refresh_snapshot(vec![row.clone()]);
    let mut provider = FakeTmuxReadProvider::default().with_pane("%1", Some(row));
    let lines = vec!["%output %1 \\033]0;from-control-mode\\007".to_string()];

    let (changed, full_snapshot_refresh, fallback_to_full) =
        daemon::test_apply_control_event_lines_with_provider(&mut snapshot, &mut provider, &lines)
            .expect("standalone title event should refresh pane");

    assert!(changed);
    assert!(!full_snapshot_refresh);
    assert!(!fallback_to_full);
    assert_eq!(provider.list_pane_count, 1);
    assert_eq!(snapshot.panes[0].tmux.pane_title_raw, "from-control-mode");
}

#[test]
fn daemon_control_event_batch_preserves_proc_identity_on_title_update() {
    let mut pane = proc_fallback_pane(42, "node", "old-title");
    pane.provider = Some(Provider::Claude);
    pane.diagnostics.proc_fallback.outcome = ProcFallbackOutcome::Resolved;
    pane.diagnostics.proc_fallback.reason = "resolved provider from process evidence".to_string();
    pane.diagnostics.proc_fallback.commands = vec!["claude".to_string()];
    let mut snapshot = empty_socket_snapshot("2026-05-03T00:00:00Z");
    snapshot.panes = vec![pane];
    let mut row = daemon_refresh_row("%42", "$1", "@1", 0, "old-title");
    row.pane_pid = 42;
    row.pane_current_command = "node".to_string();
    row.pane_tty = "/dev/pts/42".to_string();
    row.pane_current_path = "/tmp/proc-wrapper".to_string();
    let mut provider = FakeTmuxReadProvider::default().with_pane("%42", Some(row));
    let lines = vec!["%output %42 \\033]0;Claude Code | Working\\007".to_string()];

    let (changed, full_snapshot_refresh, fallback_to_full) =
        daemon::test_apply_control_event_lines_with_provider(&mut snapshot, &mut provider, &lines)
            .expect("title event should update existing proc-resolved pane");

    assert!(changed);
    assert!(!full_snapshot_refresh);
    assert!(!fallback_to_full);
    assert_eq!(provider.list_pane_count, 1);
    assert_eq!(snapshot.panes[0].provider, Some(Provider::Claude));
    assert_eq!(
        snapshot.panes[0].diagnostics.proc_fallback.outcome,
        ProcFallbackOutcome::Resolved
    );
    assert_eq!(snapshot.panes[0].status.kind, StatusKind::Busy);
    assert_eq!(snapshot.panes[0].display.label, "Working");
    assert_eq!(snapshot.panes[0].tmux.pane_title_raw, "Claude Code | Working");
}

#[test]
fn daemon_control_event_batch_preserves_proc_identity_on_generic_title_update() {
    let mut pane = proc_fallback_pane(42, "node", "old-title");
    pane.provider = Some(Provider::Claude);
    pane.diagnostics.proc_fallback.outcome = ProcFallbackOutcome::Resolved;
    pane.diagnostics.proc_fallback.reason = "resolved provider from process evidence".to_string();
    pane.diagnostics.proc_fallback.commands = vec!["claude".to_string()];
    let mut snapshot = empty_socket_snapshot("2026-05-03T00:00:00Z");
    snapshot.panes = vec![pane];
    let mut row = daemon_refresh_row("%42", "$1", "@1", 0, "old-title");
    row.pane_pid = 42;
    row.pane_current_command = "node".to_string();
    row.pane_tty = "/dev/pts/42".to_string();
    row.pane_current_path = "/tmp/proc-wrapper".to_string();
    let mut provider = FakeTmuxReadProvider::default().with_pane("%42", Some(row));
    let lines = vec!["%output %42 \\033]0;Working\\007".to_string()];

    let (changed, full_snapshot_refresh, fallback_to_full) =
        daemon::test_apply_control_event_lines_with_provider(&mut snapshot, &mut provider, &lines)
            .expect("generic title event should keep unchanged proc-resolved pane");

    assert!(changed);
    assert!(!full_snapshot_refresh);
    assert!(!fallback_to_full);
    assert_eq!(provider.list_pane_count, 1);
    assert_eq!(snapshot.panes[0].provider, Some(Provider::Claude));
    assert_eq!(
        snapshot.panes[0].diagnostics.proc_fallback.outcome,
        ProcFallbackOutcome::Resolved
    );
    assert_eq!(snapshot.panes[0].tmux.pane_title_raw, "Working");
}

#[test]
fn daemon_control_event_batch_preserves_proc_identity_on_coalesced_pane_title_update() {
    let mut pane = proc_fallback_pane(42, "node", "old-title");
    pane.provider = Some(Provider::Claude);
    pane.diagnostics.proc_fallback.outcome = ProcFallbackOutcome::Resolved;
    pane.diagnostics.proc_fallback.reason = "resolved provider from process evidence".to_string();
    pane.diagnostics.proc_fallback.commands = vec!["claude".to_string()];
    let mut snapshot = empty_socket_snapshot("2026-05-03T00:00:00Z");
    snapshot.panes = vec![pane];

    let mut pane_row = daemon_refresh_row("%42", "$1", "@1", 0, "Claude Code | Working");
    pane_row.pane_pid = 42;
    pane_row.pane_current_command = "node".to_string();
    let mut provider = FakeTmuxReadProvider::default().with_pane("%42", Some(pane_row));
    let lines = vec![
        "%output %42 \\033]0;coalesced-title\\007".to_string(),
        "%subscription-changed agentscan $1 @1 0 %42 : %42:node:::::".to_string(),
    ];

    let (changed, full_snapshot_refresh, fallback_to_full) =
        daemon::test_apply_control_event_lines_with_provider(&mut snapshot, &mut provider, &lines)
            .expect("coalesced pane and title event should preserve proc identity");

    assert!(changed);
    assert!(!full_snapshot_refresh);
    assert!(!fallback_to_full);
    assert_eq!(provider.list_pane_count, 1);
    assert_eq!(snapshot.panes[0].provider, Some(Provider::Claude));
    assert_eq!(
        snapshot.panes[0].diagnostics.proc_fallback.outcome,
        ProcFallbackOutcome::Resolved
    );
    assert_eq!(snapshot.panes[0].status.kind, StatusKind::Busy);
    assert_eq!(snapshot.panes[0].display.label, "Working");
    assert_eq!(snapshot.panes[0].tmux.pane_title_raw, "Claude Code | Working");
}

#[test]
fn daemon_control_event_batch_applies_title_after_resnapshot() {
    let old_row = daemon_refresh_row("%1", "$1", "@1", 0, "old");
    let full_row = daemon_refresh_row("%1", "$1", "@1", 0, "from-full-snapshot");
    let pane_row = daemon_refresh_row("%1", "$1", "@1", 0, "from-pane-read");
    let mut snapshot = daemon_refresh_snapshot(vec![old_row]);
    let mut provider = FakeTmuxReadProvider::default()
        .with_all_panes(vec![full_row])
        .with_pane("%1", Some(pane_row));
    let lines = vec![
        "%sessions-changed".to_string(),
        "%output %1 \\033]0;from-control-mode\\007".to_string(),
    ];

    let (changed, full_snapshot_refresh, fallback_to_full) =
        daemon::test_apply_control_event_lines_with_provider(&mut snapshot, &mut provider, &lines)
            .expect("batched resnapshot and title events should both apply");

    assert!(changed);
    assert!(full_snapshot_refresh);
    assert!(!fallback_to_full);
    assert_eq!(provider.list_all_count, 1);
    assert_eq!(provider.list_target_count, 0);
    assert_eq!(provider.list_pane_count, 1);
    assert_eq!(snapshot.panes[0].tmux.pane_title_raw, "from-control-mode");
}

#[test]
fn daemon_control_event_batch_applies_window_refresh_after_resnapshot() {
    let old_row = daemon_refresh_row("%1", "$1", "@1", 0, "old");
    let full_row = daemon_refresh_row("%1", "$1", "@1", 0, "from-full-snapshot");
    let window_row = daemon_refresh_row("%1", "$1", "@1", 0, "from-window-refresh");
    let mut snapshot = daemon_refresh_snapshot(vec![old_row]);
    let mut provider = FakeTmuxReadProvider::default()
        .with_all_panes(vec![full_row])
        .with_target_panes("@1", Some(vec![window_row]));
    let lines = vec!["%sessions-changed".to_string(), "%window-add @1".to_string()];

    let (changed, full_snapshot_refresh, fallback_to_full) =
        daemon::test_apply_control_event_lines_with_provider(&mut snapshot, &mut provider, &lines)
            .expect("later window event should refresh after resnapshot");

    assert!(changed);
    assert!(full_snapshot_refresh);
    assert!(!fallback_to_full);
    assert_eq!(provider.list_all_count, 1);
    assert_eq!(provider.list_target_count, 1);
    assert_eq!(snapshot.panes[0].tmux.pane_title_raw, "from-window-refresh");
}

#[test]
fn daemon_control_event_batch_drops_stale_title_before_new_window_pane() {
    let window_row = daemon_refresh_row("%1", "$1", "@1", 0, "from-window-refresh");
    let mut snapshot = daemon_refresh_snapshot(Vec::new());
    let mut provider =
        FakeTmuxReadProvider::default().with_target_panes("@1", Some(vec![window_row]));
    let lines = vec![
        "%output %1 \\033]0;stale-control-title\\007".to_string(),
        "%window-add @1".to_string(),
    ];

    let (changed, full_snapshot_refresh, fallback_to_full) =
        daemon::test_apply_control_event_lines_with_provider(&mut snapshot, &mut provider, &lines)
            .expect("later window refresh should win for newly discovered pane");

    assert!(changed);
    assert!(!full_snapshot_refresh);
    assert!(!fallback_to_full);
    assert_eq!(provider.list_target_count, 1);
    assert_eq!(provider.list_pane_count, 0);
    assert_eq!(snapshot.panes[0].tmux.pane_title_raw, "from-window-refresh");
}

#[test]
fn daemon_control_event_batch_keeps_unknown_title_before_unrelated_window_refresh() {
    let titled_row = daemon_refresh_row("%1", "$1", "@1", 0, "from-pane-read");
    let window_row = daemon_refresh_row("%2", "$1", "@2", 0, "other-window");
    let mut snapshot = daemon_refresh_snapshot(Vec::new());
    let mut provider = FakeTmuxReadProvider::default()
        .with_pane("%1", Some(titled_row))
        .with_target_panes("@2", Some(vec![window_row]));
    let lines = vec![
        "%output %1 \\033]0;from-control-mode\\007".to_string(),
        "%window-add @2".to_string(),
    ];

    let (changed, full_snapshot_refresh, fallback_to_full) =
        daemon::test_apply_control_event_lines_with_provider(&mut snapshot, &mut provider, &lines)
            .expect("unrelated window refresh should not suppress unknown pane title");

    assert!(changed);
    assert!(!full_snapshot_refresh);
    assert!(!fallback_to_full);
    assert_eq!(provider.list_target_count, 1);
    assert_eq!(provider.list_pane_count, 1);
    let titled = snapshot
        .panes
        .iter()
        .find(|pane| pane.pane_id == "%1")
        .expect("title event should discover pane");
    assert_eq!(titled.tmux.pane_title_raw, "from-control-mode");
}

#[test]
fn daemon_control_event_batch_keeps_title_before_unrelated_window_refresh() {
    let titled_row = daemon_refresh_row("%1", "$1", "@1", 0, "old-title");
    let pane_row = daemon_refresh_row("%1", "$1", "@1", 0, "from-pane-read");
    let window_row = daemon_refresh_row("%2", "$1", "@2", 0, "other-window");
    let mut snapshot = daemon_refresh_snapshot(vec![titled_row]);
    let mut provider = FakeTmuxReadProvider::default()
        .with_pane("%1", Some(pane_row))
        .with_target_panes("@2", Some(vec![window_row]));
    let lines = vec![
        "%output %1 \\033]0;from-control-mode\\007".to_string(),
        "%window-add @2".to_string(),
    ];

    let (changed, full_snapshot_refresh, fallback_to_full) =
        daemon::test_apply_control_event_lines_with_provider(&mut snapshot, &mut provider, &lines)
            .expect("unrelated window refresh should not discard pane title");

    assert!(changed);
    assert!(!full_snapshot_refresh);
    assert!(!fallback_to_full);
    assert_eq!(provider.list_target_count, 1);
    assert_eq!(provider.list_pane_count, 1);
    let titled = snapshot
        .panes
        .iter()
        .find(|pane| pane.pane_id == "%1")
        .expect("titled pane should remain");
    assert_eq!(titled.tmux.pane_title_raw, "from-control-mode");
}

#[test]
fn daemon_control_event_batch_does_not_apply_stale_title_before_pane_refresh() {
    let row = daemon_refresh_row("%1", "$1", "@1", 0, "old");
    let refreshed_row = daemon_refresh_row("%1", "$1", "@1", 0, "from-pane-read");
    let mut snapshot = daemon_refresh_snapshot(vec![row]);
    let mut provider = FakeTmuxReadProvider::default().with_pane("%1", Some(refreshed_row));
    let lines = vec![
        "%output %1 \\033]0;stale-control-title\\007".to_string(),
        "%subscription-changed agentscan $1 @1 0 %1 : %1:Codex:codex::::".to_string(),
    ];

    let (changed, full_snapshot_refresh, fallback_to_full) =
        daemon::test_apply_control_event_lines_with_provider(&mut snapshot, &mut provider, &lines)
            .expect("later pane refresh should win over earlier title event");

    assert!(changed);
    assert!(!full_snapshot_refresh);
    assert!(!fallback_to_full);
    assert_eq!(provider.list_pane_count, 1);
    assert_eq!(snapshot.panes[0].tmux.pane_title_raw, "from-pane-read");
}

#[test]
fn daemon_control_event_batch_does_not_refresh_for_stale_title_before_resnapshot() {
    let old_row = daemon_refresh_row("%1", "$1", "@1", 0, "old");
    let full_row = daemon_refresh_row("%1", "$1", "@1", 0, "from-full-snapshot");
    let mut snapshot = daemon_refresh_snapshot(vec![old_row]);
    let mut provider = FakeTmuxReadProvider::default().with_all_panes(vec![full_row]);
    let lines = vec![
        "%output %1 \\033]0;stale-control-title\\007".to_string(),
        "%sessions-changed".to_string(),
    ];

    let (changed, full_snapshot_refresh, fallback_to_full) =
        daemon::test_apply_control_event_lines_with_provider(&mut snapshot, &mut provider, &lines)
            .expect("later resnapshot should win over earlier title event");

    assert!(changed);
    assert!(full_snapshot_refresh);
    assert!(!fallback_to_full);
    assert_eq!(provider.list_all_count, 1);
    assert_eq!(provider.list_pane_count, 0);
    assert_eq!(snapshot.panes[0].tmux.pane_title_raw, "from-full-snapshot");
}

#[test]
fn daemon_control_event_batch_ignores_title_for_missing_pane() {
    let mut snapshot = daemon_refresh_snapshot(Vec::new());
    let mut provider = FakeTmuxReadProvider::default().with_pane("%404", None);
    let lines = vec!["%output %404 \\033]0;missing\\007".to_string()];

    let (
        changed,
        full_snapshot_refresh,
        fallback_to_full,
        targeted_title_updates,
        targeted_pane_refreshes,
        targeted_scope_refreshes,
    ) = daemon::test_apply_control_event_lines_with_provider_counts(
        &mut snapshot,
        &mut provider,
        &lines,
    )
    .expect("missing title pane should not fail");

    assert!(!changed);
    assert!(!full_snapshot_refresh);
    assert!(!fallback_to_full);
    assert_eq!(targeted_title_updates, 0);
    assert_eq!(targeted_pane_refreshes, 0);
    assert_eq!(targeted_scope_refreshes, 0);
    assert_eq!(provider.list_pane_count, 1);
    assert!(snapshot.panes.is_empty());
}

#[test]
fn daemon_deep_control_mode_telemetry_env_value_parser() {
    assert!(daemon::test_deep_control_mode_telemetry_value_enabled("1"));
    assert!(daemon::test_deep_control_mode_telemetry_value_enabled("true"));
    assert!(daemon::test_deep_control_mode_telemetry_value_enabled(" yes "));
    assert!(!daemon::test_deep_control_mode_telemetry_value_enabled(""));
    assert!(!daemon::test_deep_control_mode_telemetry_value_enabled("0"));
    assert!(!daemon::test_deep_control_mode_telemetry_value_enabled("false"));
    assert!(!daemon::test_deep_control_mode_telemetry_value_enabled("off"));
}

#[test]
fn daemon_observability_skips_snapshot_diff_for_ignored_control_output() {
    let lines = vec!["%output %1 ordinary pane bytes".to_string()];

    let (should_record, should_capture_snapshot_diff, refresh, detail) =
        daemon::test_control_event_observability_for_lines(&lines);

    assert!(!should_record);
    assert!(!should_capture_snapshot_diff);
    assert_eq!(refresh, "none");
    assert_eq!(detail.as_deref(), Some("ignored:1"));
}

#[test]
fn daemon_reconcile_publish_decision_suppresses_timestamp_only_changes() {
    let previous = empty_socket_snapshot("2026-05-23T18:00:00Z");
    let mut current = previous.clone();
    current.generated_at = "2026-05-23T18:00:01Z".to_string();
    current.source.daemon_generated_at = Some("2026-05-23T18:00:01Z".to_string());

    let (should_publish, reset_reconcile_timer) =
        daemon::test_reconcile_refresh_publish_decision(&previous, &current);

    assert!(!should_publish);
    assert!(reset_reconcile_timer);
}

#[test]
fn daemon_reconcile_publish_decision_publishes_material_changes() {
    let previous = empty_socket_snapshot("2026-05-23T18:00:00Z");
    let mut current = previous.clone();
    current.panes.push(proc_fallback_pane(42, "claude", "claude"));

    let (should_publish, reset_reconcile_timer) =
        daemon::test_reconcile_refresh_publish_decision(&previous, &current);

    assert!(should_publish);
    assert!(reset_reconcile_timer);
}

#[test]
fn daemon_control_event_timer_reset_tracks_broker_recovery_and_fallback() {
    assert!(daemon::test_control_event_refresh_should_reset_reconcile_timer(
        true, true, true
    ));
    assert!(daemon::test_control_event_refresh_should_reset_reconcile_timer(
        true, false, false
    ));
    assert!(!daemon::test_control_event_refresh_should_reset_reconcile_timer(
        false, false, false
    ));
    assert!(!daemon::test_control_event_refresh_should_reset_reconcile_timer(
        true, false, true
    ));
}

#[test]
fn daemon_control_exit_event_skips_broker_recovery() {
    assert!(daemon::test_control_event_should_recover_broker(false));
    assert!(!daemon::test_control_event_should_recover_broker(true));
}

#[test]
fn daemon_reconcile_interval_uses_fallback_when_broker_is_disabled() {
    assert_eq!(
        daemon::test_reconcile_interval_for_broker_enabled(false),
        std::time::Duration::from_secs(1)
    );
}

#[test]
fn daemon_runtime_telemetry_counts_reconcile_results_and_fallbacks() {
    let previous = empty_socket_snapshot("2026-05-23T18:00:00Z");
    let mut noop_current = previous.clone();
    noop_current.generated_at = "2026-05-23T18:00:01Z".to_string();
    noop_current.source.daemon_generated_at = Some("2026-05-23T18:00:01Z".to_string());

    let mut changed_current = noop_current.clone();
    changed_current.panes.push(proc_fallback_pane(42, "claude", "claude"));

    let telemetry = daemon::test_runtime_telemetry_after_reconcile_results(
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
