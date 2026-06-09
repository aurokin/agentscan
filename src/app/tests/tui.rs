fn tui_test_pane(pane_index: u32) -> PaneRecord {
    let command = if pane_index.is_multiple_of(2) {
        "claude"
    } else {
        "codex"
    };

    tmux_pane_row(pane_index)
        .session_name("alpha")
        .pane_index(pane_index)
        .command(command)
        .title(format!("Task {pane_index:02}"))
        .current_path("/tmp/alpha")
        .window_name("editor")
        .pane()
}

#[test]
fn tui_render_rows_include_location_status_and_key_labels() {
    let pane = tmux_pane_row(324026)
        .session_name("notes")
        .window_index(4)
        .pane_id("%41")
        .command("claude")
        .title("Claude Code | Working")
        .tty("/dev/pts/44")
        .current_path("/home/auro/notes")
        .pane();

    let mut key_targets = std::collections::BTreeMap::new();
    super::tui::synchronize_key_targets(&mut key_targets, std::slice::from_ref(&pane));

    let lines = super::tui::render_rows(&[pane], &key_targets);
    assert_eq!(lines, vec!["[1] 🟡 👾 notes:4.1 - Working"]);
}

#[test]
fn tui_render_frame_includes_workspace_and_full_location_for_cwd_grouping() {
    let pane = tmux_pane_row(324026)
        .session_name("notes")
        .window_index(4)
        .pane_id("%41")
        .command("claude")
        .title("Claude Code | Working")
        .tty("/dev/pts/44")
        .current_path("/home/auro/code/agentscan")
        .pane();
    let mut state = super::tui::TuiState::with_picker_config(
        super::picker::PickerKeySet::default(),
        super::picker::PickerGroupBy::Cwd,
    );
    state.replace_panes(vec![pane]);

    let frame = super::tui::render_tui_frame_for_size(
        &mut state,
        super::tui::TuiTerminalSize {
            width: 120,
            height: 10,
        },
    );

    assert_eq!(frame.lines[0], "[1] 🟡 👾 agentscan notes:4.1 - Working");
}

#[test]
fn provider_display_marker_uses_emoji_by_default() {
    assert_provider_markers(
        IconMode::Emoji,
        &[
            (Provider::Codex, "💭"),
            (Provider::Claude, "👾"),
            (Provider::Gemini, "✨"),
            (Provider::Antigravity, "🛸"),
            (Provider::Copilot, "🛫"),
            (Provider::CursorCli, "🌐"),
            (Provider::Pi, "🥧"),
            (Provider::Grok, "🚀"),
            (Provider::Hermes, "⚕️"),
            (Provider::Opencode, "🔲"),
            (Provider::Droid, "🏭"),
        ],
    );
    assert_eq!(super::provider_display_marker(None, IconMode::Emoji), "?");
}

#[test]
fn provider_display_marker_supports_nerd_font_mode() {
    assert_provider_markers(
        IconMode::NerdFont,
        &[
            (Provider::Codex, "\u{f4ac}"),
            (Provider::Claude, "\u{f0bc9}"),
            (Provider::Gemini, "\u{e370}"),
            (Provider::Antigravity, "\u{f02af}"),
            (Provider::Copilot, "\u{ec1e}"),
            (Provider::CursorCli, "\u{f01bf}"),
            (Provider::Pi, "\u{e22c}"),
            (Provider::Grok, "\u{f14de}"),
            (Provider::Hermes, "⚕"),
            (Provider::Opencode, "\u{f0168}"),
            (Provider::Droid, "\u{f020f}"),
        ],
    );
}

#[test]
fn provider_display_marker_supports_nerd_font_patched_mode() {
    assert_provider_markers(
        IconMode::NerdFontPatched,
        &[
            (Provider::Codex, "\u{100040}"),
            (Provider::Claude, "\u{100041}"),
            (Provider::Gemini, "\u{100044}"),
            (Provider::Antigravity, "\u{10004C}"),
            (Provider::Copilot, "\u{100049}"),
            (Provider::CursorCli, "\u{100042}"),
            (Provider::Pi, "\u{100052}"),
            (Provider::Grok, "\u{100051}"),
            (Provider::Hermes, "\u{100045}"),
            (Provider::Opencode, "\u{100043}"),
            (Provider::Droid, "\u{100056}"),
        ],
    );
}

