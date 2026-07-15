#[test]
fn picker_rows_assign_tui_keys_and_expose_display_contract() {
    let picker_keys = super::picker::PickerKeySet::default();
    let mut panes = vec![
        proc_fallback_pane(42, "codex", "Codex"),
        proc_fallback_pane(43, "claude", "Claude Code"),
    ];
    panes[0].provider = Some(Provider::Codex);
    panes[0].status = PaneStatus::metadata(StatusKind::Idle);
    panes[0].display.label = "Root Task".to_string();
    panes[1].provider = Some(Provider::Claude);
    panes[1].status = PaneStatus::metadata(StatusKind::Busy);
    panes[1].display.label = "Split Task".to_string();

    let rows = super::picker::picker_rows(
        &panes,
        None,
        0,
        super::picker::PickerGroupBy::Session,
        &picker_keys,
    );

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].key, '1');
    assert_eq!(rows[0].pane_id, "%42");
    assert_eq!(rows[0].provider, Some(Provider::Codex));
    assert_eq!(rows[0].status.kind, StatusKind::Idle);
    assert_eq!(rows[0].display_label, "Root Task");
    assert_eq!(rows[0].location_tag, "ambiguous:1.1");
    assert_eq!(rows[0].workspace.label, "ambiguous");
    assert_eq!(
        rows[0].workspace.source,
        super::picker::PickerWorkspaceSource::Session
    );
    assert_eq!(rows[1].key, '2');
    assert_eq!(rows[1].pane_id, "%43");
}

#[test]
fn picker_key_normalization_accepts_supported_keys_only() {
    let picker_keys = super::picker::PickerKeySet::default();

    assert_eq!(
        super::picker::normalize_picker_key("q", &picker_keys).unwrap(),
        'Q'
    );
    assert_eq!(
        super::picker::normalize_picker_key("Q", &picker_keys).unwrap(),
        'Q'
    );
    assert_eq!(
        super::picker::normalize_picker_key("1", &picker_keys).unwrap(),
        '1'
    );

    let error = super::picker::normalize_picker_key("a", &picker_keys).unwrap_err();
    assert!(
        error.to_string().contains("is not supported"),
        "expected unsupported key error, got {error:#}"
    );

    let error = super::picker::normalize_picker_key("qq", &picker_keys).unwrap_err();
    assert!(
        error.to_string().contains("must be a single key"),
        "expected single-key error, got {error:#}"
    );
}

#[test]
fn picker_rows_accept_custom_key_order() {
    let picker_keys = super::picker::PickerKeySet::from_config_values(&custom_picker_key_values())
        .expect("custom key set should parse");
    let panes = vec![
        proc_fallback_pane(42, "codex", "Codex"),
        proc_fallback_pane(43, "claude", "Claude Code"),
    ];

    let rows = super::picker::picker_rows(
        &panes,
        None,
        0,
        super::picker::PickerGroupBy::Session,
        &picker_keys,
    );

    assert_eq!(rows[0].key, 'A');
    assert_eq!(rows[1].key, 'S');
    assert_eq!(
        super::picker::normalize_picker_key("s", &picker_keys).unwrap(),
        'S'
    );
    assert!(
        super::picker::normalize_picker_key("1", &picker_keys)
            .unwrap_err()
            .to_string()
            .contains("is not supported")
    );
}

#[test]
fn picker_rows_session_grouping_preserves_input_order() {
    let picker_keys = super::picker::PickerKeySet::default();
    let panes = vec![
        tmux_pane_row(1)
            .session_name("zeta")
            .pane_id("%1")
            .command("codex")
            .pane(),
        tmux_pane_row(2)
            .session_name("alpha")
            .pane_id("%2")
            .command("codex")
            .pane(),
    ];

    let rows = super::picker::picker_rows(
        &panes,
        None,
        0,
        super::picker::PickerGroupBy::Session,
        &picker_keys,
    );

    assert_eq!(rows[0].pane_id, "%1");
    assert_eq!(rows[0].key, '1');
    assert_eq!(rows[1].pane_id, "%2");
    assert_eq!(rows[1].key, '2');
}

