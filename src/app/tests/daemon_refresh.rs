use std::collections::HashMap;

#[derive(Default)]
struct FakeTmuxReadProvider {
    all_panes: Vec<super::TmuxPaneRow>,
    target_panes: HashMap<String, Option<Vec<super::TmuxPaneRow>>>,
    pane_rows: HashMap<String, Option<super::TmuxPaneRow>>,
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
        Ok(self.all_panes.clone())
    }

    fn list_target_panes(
        &mut self,
        target: &str,
    ) -> anyhow::Result<Option<Vec<super::TmuxPaneRow>>> {
        Ok(self.target_panes.get(target).cloned().unwrap_or(None))
    }

    fn list_pane(&mut self, pane_id: &str) -> anyhow::Result<Option<super::TmuxPaneRow>> {
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
    assert_eq!(telemetry.targeted_refresh_fallback_to_full_count, 1);
    assert_eq!(telemetry.reconcile_attempt_count, 2);
    assert_eq!(telemetry.reconcile_noop_count, 1);
    assert_eq!(telemetry.reconcile_changed_snapshot_count, 1);
    assert_eq!(telemetry.broker_fallback_count, 2);
}