fn assert_provider_markers(icon_mode: IconMode, expected: &[(Provider, &str)]) {
    for (provider, marker) in expected {
        assert_eq!(
            super::provider_display_marker(Some(*provider), icon_mode),
            *marker
        );
    }
}

#[test]
fn tui_render_rows_can_use_nerd_font_provider_markers() {
    let pane = tmux_pane_row(324026)
        .session_name("notes")
        .window_index(4)
        .pane_id("%41")
        .command("claude")
        .title("Claude Code | Working")
        .tty("/dev/pts/44")
        .current_path("/home/auro/notes")
        .pane();

    let mut key_targets = std::collections::BTreeMap::new();
    super::tui::synchronize_key_targets(&mut key_targets, std::slice::from_ref(&pane));

    let lines =
        super::tui::render_rows_for_width_with_icons(&[pane], &key_targets, usize::MAX, IconMode::NerdFont);
    assert_eq!(lines, vec!["[1] 🟡 \u{f0bc9} notes:4.1 - Working"]);
}

#[test]
fn tui_render_rows_can_use_nerd_font_patched_provider_markers() {
    let pane = tmux_pane_row(324026)
        .session_name("notes")
        .window_index(4)
        .pane_id("%41")
        .command("claude")
        .title("Claude Code | Working")
        .tty("/dev/pts/44")
        .current_path("/home/auro/notes")
        .pane();

    let mut key_targets = std::collections::BTreeMap::new();
    super::tui::synchronize_key_targets(&mut key_targets, std::slice::from_ref(&pane));

    let lines = super::tui::render_rows_for_width_with_icons(
        &[pane],
        &key_targets,
        usize::MAX,
        IconMode::NerdFontPatched,
    );
    assert_eq!(lines, vec!["[1] 🟡 \u{100041} notes:4.1 - Working"]);
}

#[test]
fn tui_render_rows_respect_terminal_cell_width_with_wide_status_emoji() {
    let pane = tmux_pane_row(324026)
        .session_name("notes")
        .window_index(4)
        .pane_id("%41")
        .command("claude")
        .title("Claude Code | Working on a much longer task title")
        .tty("/dev/pts/44")
        .current_path("/home/auro/notes")
        .pane();

    let mut key_targets = std::collections::BTreeMap::new();
    super::tui::synchronize_key_targets(&mut key_targets, std::slice::from_ref(&pane));

    let width = 28;
    let lines = super::tui::render_rows_for_width(&[pane], &key_targets, width);

    assert_eq!(lines.len(), 1);
    assert!(lines[0].ends_with('…'));
    assert!(UnicodeWidthStr::width(lines[0].as_str()) <= width);
}

#[test]
fn tui_render_rows_sanitize_control_characters_and_escape_sequences() {
    let pane = tmux_pane_row(324026)
        .session_name("notes")
        .window_index(4)
        .pane_id("%41")
        .command("claude")
        .title("Claude Code | Working")
        .tty("/dev/pts/44")
        .current_path("/home/auro/notes")
        .agent_label("Task\nnext\r\tstep \u{1b}[31mnow\u{1b}[0m\u{7}")
        .pane();

    let mut key_targets = std::collections::BTreeMap::new();
    super::tui::synchronize_key_targets(&mut key_targets, std::slice::from_ref(&pane));

    let lines = super::tui::render_rows(&[pane], &key_targets);
    assert_eq!(
        lines,
        vec!["[1] 🟡 👾 notes:4.1 - Task next step now"]
    );
}