#[test]
fn pane_record_uses_canonical_shape() {
    let pane = tmux_pane_row(324026)
        .session_name("notes")
        .window_index(4)
        .pane_id("%41")
        .command("claude")
        .title("Claude Code | Query")
        .tty("/dev/pts/44")
        .current_path("/home/auro/notes")
        .pane();

    assert_eq!(pane.provider, Some(Provider::Claude));
    assert_eq!(pane.location.session_name, "notes");
    assert_eq!(pane.display.label, "Query");
    assert_eq!(pane.display.activity_label.as_deref(), Some("Query"));
}

#[test]
fn active_flags_propagate_through_pane_record_and_picker() {
    let active_row = |pane_id: &str, pane_active: bool, window_active: bool| {
        tmux_pane_row(1000)
            .session_name("notes")
            .pane_id(pane_id)
            .command("claude")
            .title("Claude Code")
            .tty("/dev/pts/1")
            .current_path("/home/auro/notes")
            .pane_active(pane_active)
            .window_active(window_active)
            .build()
    };

    // is_active requires BOTH the pane and its window to be active.
    let focused = classify::pane_from_row(active_row("%1", true, true));
    assert!(focused.tmux.pane_active);
    assert!(focused.tmux.window_active);
    assert!(focused.is_active());

    let pane_active_other_window = classify::pane_from_row(active_row("%2", true, false));
    assert!(!pane_active_other_window.is_active());

    let inactive = classify::pane_from_row(active_row("%3", false, true));
    assert!(!inactive.is_active());

    // The is_active flag flows into the picker projection clients consume.
    let panes = vec![focused, pane_active_other_window, inactive];
    let picker_keys = super::picker::PickerKeySet::default();
    let rows = super::picker::picker_rows(
        &panes,
        None,
        2,
        super::picker::PickerGroupBy::Session,
        &picker_keys,
    );
    assert!(rows[0].is_active);
    assert!(!rows[1].is_active);
    assert!(!rows[2].is_active);
    // Without a focused session, no row is the live pane.
    assert!(rows.iter().all(|row| !row.is_focused));
    // The attached-client count is echoed on every row.
    assert!(rows.iter().all(|row| row.attached_client_count == 2));

    // With the "notes" session focused, only the active pane of that session is
    // the live pane; an active-but-not-window-active pane stays unfocused.
    let focused_rows = super::picker::picker_rows(
        &panes,
        Some("notes"),
        1,
        super::picker::PickerGroupBy::Session,
        &picker_keys,
    );
    assert!(focused_rows[0].is_focused);
    assert!(!focused_rows[1].is_focused);
    assert!(!focused_rows[2].is_focused);

    // A focused session with no matching active pane yields no live pane.
    assert!(
        super::picker::picker_rows(
            &panes,
            Some("other"),
            1,
            super::picker::PickerGroupBy::Session,
            &picker_keys,
        )
            .iter()
            .all(|row| !row.is_focused)
    );
}

#[test]
fn focus_recency_propagates_through_picker_rows_and_omits_none() {
    let row = |pane_id: &str| {
        tmux_pane_row(1000)
            .session_name("notes")
            .pane_id(pane_id)
            .command("claude")
            .title("Claude Code")
            .tty("/dev/pts/1")
            .current_path("/home/auro/notes")
            .build()
    };
    let mut recent = classify::pane_from_row(row("%1"));
    recent.last_focus_seq = Some(9);
    let unstamped = classify::pane_from_row(row("%2"));

    let picker_keys = super::picker::PickerKeySet::default();
    let rows = super::picker::picker_rows(
        &[recent, unstamped],
        None,
        1,
        super::picker::PickerGroupBy::Session,
        &picker_keys,
    );
    assert_eq!(rows[0].last_focus_seq, Some(9));
    assert_eq!(rows[1].last_focus_seq, None);

    // Absent recency stays off the wire; present recency is serialized.
    let serialized = serde_json::to_string(&rows).expect("rows serialize");
    assert_eq!(serialized.matches("last_focus_seq").count(), 1);
}

