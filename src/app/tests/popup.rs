fn popup_test_pane(pane_index: u32) -> PaneRecord {
    classify::pane_from_row(super::TmuxPaneRow {
        session_name: "alpha".to_string(),
        window_index: 1,
        pane_index,
        pane_id: format!("%{pane_index}"),
        pane_pid: pane_index,
        pane_current_command: if pane_index.is_multiple_of(2) {
            "claude".to_string()
        } else {
            "codex".to_string()
        },
        pane_title_raw: format!("Task {pane_index:02}"),
        pane_tty: format!("/dev/pts/{pane_index}"),
        pane_current_path: "/tmp/alpha".to_string(),
        window_name: "editor".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    })
}

#[test]
fn popup_render_rows_include_location_status_and_key_labels() {
    let pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "notes".to_string(),
        window_index: 4,
        pane_index: 1,
        pane_id: "%41".to_string(),
        pane_pid: 324026,
        pane_current_command: "claude".to_string(),
        pane_title_raw: "Claude Code | Working".to_string(),
        pane_tty: "/dev/pts/44".to_string(),
        pane_current_path: "/home/auro/notes".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });

    let mut key_targets = std::collections::BTreeMap::new();
    super::popup_ui::synchronize_key_targets(&mut key_targets, std::slice::from_ref(&pane));

    let lines = super::popup_ui::render_rows(&[pane], &key_targets);
    assert_eq!(lines, vec!["[1] 🟡 \u{e76f} notes:4.1 - Working"]);
}

#[test]
fn provider_display_marker_uses_compact_markers_for_codex_and_claude() {
    assert_eq!(
        super::provider_display_marker(Some(Provider::Codex)),
        "\u{f07b5}"
    );
    assert_eq!(
        super::provider_display_marker(Some(Provider::Claude)),
        "\u{e76f}"
    );
    assert_eq!(
        super::provider_display_marker(Some(Provider::Gemini)),
        "\u{e7f0}"
    );
    assert_eq!(
        super::provider_display_marker(Some(Provider::Copilot)),
        "\u{ec1e}"
    );
    assert_eq!(
        super::provider_display_marker(Some(Provider::CursorCli)),
        "\u{f12e9}"
    );
    assert_eq!(
        super::provider_display_marker(Some(Provider::Pi)),
        "\u{e22c}"
    );
    assert_eq!(
        super::provider_display_marker(Some(Provider::Opencode)),
        "\u{f07e2}"
    );
    assert_eq!(super::provider_display_marker(None), "unknown");
}

#[test]
fn popup_render_rows_respect_terminal_cell_width_with_wide_status_emoji() {
    let pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "notes".to_string(),
        window_index: 4,
        pane_index: 1,
        pane_id: "%41".to_string(),
        pane_pid: 324026,
        pane_current_command: "claude".to_string(),
        pane_title_raw: "Claude Code | Working on a much longer task title".to_string(),
        pane_tty: "/dev/pts/44".to_string(),
        pane_current_path: "/home/auro/notes".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });

    let mut key_targets = std::collections::BTreeMap::new();
    super::popup_ui::synchronize_key_targets(&mut key_targets, std::slice::from_ref(&pane));

    let width = 28;
    let lines = super::popup_ui::render_rows_for_width(&[pane], &key_targets, width);

    assert_eq!(lines.len(), 1);
    assert!(lines[0].ends_with('…'));
    assert!(UnicodeWidthStr::width(lines[0].as_str()) <= width);
}

#[test]
fn popup_render_rows_sanitize_control_characters_and_escape_sequences() {
    let pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "notes".to_string(),
        window_index: 4,
        pane_index: 1,
        pane_id: "%41".to_string(),
        pane_pid: 324026,
        pane_current_command: "claude".to_string(),
        pane_title_raw: "Claude Code | Working".to_string(),
        pane_tty: "/dev/pts/44".to_string(),
        pane_current_path: "/home/auro/notes".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: Some("Task\nnext\r\tstep \u{1b}[31mnow\u{1b}[0m\u{7}".to_string()),
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });

    let mut key_targets = std::collections::BTreeMap::new();
    super::popup_ui::synchronize_key_targets(&mut key_targets, std::slice::from_ref(&pane));

    let lines = super::popup_ui::render_rows(&[pane], &key_targets);
    assert_eq!(
        lines,
        vec!["[1] 🟡 \u{e76f} notes:4.1 - Task next step now"]
    );
}