#[test]
fn tui_key_assignments_stay_stable_across_rerenders() {
    let pane_one = tui_test_pane(1);
    let pane_two = tui_test_pane(2);
    let pane_three = tui_test_pane(3);

    let mut key_targets = std::collections::BTreeMap::new();
    super::tui::synchronize_key_targets(
        &mut key_targets,
        &[pane_one.clone(), pane_two.clone(), pane_three.clone()],
    );
    assert_eq!(key_targets.get(&'1').map(String::as_str), Some("%1"));
    assert_eq!(key_targets.get(&'2').map(String::as_str), Some("%2"));
    assert_eq!(key_targets.get(&'3').map(String::as_str), Some("%3"));

    super::tui::synchronize_key_targets(
        &mut key_targets,
        &[pane_one.clone(), pane_three.clone()],
    );
    assert_eq!(key_targets.get(&'1').map(String::as_str), Some("%1"));
    assert_eq!(key_targets.get(&'3').map(String::as_str), Some("%3"));

    super::tui::synchronize_key_targets(&mut key_targets, &[pane_three, pane_two]);
    assert_eq!(key_targets.get(&'3').map(String::as_str), Some("%3"));
    assert_eq!(key_targets.get(&'1').map(String::as_str), Some("%2"));
}

#[test]
fn tui_key_assignments_reset_after_workspace_reorder() {
    let pane_one = tmux_pane_row(1)
        .session_name("work")
        .pane_id("%1")
        .command("codex")
        .title("Task 1")
        .current_path("/work/beta")
        .pane();
    let pane_two = tmux_pane_row(2)
        .session_name("work")
        .pane_id("%2")
        .command("codex")
        .title("Task 2")
        .current_path("/work/gamma")
        .pane();
    let mut state = super::tui::TuiState::with_picker_config(
        super::picker::PickerKeySet::default(),
        super::picker::PickerGroupBy::Cwd,
    );
    let terminal_size = super::tui::TuiTerminalSize {
        width: 120,
        height: 10,
    };
    state.replace_panes(vec![pane_one.clone(), pane_two]);
    super::tui::render_tui_frame_for_size(&mut state, terminal_size);
    assert_eq!(state.test_key_target('1'), Some("%1"));
    assert_eq!(state.test_key_target('2'), Some("%2"));

    let moved_pane_two = tmux_pane_row(2)
        .session_name("work")
        .pane_id("%2")
        .command("codex")
        .title("Task 2")
        .current_path("/work/alpha")
        .pane();
    state.replace_panes(vec![pane_one, moved_pane_two]);
    let frame = super::tui::render_tui_frame_for_size(&mut state, terminal_size);

    assert_eq!(frame.visible_pane_ids, vec!["%2", "%1"]);
    assert_eq!(state.test_key_target('1'), Some("%2"));
    assert_eq!(state.test_key_target('2'), Some("%1"));
    assert_eq!(state.test_retired_key_target('1'), None);
}

#[test]
fn tui_key_assignments_reset_when_workspace_insertion_shifts_visible_rows() {
    let pane_one = tmux_pane_row(1)
        .session_name("work")
        .pane_id("%1")
        .command("codex")
        .title("Task 1")
        .current_path("/work/alpha")
        .pane();
    let pane_two = tmux_pane_row(2)
        .session_name("work")
        .pane_id("%2")
        .command("codex")
        .title("Task 2")
        .current_path("/work/gamma")
        .pane();
    let inserted_pane = tmux_pane_row(3)
        .session_name("work")
        .pane_id("%3")
        .command("codex")
        .title("Task 3")
        .current_path("/work/beta")
        .pane();
    let mut state = super::tui::TuiState::with_picker_config(
        super::picker::PickerKeySet::default(),
        super::picker::PickerGroupBy::Cwd,
    );
    let terminal_size = super::tui::TuiTerminalSize {
        width: 120,
        height: 10,
    };
    state.replace_panes(vec![pane_one.clone(), pane_two.clone()]);
    super::tui::render_tui_frame_for_size(&mut state, terminal_size);
    assert_eq!(state.test_key_target('1'), Some("%1"));
    assert_eq!(state.test_key_target('2'), Some("%2"));

    state.replace_panes(vec![pane_one, pane_two, inserted_pane]);
    let frame = super::tui::render_tui_frame_for_size(&mut state, terminal_size);

    assert_eq!(frame.visible_pane_ids, vec!["%1", "%3", "%2"]);
    assert_eq!(state.test_key_target('1'), Some("%1"));
    assert_eq!(state.test_key_target('2'), Some("%3"));
    assert_eq!(state.test_key_target('3'), Some("%2"));
}