#[test]
fn picker_rows_group_by_cwd_basename_and_order_by_group_then_location() {
    let picker_keys = super::picker::PickerKeySet::default();
    let panes = vec![
        tmux_pane_row(1)
            .session_name("zeta")
            .window_index(0)
            .pane_index(0)
            .pane_id("%1")
            .command("codex")
            .current_path("/work/beta")
            .pane(),
        tmux_pane_row(2)
            .session_name("alpha")
            .window_index(0)
            .pane_index(0)
            .pane_id("%2")
            .command("codex")
            .current_path("/work/alpha")
            .pane(),
        tmux_pane_row(3)
            .session_name("alpha")
            .window_index(1)
            .pane_index(0)
            .pane_id("%3")
            .command("codex")
            .current_path("/work/beta")
            .pane(),
    ];

    let rows = super::picker::picker_rows(
        &panes,
        None,
        0,
        super::picker::PickerGroupBy::Cwd,
        &picker_keys,
    );

    assert_eq!(rows[0].key, '1');
    assert_eq!(rows[0].pane_id, "%2");
    assert_eq!(rows[0].workspace.label, "alpha");
    assert_eq!(
        rows[0].workspace.source,
        super::picker::PickerWorkspaceSource::Cwd
    );
    assert_eq!(rows[1].key, '2');
    assert_eq!(rows[1].pane_id, "%3");
    assert_eq!(rows[1].workspace.label, "beta");
    assert_eq!(rows[2].key, '3');
    assert_eq!(rows[2].pane_id, "%1");
    assert_eq!(rows[2].location_tag, "zeta:0.0");
}

#[test]
fn picker_workspace_identity_disambiguates_same_basename_paths() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let acme_api = tempdir.path().join("src/acme/api");
    let vendor_api = tempdir.path().join("src/vendor/api");
    std::fs::create_dir_all(acme_api.join(".git")).expect("acme repo should be created");
    std::fs::create_dir_all(vendor_api.join(".git")).expect("vendor repo should be created");

    let cwd_panes = vec![
        tmux_pane_row(1)
            .session_name("alpha")
            .pane_id("%1")
            .command("codex")
            .current_path(acme_api.to_string_lossy())
            .pane(),
        tmux_pane_row(2)
            .session_name("beta")
            .pane_id("%2")
            .command("codex")
            .current_path(vendor_api.to_string_lossy())
            .pane(),
    ];
    let picker_keys = super::picker::PickerKeySet::default();

    let cwd_rows = super::picker::picker_rows(
        &cwd_panes,
        None,
        0,
        super::picker::PickerGroupBy::Cwd,
        &picker_keys,
    );

    assert_eq!(cwd_rows[0].workspace.label, "api");
    assert_eq!(cwd_rows[1].workspace.label, "api");
    assert_ne!(cwd_rows[0].workspace.id, cwd_rows[1].workspace.id);

    let git_rows = super::picker::picker_rows(
        &cwd_panes,
        None,
        0,
        super::picker::PickerGroupBy::GitRepo,
        &picker_keys,
    );

    assert_eq!(git_rows[0].workspace.label, "api");
    assert_eq!(git_rows[1].workspace.label, "api");
    assert_ne!(git_rows[0].workspace.id, git_rows[1].workspace.id);
}

#[cfg(unix)]
#[test]
fn picker_workspace_identity_keeps_symlink_aliases_distinct() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let real_repo = tempdir.path().join("Users/me/agentscan");
    let alias_parent = tempdir.path().join("work");
    let alias_repo = alias_parent.join("current");
    std::fs::create_dir_all(real_repo.join(".git")).expect("repo should be created");
    std::fs::create_dir_all(alias_parent.as_path()).expect("alias parent should be created");
    std::os::unix::fs::symlink(real_repo.as_path(), alias_repo.as_path())
        .expect("repo alias should be created");

    let picker_keys = super::picker::PickerKeySet::default();
    let panes = vec![
        tmux_pane_row(1)
            .session_name("alias")
            .pane_id("%1")
            .command("codex")
            .current_path(alias_repo.to_string_lossy())
            .pane(),
        tmux_pane_row(2)
            .session_name("real")
            .pane_id("%2")
            .command("codex")
            .current_path(real_repo.to_string_lossy())
            .pane(),
    ];

    let rows = super::picker::picker_rows(
        &panes,
        None,
        0,
        super::picker::PickerGroupBy::GitRepo,
        &picker_keys,
    );

    assert_eq!(rows[0].workspace.label, "agentscan");
    assert_eq!(rows[1].workspace.label, "current");
    assert_ne!(rows[0].workspace.id, rows[1].workspace.id);
}