#[test]
fn popup_key_assignments_stay_stable_across_rerenders() {
    let pane_one = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "alpha".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%1".to_string(),
        pane_pid: 1,
        pane_current_command: "codex".to_string(),
        pane_title_raw: "Working".to_string(),
        pane_tty: "/dev/pts/1".to_string(),
        pane_current_path: "/tmp/alpha".to_string(),
        window_name: "editor".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
    let pane_two = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "alpha".to_string(),
        window_index: 1,
        pane_index: 2,
        pane_id: "%2".to_string(),
        pane_pid: 2,
        pane_current_command: "claude".to_string(),
        pane_title_raw: "Ready".to_string(),
        pane_tty: "/dev/pts/2".to_string(),
        pane_current_path: "/tmp/alpha".to_string(),
        window_name: "editor".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
    let pane_three = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "alpha".to_string(),
        window_index: 1,
        pane_index: 3,
        pane_id: "%3".to_string(),
        pane_pid: 3,
        pane_current_command: "claude".to_string(),
        pane_title_raw: "Working".to_string(),
        pane_tty: "/dev/pts/3".to_string(),
        pane_current_path: "/tmp/alpha".to_string(),
        window_name: "editor".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });

    let mut key_targets = std::collections::BTreeMap::new();
    super::popup_ui::synchronize_key_targets(
        &mut key_targets,
        &[pane_one.clone(), pane_two.clone(), pane_three.clone()],
    );
    assert_eq!(key_targets.get(&'1').map(String::as_str), Some("%1"));
    assert_eq!(key_targets.get(&'2').map(String::as_str), Some("%2"));
    assert_eq!(key_targets.get(&'3').map(String::as_str), Some("%3"));

    super::popup_ui::synchronize_key_targets(
        &mut key_targets,
        &[pane_one.clone(), pane_three.clone()],
    );
    assert_eq!(key_targets.get(&'1').map(String::as_str), Some("%1"));
    assert_eq!(key_targets.get(&'3').map(String::as_str), Some("%3"));

    super::popup_ui::synchronize_key_targets(&mut key_targets, &[pane_three, pane_two]);
    assert_eq!(key_targets.get(&'3').map(String::as_str), Some("%3"));
    assert_eq!(key_targets.get(&'1').map(String::as_str), Some("%2"));
}

#[test]
fn popup_error_frame_includes_recovery_guidance() {
    let lines = super::popup_ui::render_error_frame("failed to read cache");
    assert_eq!(lines[0], "agentscan popup unavailable");
    assert!(lines.iter().any(|line| line.contains("popup --refresh")));
    assert!(lines.iter().any(|line| line.contains("Esc or Ctrl-C")));
    assert!(lines.iter().any(|line| line.contains("daemon run")));
}

#[test]
fn popup_session_order_appends_new_panes_without_reshuffling_existing_rows() {
    let current_order = vec![popup_test_pane(1), popup_test_pane(2), popup_test_pane(3)];
    let updated = vec![
        popup_test_pane(3),
        popup_test_pane(2),
        popup_test_pane(4),
        popup_test_pane(1),
    ];

    let merged = super::popup_ui::merge_popup_session_panes(&current_order, updated);
    let merged_ids: Vec<_> = merged.iter().map(|pane| pane.pane_id.as_str()).collect();

    assert_eq!(merged_ids, vec!["%1", "%2", "%3", "%4"]);
}

#[test]
fn popup_frame_paginates_and_limits_selection_to_visible_rows() {
    let panes = (1..=18).map(popup_test_pane).collect::<Vec<_>>();
    let mut state = super::popup_ui::PopupState::default();
    state.replace_panes(panes);

    let frame = super::popup_ui::render_popup_frame_for_size(
        &mut state,
        super::popup_ui::PopupTerminalSize {
            width: 120,
            height: 24,
        },
    );

    assert_eq!(frame.page_start, 0);
    assert_eq!(frame.page_size, 16);
    assert_eq!(frame.page_count, 2);
    assert_eq!(frame.visible_pane_ids.len(), 16);
    assert_eq!(frame.visible_pane_ids[0], "%1");
    assert_eq!(frame.visible_pane_ids[15], "%16");
    assert!(frame.lines.iter().any(|line| line.contains("Page 1/2")));
    assert!(!frame.lines.iter().any(|line| line.contains("Task 17")));
}