#[test]
fn tui_workspace_reorder_reanchors_to_previous_visible_pane_page() {
    let pane = |index: u32, cwd: String| {
        tmux_pane_row(index)
            .session_name("work")
            .pane_id(format!("%{index}"))
            .command("codex")
            .title(format!("Task {index}"))
            .current_path(cwd)
            .pane()
    };
    let panes = (1..=8)
        .map(|index| pane(index, format!("/work/p{index:02}")))
        .collect::<Vec<_>>();
    let mut state = super::tui::TuiState::with_picker_config(
        super::picker::PickerKeySet::default(),
        super::picker::PickerGroupBy::Cwd,
    );
    let terminal_size = super::tui::TuiTerminalSize {
        width: 120,
        height: 6,
    };
    state.replace_panes(panes);
    let first_frame = super::tui::render_tui_frame_for_size(&mut state, terminal_size);
    assert_eq!(first_frame.page_size, 4);
    assert!(state.next_page());
    let second_frame = super::tui::render_tui_frame_for_size(&mut state, terminal_size);
    assert_eq!(second_frame.visible_pane_ids, vec!["%5", "%6", "%7", "%8"]);

    let reordered = (1..=8)
        .map(|index| {
            let cwd = if index == 5 {
                "/work/p00".to_string()
            } else {
                format!("/work/p{index:02}")
            };
            pane(index, cwd)
        })
        .collect::<Vec<_>>();
    state.replace_panes(reordered);
    let anchored_frame = super::tui::render_tui_frame_for_size(&mut state, terminal_size);

    assert_eq!(anchored_frame.page_start, 0);
    assert_eq!(anchored_frame.visible_pane_ids[0], "%5");
    assert_eq!(state.test_key_target('1'), Some("%5"));
}

#[test]
fn tui_retains_retired_key_targets_for_missing_pane_selection() {
    let pane_one = tui_test_pane(1);
    let pane_two = tui_test_pane(2);
    let mut state = super::tui::TuiState::default();
    let terminal_size = super::tui::TuiTerminalSize {
        width: 80,
        height: 12,
    };
    state.replace_panes(vec![pane_one.clone(), pane_two]);

    super::tui::render_tui_frame_for_size(&mut state, terminal_size);
    assert_eq!(state.test_key_target('2'), Some("%2"));

    state.replace_panes(vec![pane_one]);
    super::tui::render_tui_frame_for_size(&mut state, terminal_size);

    assert_eq!(
        state.test_retired_key_target('2'),
        Some("%2")
    );
}

#[test]
fn tui_removal_does_not_reuse_missing_pane_key_before_retiring_it() {
    let pane_one = tui_test_pane(1);
    let pane_two = tui_test_pane(2);
    let mut state = super::tui::TuiState::default();
    let terminal_size = super::tui::TuiTerminalSize {
        width: 80,
        height: 12,
    };
    state.replace_panes(vec![pane_one, pane_two.clone()]);

    super::tui::render_tui_frame_for_size(&mut state, terminal_size);
    assert_eq!(state.test_key_target('1'), Some("%1"));
    assert_eq!(state.test_key_target('2'), Some("%2"));

    state.replace_panes(vec![pane_two]);
    super::tui::render_tui_frame_for_size(&mut state, terminal_size);

    assert_eq!(state.test_key_target('1'), None);
    assert_eq!(state.test_key_target('2'), Some("%2"));
    assert_eq!(state.test_retired_key_target('1'), Some("%1"));
}

#[test]
fn tui_non_session_removal_resets_shifted_visible_keys() {
    let pane_one = tmux_pane_row(1)
        .session_name("work")
        .pane_id("%1")
        .command("codex")
        .title("Task 1")
        .current_path("/work/alpha")
        .pane();
    let pane_two = tmux_pane_row(2)
        .session_name("work")
        .pane_id("%2")
        .command("codex")
        .title("Task 2")
        .current_path("/work/beta")
        .pane();
    let mut state = super::tui::TuiState::with_picker_config(
        super::picker::PickerKeySet::default(),
        super::picker::PickerGroupBy::Cwd,
    );
    let terminal_size = super::tui::TuiTerminalSize {
        width: 80,
        height: 12,
    };
    state.replace_panes(vec![pane_one, pane_two.clone()]);

    super::tui::render_tui_frame_for_size(&mut state, terminal_size);
    assert_eq!(state.test_key_target('1'), Some("%1"));
    assert_eq!(state.test_key_target('2'), Some("%2"));

    state.replace_panes(vec![pane_two]);
    super::tui::render_tui_frame_for_size(&mut state, terminal_size);

    assert_eq!(state.test_key_target('1'), Some("%2"));
    assert_eq!(state.test_key_target('2'), None);
    assert_eq!(state.test_retired_key_target('1'), None);
}