#[test]
fn picker_workspace_cache_keeps_session_fallbacks_distinct() {
    let picker_keys = super::picker::PickerKeySet::default();
    let panes = vec![
        tmux_pane_row(1)
            .session_name("alpha")
            .pane_id("%1")
            .command("codex")
            .current_path("/")
            .pane(),
        tmux_pane_row(2)
            .session_name("beta")
            .pane_id("%2")
            .command("codex")
            .current_path("/")
            .pane(),
    ];

    let rows = super::picker::picker_rows(
        &panes,
        None,
        0,
        super::picker::PickerGroupBy::Cwd,
        &picker_keys,
    );

    assert_eq!(rows[0].workspace.label, "alpha");
    assert_eq!(rows[1].workspace.label, "beta");
    assert_ne!(rows[0].workspace.id, rows[1].workspace.id);
}

#[test]
fn picker_workspace_prefers_agent_metadata_cwd_over_tmux_path() {
    let pane = tmux_pane_row(1)
        .session_name("work")
        .current_path("/tmp/bootstrap")
        .agent_cwd("/repo/actual-task")
        .pane();

    let workspace = super::picker::workspace_for_pane(&pane, super::picker::PickerGroupBy::Cwd);

    assert_eq!(workspace.label, "actual-task");
    assert_eq!(workspace.source, super::picker::PickerWorkspaceSource::Cwd);
}

#[test]
fn picker_rows_group_by_git_repo_with_cwd_fallback() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let repo = tempdir.path().join("agentscan");
    let nested = repo.join("crates/core");
    std::fs::create_dir_all(nested.as_path()).expect("repo dirs should be created");
    std::fs::create_dir_all(repo.join(".git")).expect("git dir should be created");
    let outside = tempdir.path().join("outside/work");
    std::fs::create_dir_all(outside.as_path()).expect("outside dir should be created");

    let picker_keys = super::picker::PickerKeySet::default();
    let panes = vec![
        tmux_pane_row(1)
            .session_name("z")
            .pane_id("%1")
            .command("codex")
            .current_path(outside.to_string_lossy())
            .pane(),
        tmux_pane_row(2)
            .session_name("a")
            .pane_id("%2")
            .command("codex")
            .current_path(nested.to_string_lossy())
            .pane(),
    ];

    let rows = super::picker::picker_rows(
        &panes,
        None,
        0,
        super::picker::PickerGroupBy::GitRepo,
        &picker_keys,
    );

    assert_eq!(rows[0].pane_id, "%2");
    assert_eq!(rows[0].workspace.label, "agentscan");
    assert_eq!(
        rows[0].workspace.source,
        super::picker::PickerWorkspaceSource::GitRepo
    );
    assert_eq!(rows[1].pane_id, "%1");
    assert_eq!(rows[1].workspace.label, "work");
    assert_eq!(
        rows[1].workspace.source,
        super::picker::PickerWorkspaceSource::Cwd
    );
}

#[test]
fn picker_git_repo_cache_does_not_group_nested_repo_under_parent() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let outer = tempdir.path().join("outer");
    let nested = outer.join("nested");
    std::fs::create_dir_all(outer.join(".git")).expect("outer git dir should be created");
    std::fs::create_dir_all(nested.join(".git")).expect("nested git dir should be created");
    std::fs::create_dir_all(outer.join("src")).expect("outer src should be created");
    std::fs::create_dir_all(nested.join("src")).expect("nested src should be created");

    let picker_keys = super::picker::PickerKeySet::default();
    let panes = vec![
        tmux_pane_row(1)
            .session_name("work")
            .pane_id("%1")
            .command("codex")
            .current_path(outer.join("src").to_string_lossy())
            .pane(),
        tmux_pane_row(2)
            .session_name("work")
            .pane_id("%2")
            .command("codex")
            .current_path(nested.join("src").to_string_lossy())
            .pane(),
    ];

    let rows = super::picker::picker_rows(
        &panes,
        None,
        0,
        super::picker::PickerGroupBy::GitRepo,
        &picker_keys,
    );
    let outer_row = rows
        .iter()
        .find(|row| row.pane_id == "%1")
        .expect("outer row should be present");
    let nested_row = rows
        .iter()
        .find(|row| row.pane_id == "%2")
        .expect("nested row should be present");

    assert_eq!(outer_row.workspace.label, "outer");
    assert_eq!(nested_row.workspace.label, "nested");
    assert_ne!(outer_row.workspace.id, nested_row.workspace.id);
}