#[test]
fn popup_frame_clamps_to_last_non_empty_page_after_cache_removal() {
    let panes = (1..=18).map(popup_test_pane).collect::<Vec<_>>();
    let mut state = super::popup_ui::PopupState::default();
    state.replace_panes(panes);

    let full_height = super::popup_ui::PopupTerminalSize {
        width: 120,
        height: 24,
    };
    let first_frame = super::popup_ui::render_popup_frame_for_size(&mut state, full_height);
    assert_eq!(first_frame.page_size, 16);
    assert!(state.next_page());

    let second_page_frame = super::popup_ui::render_popup_frame_for_size(&mut state, full_height);
    assert_eq!(second_page_frame.page_start, 16);
    assert_eq!(second_page_frame.visible_pane_ids, vec!["%17", "%18"]);

    state.replace_panes((1..=10).map(popup_test_pane).collect());

    let clamped_frame = super::popup_ui::render_popup_frame_for_size(&mut state, full_height);
    assert_eq!(clamped_frame.page_start, 0);
    assert_eq!(clamped_frame.page_count, 1);
    assert_eq!(clamped_frame.visible_pane_ids[0], "%1");
    assert!(
        clamped_frame
            .lines
            .iter()
            .any(|line| line.contains("Page 1/1"))
    );
}

#[test]
fn popup_refresh_keeps_first_surviving_visible_pane_in_view() {
    let panes = (1..=18).map(popup_test_pane).collect::<Vec<_>>();
    let mut state = super::popup_ui::PopupState::default();
    state.replace_panes(panes);

    let full_height = super::popup_ui::PopupTerminalSize {
        width: 120,
        height: 24,
    };
    let first_frame = super::popup_ui::render_popup_frame_for_size(&mut state, full_height);
    assert_eq!(first_frame.page_size, 16);
    assert!(state.next_page());

    let second_page_frame = super::popup_ui::render_popup_frame_for_size(&mut state, full_height);
    assert_eq!(second_page_frame.visible_pane_ids, vec!["%17", "%18"]);

    state.replace_panes((2..=18).map(popup_test_pane).collect());

    let anchored_frame = super::popup_ui::render_popup_frame_for_size(&mut state, full_height);
    assert_eq!(anchored_frame.page_start, 15);
    assert_eq!(anchored_frame.visible_pane_ids, vec!["%17", "%18"]);
    assert!(
        anchored_frame
            .lines
            .iter()
            .any(|line| line.contains("Page 2/2"))
    );
}

#[test]
fn popup_refresh_reanchors_against_merged_pane_order() {
    let panes = (1..=18).map(popup_test_pane).collect::<Vec<_>>();
    let mut state = super::popup_ui::PopupState::default();
    state.replace_panes(panes);

    let full_height = super::popup_ui::PopupTerminalSize {
        width: 120,
        height: 24,
    };
    let first_frame = super::popup_ui::render_popup_frame_for_size(&mut state, full_height);
    assert_eq!(first_frame.page_size, 16);
    assert!(state.next_page());

    let second_page_frame = super::popup_ui::render_popup_frame_for_size(&mut state, full_height);
    assert_eq!(second_page_frame.page_start, 16);
    assert_eq!(second_page_frame.visible_pane_ids, vec!["%17", "%18"]);

    state.replace_panes(vec![
        popup_test_pane(1),
        popup_test_pane(2),
        popup_test_pane(3),
        popup_test_pane(19),
        popup_test_pane(4),
        popup_test_pane(5),
        popup_test_pane(6),
        popup_test_pane(7),
        popup_test_pane(8),
        popup_test_pane(9),
        popup_test_pane(10),
        popup_test_pane(11),
        popup_test_pane(12),
        popup_test_pane(13),
        popup_test_pane(14),
        popup_test_pane(15),
        popup_test_pane(16),
        popup_test_pane(17),
        popup_test_pane(18),
    ]);

    let anchored_frame = super::popup_ui::render_popup_frame_for_size(&mut state, full_height);
    assert_eq!(anchored_frame.page_start, 16);
    assert_eq!(anchored_frame.visible_pane_ids, vec!["%17", "%18", "%19"]);
    assert!(
        anchored_frame
            .lines
            .iter()
            .any(|line| line.contains("Page 2/2"))
    );
}