#[test]
fn tui_error_frame_includes_recovery_guidance() {
    let lines = super::tui::render_error_frame("failed to connect to daemon");
    assert_eq!(lines[0], "agentscan tui unavailable");
    assert!(lines.iter().any(|line| line.contains("daemon status")));
    assert!(lines.iter().any(|line| line.contains("Esc or Ctrl-C")));
    assert!(!lines.iter().any(|line| line.contains("tui --refresh")));
}

#[test]
fn tui_connecting_frame_renders_before_bootstrap() {
    let mut state = super::tui::TuiState::default();
    state.set_connecting("starting daemon".to_string());

    let frame = super::tui::render_tui_frame_for_size(
        &mut state,
        super::tui::TuiTerminalSize {
            width: 80,
            height: 6,
        },
    );

    assert!(
        frame
            .lines
            .iter()
            .any(|line| line.contains("Connecting to agentscan daemon"))
    );
    assert!(frame.lines.iter().any(|line| line.contains("[connecting]")));
    assert!(frame.lines.iter().any(|line| line.contains("starting daemon")));
}

#[test]
fn tui_offline_state_preserves_last_snapshot_rows() {
    let mut state = super::tui::TuiState::default();
    state.replace_panes(vec![tui_test_pane(1)]);
    state.set_offline("daemon subscription closed".to_string(), true);

    let frame = super::tui::render_tui_frame_for_size(
        &mut state,
        super::tui::TuiTerminalSize {
            width: 80,
            height: 5,
        },
    );

    assert!(frame.lines.iter().any(|line| line.contains("Task 01")));
    assert!(frame.lines.iter().any(|line| line.contains("[reconnecting]")));
    assert!(
        frame
            .lines
            .iter()
            .any(|line| line.contains("daemon subscription closed"))
    );
}

#[test]
fn tui_shutdown_state_preserves_last_snapshot_rows() {
    let mut state = super::tui::TuiState::default();
    state.replace_panes(vec![tui_test_pane(1)]);
    state.set_shutdown("daemon socket server is closing".to_string());

    let frame = super::tui::render_tui_frame_for_size(
        &mut state,
        super::tui::TuiTerminalSize {
            width: 80,
            height: 5,
        },
    );

    assert!(frame.lines.iter().any(|line| line.contains("Task 01")));
    assert!(frame.lines.iter().any(|line| line.contains("[shutdown]")));
    assert!(
        frame
            .lines
            .iter()
            .any(|line| line.contains("daemon socket server is closing"))
    );
}

#[test]
fn tui_unavailable_frame_omits_refresh_guidance() {
    let mut state = super::tui::TuiState::default();
    state.set_unavailable("unsupported daemon protocol".to_string());

    let frame = super::tui::render_tui_frame_for_size(
        &mut state,
        super::tui::TuiTerminalSize {
            width: 80,
            height: 8,
        },
    );

    assert!(frame.lines.iter().any(|line| line.contains("[unavailable]")));
    assert!(
        frame
            .lines
            .iter()
            .any(|line| line.contains("unsupported daemon protocol"))
    );
    assert!(!frame.lines.iter().any(|line| line.contains("tui --refresh")));
}

#[test]
fn tui_footer_connection_indicator_fits_narrow_widths() {
    for width in [0, 1, 8, 16, 24] {
        let mut state = super::tui::TuiState::default();
        state.replace_panes(vec![tui_test_pane(1)]);
        state.set_offline("a very long reconnecting status message".to_string(), true);

        let frame = super::tui::render_tui_frame_for_size(
            &mut state,
            super::tui::TuiTerminalSize { width, height: 4 },
        );

        assert!(
            frame
                .lines
                .iter()
                .all(|line| UnicodeWidthStr::width(line.as_str()) <= usize::from(width)),
            "line exceeded width {width}: {:?}",
            frame.lines
        );
    }
}