#[test]
fn picker_git_repo_grouping_labels_linked_worktree_by_common_repo_name() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let common_repo = tempdir.path().join("agentscan");
    let worktree = tempdir.path().join("worktrees/new-detection");
    let gitdir = common_repo.join(".git/worktrees/new-detection");
    let nested = worktree.join("src/app");
    std::fs::create_dir_all(gitdir.as_path()).expect("gitdir should be created");
    std::fs::create_dir_all(nested.as_path()).expect("worktree dirs should be created");
    std::fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", gitdir.display()),
    )
    .expect(".git file should be written");
    std::fs::write(gitdir.join("commondir"), "../..\n").expect("commondir should be written");

    let pane = tmux_pane_row(1)
        .session_name("work")
        .command("codex")
        .current_path(nested.to_string_lossy())
        .pane();

    let workspace = super::picker::workspace_for_pane(&pane, super::picker::PickerGroupBy::GitRepo);

    assert_eq!(workspace.label, "agentscan");
    assert_eq!(
        workspace.source,
        super::picker::PickerWorkspaceSource::GitRepo
    );
}

#[test]
fn picker_git_repo_grouping_labels_bare_linked_worktree_by_repo_name() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let bare_repo = tempdir.path().join("agentscan.git");
    let worktree = tempdir.path().join("worktrees/new-detection");
    let gitdir = bare_repo.join("worktrees/new-detection");
    let nested = worktree.join("src/app");
    std::fs::create_dir_all(gitdir.as_path()).expect("gitdir should be created");
    std::fs::create_dir_all(nested.as_path()).expect("worktree dirs should be created");
    std::fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", gitdir.display()),
    )
    .expect(".git file should be written");
    std::fs::write(gitdir.join("commondir"), "../..\n").expect("commondir should be written");

    let pane = tmux_pane_row(1)
        .session_name("work")
        .command("codex")
        .current_path(nested.to_string_lossy())
        .pane();

    let workspace = super::picker::workspace_for_pane(&pane, super::picker::PickerGroupBy::GitRepo);

    assert_eq!(workspace.label, "agentscan");
    assert_eq!(
        workspace.source,
        super::picker::PickerWorkspaceSource::GitRepo
    );
}

#[test]
fn picker_git_repo_grouping_labels_submodule_by_submodule_folder() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let super_repo = tempdir.path().join("super-project");
    let submodule = super_repo.join("vendor/tooling");
    let gitdir = super_repo.join(".git/modules/vendor/tooling");
    std::fs::create_dir_all(gitdir.as_path()).expect("submodule gitdir should be created");
    std::fs::create_dir_all(submodule.join("src")).expect("submodule dirs should be created");
    std::fs::write(
        submodule.join(".git"),
        format!("gitdir: {}\n", gitdir.display()),
    )
    .expect(".git file should be written");
    std::fs::write(gitdir.join("commondir"), "../../..\n").expect("commondir should be written");

    let pane = tmux_pane_row(1)
        .session_name("work")
        .command("codex")
        .current_path(submodule.join("src").to_string_lossy())
        .pane();

    let workspace = super::picker::workspace_for_pane(&pane, super::picker::PickerGroupBy::GitRepo);

    assert_eq!(workspace.label, "tooling");
    assert_eq!(
        workspace.source,
        super::picker::PickerWorkspaceSource::GitRepo
    );
}
