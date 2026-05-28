#[test]
fn picker_rows_assign_tui_keys_and_expose_display_contract() {
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

    let rows = super::picker::picker_rows(&panes, None, 0);

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].key, '1');
    assert_eq!(rows[0].pane_id, "%42");
    assert_eq!(rows[0].provider, Some(Provider::Codex));
    assert_eq!(rows[0].status.kind, StatusKind::Idle);
    assert_eq!(rows[0].display_label, "Root Task");
    assert_eq!(rows[0].location_tag, "ambiguous:1.1");
    assert_eq!(rows[1].key, '2');
    assert_eq!(rows[1].pane_id, "%43");
}

#[test]
fn picker_key_normalization_accepts_supported_keys_only() {
    assert_eq!(super::picker::normalize_picker_key("q").unwrap(), 'Q');
    assert_eq!(super::picker::normalize_picker_key("Q").unwrap(), 'Q');
    assert_eq!(super::picker::normalize_picker_key("1").unwrap(), '1');

    let error = super::picker::normalize_picker_key("a").unwrap_err();
    assert!(
        error.to_string().contains("is not supported"),
        "expected unsupported key error, got {error:#}"
    );

    let error = super::picker::normalize_picker_key("qq").unwrap_err();
    assert!(
        error.to_string().contains("must be a single key"),
        "expected single-key error, got {error:#}"
    );
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
    let rows = super::picker::picker_rows(&panes, None, 2);
    assert!(rows[0].is_active);
    assert!(!rows[1].is_active);
    assert!(!rows[2].is_active);
    // Without a focused session, no row is the live pane.
    assert!(rows.iter().all(|row| !row.is_focused));
    // The attached-client count is echoed on every row.
    assert!(rows.iter().all(|row| row.attached_client_count == 2));

    // With the "notes" session focused, only the active pane of that session is
    // the live pane; an active-but-not-window-active pane stays unfocused.
    let focused_rows = super::picker::picker_rows(&panes, Some("notes"), 1);
    assert!(focused_rows[0].is_focused);
    assert!(!focused_rows[1].is_focused);
    assert!(!focused_rows[2].is_focused);

    // A focused session with no matching active pane yields no live pane.
    assert!(
        super::picker::picker_rows(&panes, Some("other"), 1)
            .iter()
            .all(|row| !row.is_focused)
    );
}