#[test]
fn popup_resize_keeps_visible_keys_stable_for_rows_that_remain_visible() {
    let panes = (1..=8).map(popup_test_pane).collect::<Vec<_>>();
    let mut state = super::popup_ui::PopupState::default();
    state.replace_panes(panes);

    let tall_frame = super::popup_ui::render_popup_frame_for_size(
        &mut state,
        super::popup_ui::PopupTerminalSize {
            width: 120,
            height: 6,
        },
    );
    assert_eq!(tall_frame.page_size, 4);
    assert!(tall_frame.lines[0].starts_with("[1]"));
    assert!(tall_frame.lines[1].starts_with("[2]"));
    assert!(tall_frame.lines[2].starts_with("[3]"));
    assert!(tall_frame.lines[3].starts_with("[4]"));

    let short_frame = super::popup_ui::render_popup_frame_for_size(
        &mut state,
        super::popup_ui::PopupTerminalSize {
            width: 120,
            height: 5,
        },
    );
    assert_eq!(short_frame.page_size, 3);
    assert!(short_frame.lines[0].starts_with("[1]"));
    assert!(short_frame.lines[1].starts_with("[2]"));
    assert!(short_frame.lines[2].starts_with("[3]"));
    assert!(
        short_frame
            .lines
            .iter()
            .any(|line| line.contains("Page 1/3"))
    );
}

#[test]
fn popup_small_viewport_renders_undersized_frame() {
    let mut state = super::popup_ui::PopupState::default();
    state.replace_panes(vec![popup_test_pane(1)]);

    let frame = super::popup_ui::render_popup_frame_for_size(
        &mut state,
        super::popup_ui::PopupTerminalSize {
            width: 40,
            height: 2,
        },
    );

    assert_eq!(frame.page_size, 0);
    assert!(
        frame
            .lines
            .iter()
            .any(|line| line.contains("Popup too small"))
    );
}

#[test]
fn popup_frame_lines_stay_within_terminal_width() {
    let panes = vec![popup_test_pane(1), popup_test_pane(2), popup_test_pane(3)];
    let mut state = super::popup_ui::PopupState::default();
    state.replace_panes(panes);

    let frame = super::popup_ui::render_popup_frame_for_size(
        &mut state,
        super::popup_ui::PopupTerminalSize {
            width: 24,
            height: 5,
        },
    );

    assert_eq!(frame.page_size, 3);
    assert!(
        frame
            .lines
            .iter()
            .all(|line| UnicodeWidthStr::width(line.as_str()) <= 24)
    );
}

#[test]
fn popup_frame_writer_avoids_newlines_for_full_height_frames() {
    let panes = (1..=16).map(popup_test_pane).collect::<Vec<_>>();
    let mut state = super::popup_ui::PopupState::default();
    state.replace_panes(panes);

    let frame = super::popup_ui::render_popup_frame_for_size(
        &mut state,
        super::popup_ui::PopupTerminalSize {
            width: 120,
            height: 18,
        },
    );
    assert_eq!(frame.lines.len(), 18);

    let mut rendered = Vec::new();
    super::popup_ui::write_popup_frame(&mut rendered, &frame).expect("frame should serialize");
    let rendered = String::from_utf8(rendered).expect("frame output should be utf-8");

    assert!(rendered.contains("Task 01"));
    assert!(!rendered.contains("\r\n"));
}

#[test]
fn popup_frame_writer_sanitizes_tmux_controlled_labels() {
    let pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "notes".to_string(),
        window_index: 4,
        pane_index: 1,
        pane_id: "%41".to_string(),
        pane_pid: 324026,
        pane_current_command: "claude".to_string(),
        pane_title_raw: "Claude Code | Working".to_string(),
        pane_tty: "/dev/pts/44".to_string(),
        pane_current_path: "/home/auro/notes".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: Some("Task\nnext\tstep".to_string()),
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
    let mut state = super::popup_ui::PopupState::default();
    state.replace_panes(vec![pane]);

    let frame = super::popup_ui::render_popup_frame_for_size(
        &mut state,
        super::popup_ui::PopupTerminalSize {
            width: 120,
            height: 3,
        },
    );
    assert_eq!(frame.lines[0], "[1] 🟡 \u{e76f} notes:4.1 - Task next step");
    assert!(!frame.lines[0].contains(['\n', '\r', '\t', '\u{1b}']));

    let mut rendered = Vec::new();
    super::popup_ui::write_popup_frame(&mut rendered, &frame).expect("frame should serialize");
    let rendered = String::from_utf8(rendered).expect("frame output should be utf-8");

    assert!(rendered.contains("Task next step"));
    assert!(!rendered.contains("Task\nnext\tstep"));
    assert!(!rendered.contains('\n'));
}

#[test]
fn tmux_target_is_missing_matches_common_focus_errors() {
    assert!(super::tmux::tmux_target_is_missing(b"can't find pane: %42"));
    assert!(super::tmux::tmux_target_is_missing(
        b"can't find window: @9"
    ));
    assert!(!super::tmux::tmux_target_is_missing(
        b"unknown command: switch-client"
    ));
}