#[test]
fn tui_session_order_appends_new_panes_without_reshuffling_existing_rows() {
    let current_order = vec![tui_test_pane(1), tui_test_pane(2), tui_test_pane(3)];
    let updated = vec![
        tui_test_pane(3),
        tui_test_pane(2),
        tui_test_pane(4),
        tui_test_pane(1),
    ];

    let merged = super::tui::merge_tui_session_panes(&current_order, updated);
    let merged_ids: Vec<_> = merged.iter().map(|pane| pane.pane_id.as_str()).collect();

    assert_eq!(merged_ids, vec!["%1", "%2", "%3", "%4"]);
}

#[test]
fn tui_frame_paginates_and_limits_selection_to_visible_rows() {
    let panes = (1..=18).map(tui_test_pane).collect::<Vec<_>>();
    let mut state = super::tui::TuiState::default();
    state.replace_panes(panes);

    let frame = super::tui::render_tui_frame_for_size(
        &mut state,
        super::tui::TuiTerminalSize {
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
fn tui_frame_uses_custom_picker_key_order() {
    let picker_keys = super::picker::PickerKeySet::from_config_values(&custom_picker_key_values())
        .expect("custom key set should parse");
    let panes = (1..=5).map(tui_test_pane).collect::<Vec<_>>();
    let mut state = super::tui::TuiState::with_picker_keys(picker_keys);
    state.replace_panes(panes);

    let frame = super::tui::render_tui_frame_for_size(
        &mut state,
        super::tui::TuiTerminalSize {
            width: 120,
            height: 12,
        },
    );

    assert!(frame.lines[0].starts_with("[A]"));
    assert!(frame.lines[1].starts_with("[S]"));
    assert!(frame.lines.iter().any(|line| line.contains("Task 03")));
    assert!(frame.lines.iter().any(|line| line.contains("Page 1/1")));
}

#[test]
fn tui_frame_clamps_to_last_non_empty_page_after_snapshot_removal() {
    let panes = (1..=18).map(tui_test_pane).collect::<Vec<_>>();
    let mut state = super::tui::TuiState::default();
    state.replace_panes(panes);

    let full_height = super::tui::TuiTerminalSize {
        width: 120,
        height: 24,
    };
    let first_frame = super::tui::render_tui_frame_for_size(&mut state, full_height);
    assert_eq!(first_frame.page_size, 16);
    assert!(state.next_page());

    let second_page_frame = super::tui::render_tui_frame_for_size(&mut state, full_height);
    assert_eq!(second_page_frame.page_start, 16);
    assert_eq!(second_page_frame.visible_pane_ids, vec!["%17", "%18"]);

    state.replace_panes((1..=10).map(tui_test_pane).collect());

    let clamped_frame = super::tui::render_tui_frame_for_size(&mut state, full_height);
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
fn tui_refresh_keeps_first_surviving_visible_pane_in_view() {
    let panes = (1..=18).map(tui_test_pane).collect::<Vec<_>>();
    let mut state = super::tui::TuiState::default();
    state.replace_panes(panes);

    let full_height = super::tui::TuiTerminalSize {
        width: 120,
        height: 24,
    };
    let first_frame = super::tui::render_tui_frame_for_size(&mut state, full_height);
    assert_eq!(first_frame.page_size, 16);
    assert!(state.next_page());

    let second_page_frame = super::tui::render_tui_frame_for_size(&mut state, full_height);
    assert_eq!(second_page_frame.visible_pane_ids, vec!["%17", "%18"]);

    state.replace_panes((2..=18).map(tui_test_pane).collect());

    let anchored_frame = super::tui::render_tui_frame_for_size(&mut state, full_height);
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
fn tui_refresh_reanchors_against_merged_pane_order() {
    let panes = (1..=18).map(tui_test_pane).collect::<Vec<_>>();
    let mut state = super::tui::TuiState::default();
    state.replace_panes(panes);

    let full_height = super::tui::TuiTerminalSize {
        width: 120,
        height: 24,
    };
    let first_frame = super::tui::render_tui_frame_for_size(&mut state, full_height);
    assert_eq!(first_frame.page_size, 16);
    assert!(state.next_page());

    let second_page_frame = super::tui::render_tui_frame_for_size(&mut state, full_height);
    assert_eq!(second_page_frame.page_start, 16);
    assert_eq!(second_page_frame.visible_pane_ids, vec!["%17", "%18"]);

    state.replace_panes(vec![
        tui_test_pane(1),
        tui_test_pane(2),
        tui_test_pane(3),
        tui_test_pane(19),
        tui_test_pane(4),
        tui_test_pane(5),
        tui_test_pane(6),
        tui_test_pane(7),
        tui_test_pane(8),
        tui_test_pane(9),
        tui_test_pane(10),
        tui_test_pane(11),
        tui_test_pane(12),
        tui_test_pane(13),
        tui_test_pane(14),
        tui_test_pane(15),
        tui_test_pane(16),
        tui_test_pane(17),
        tui_test_pane(18),
    ]);

    let anchored_frame = super::tui::render_tui_frame_for_size(&mut state, full_height);
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
fn tui_resize_keeps_visible_keys_stable_for_rows_that_remain_visible() {
    let panes = (1..=8).map(tui_test_pane).collect::<Vec<_>>();
    let mut state = super::tui::TuiState::default();
    state.replace_panes(panes);

    let tall_frame = super::tui::render_tui_frame_for_size(
        &mut state,
        super::tui::TuiTerminalSize {
            width: 120,
            height: 6,
        },
    );
    assert_eq!(tall_frame.page_size, 4);
    assert!(tall_frame.lines[0].starts_with("[1]"));
    assert!(tall_frame.lines[1].starts_with("[2]"));
    assert!(tall_frame.lines[2].starts_with("[3]"));
    assert!(tall_frame.lines[3].starts_with("[4]"));

    let short_frame = super::tui::render_tui_frame_for_size(
        &mut state,
        super::tui::TuiTerminalSize {
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
fn tui_small_viewport_renders_undersized_frame() {
    let mut state = super::tui::TuiState::default();
    state.replace_panes(vec![tui_test_pane(1)]);

    let frame = super::tui::render_tui_frame_for_size(
        &mut state,
        super::tui::TuiTerminalSize {
            width: 40,
            height: 2,
        },
    );

    assert_eq!(frame.page_size, 0);
    assert!(
        frame
            .lines
            .iter()
            .any(|line| line.contains("TUI too small"))
    );
}

#[test]
fn tui_frame_lines_stay_within_terminal_width() {
    let panes = vec![tui_test_pane(1), tui_test_pane(2), tui_test_pane(3)];
    let mut state = super::tui::TuiState::default();
    state.replace_panes(panes);

    let frame = super::tui::render_tui_frame_for_size(
        &mut state,
        super::tui::TuiTerminalSize {
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
fn tui_frame_writer_avoids_newlines_for_full_height_frames() {
    let panes = (1..=16).map(tui_test_pane).collect::<Vec<_>>();
    let mut state = super::tui::TuiState::default();
    state.replace_panes(panes);

    let frame = super::tui::render_tui_frame_for_size(
        &mut state,
        super::tui::TuiTerminalSize {
            width: 120,
            height: 18,
        },
    );
    assert_eq!(frame.lines.len(), 18);

    let mut rendered = Vec::new();
    super::tui::write_tui_frame(&mut rendered, &frame).expect("frame should serialize");
    let rendered = String::from_utf8(rendered).expect("frame output should be utf-8");

    assert!(rendered.contains("Task 01"));
    assert!(!rendered.contains("\r\n"));
}

#[test]
fn tui_frame_writer_sanitizes_tmux_controlled_labels() {
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
        pane_active: false,
        window_active: false,
    });
    let mut state = super::tui::TuiState::default();
    state.replace_panes(vec![pane]);

    let frame = super::tui::render_tui_frame_for_size(
        &mut state,
        super::tui::TuiTerminalSize {
            width: 120,
            height: 3,
        },
    );
    assert_eq!(frame.lines[0], "[1] 🟡 👾 notes:4.1 - Task next step");
    assert!(!frame.lines[0].contains(['\n', '\r', '\t', '\u{1b}']));

    let mut rendered = Vec::new();
    super::tui::write_tui_frame(&mut rendered, &frame).expect("frame should serialize");
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
