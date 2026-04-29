#[test]
fn classifies_from_command() {
    let matched = classify::classify_provider(None, "codex", "").expect("should match codex");
    assert_eq!(matched.provider, Provider::Codex);
    assert_eq!(
        matched.matched_by,
        super::ClassificationMatchKind::PaneCurrentCommand
    );
    assert_eq!(
        matched.confidence,
        super::ClassificationConfidence::High,
        "exact canonical binary match should be high confidence"
    );

    let suffixed = classify::classify_provider(None, "codex-exec", "")
        .expect("suffixed codex binary should still classify");
    assert_eq!(suffixed.provider, Provider::Codex);
    assert_eq!(
        suffixed.confidence,
        super::ClassificationConfidence::Medium,
        "suffixed binary match should stay medium confidence"
    );

    let gemini_cli = classify::classify_provider(None, "gemini-cli", "")
        .expect("gemini-cli should classify as Gemini");
    assert_eq!(gemini_cli.provider, Provider::Gemini);
    assert_eq!(
        gemini_cli.matched_by,
        super::ClassificationMatchKind::PaneCurrentCommand
    );
    assert_eq!(
        gemini_cli.confidence,
        super::ClassificationConfidence::Medium,
        "suffixed gemini binary should stay medium confidence"
    );
}

#[test]
fn rejects_suffixed_binaries_for_generic_word_providers() {
    assert!(
        classify::classify_provider(None, "copilot-backend", "").is_none(),
        "copilot suffix should not classify as Copilot"
    );
    assert!(
        classify::classify_provider(None, "cursor-agent-beta", "").is_none(),
        "cursor-agent suffix should not classify as CursorCli"
    );
    assert!(
        classify::classify_provider(None, "pi-coding-agent-foo", "").is_none(),
        "pi-coding-agent suffix should not classify as Pi"
    );
}

#[test]
fn classifies_copilot_and_cursor_cli_from_command() {
    let copilot = classify::classify_provider(None, "copilot", "").expect("should match copilot");
    assert_eq!(copilot.provider, Provider::Copilot);
    assert_eq!(
        copilot.matched_by,
        super::ClassificationMatchKind::PaneCurrentCommand
    );

    let github_copilot = classify::classify_provider(None, "github-copilot", "")
        .expect("should match github-copilot");
    assert_eq!(github_copilot.provider, Provider::Copilot);
    assert_eq!(
        github_copilot.matched_by,
        super::ClassificationMatchKind::PaneCurrentCommand
    );

    let cursor =
        classify::classify_provider(None, "cursor-agent", "").expect("should match cursor cli");
    assert_eq!(cursor.provider, Provider::CursorCli);
    assert_eq!(
        cursor.matched_by,
        super::ClassificationMatchKind::PaneCurrentCommand
    );

    let plain_cursor = classify::classify_provider(None, "cursor", "");
    assert!(
        plain_cursor.is_none(),
        "plain cursor launcher should not match cursor cli"
    );
}

#[test]
fn classifies_pi_from_specific_command_and_title() {
    let command = classify::classify_provider(None, "pi-coding-agent", "")
        .expect("should match pi coding agent");
    assert_eq!(command.provider, Provider::Pi);
    assert_eq!(
        command.matched_by,
        super::ClassificationMatchKind::PaneCurrentCommand
    );

    let title = classify::classify_provider(None, "pi", "pi - refactor - agentscan")
        .expect("bare pi command plus task title should match");
    assert_eq!(title.provider, Provider::Pi);
    assert_eq!(
        title.matched_by,
        super::ClassificationMatchKind::PaneCurrentCommand
    );
}

#[test]
fn does_not_classify_bare_pi_command_without_other_signal() {
    let bare = classify::classify_provider(None, "pi", "");
    let generic_title = classify::classify_provider(None, "pi", "pi - agentscan");

    assert!(bare.is_none(), "bare pi command should not match");
    assert!(
        generic_title.is_none(),
        "bare pi command plus generic title should not match"
    );
}

#[test]
fn classifies_from_title_when_command_is_generic() {
    let matched = classify::classify_provider(None, "zsh", "Claude Code | Working")
        .expect("should match claude");
    assert_eq!(matched.provider, Provider::Claude);
    assert_eq!(
        matched.matched_by,
        super::ClassificationMatchKind::PaneTitle
    );

    let gemini = classify::classify_provider(None, "zsh", "◇  Ready (workspace)")
        .expect("should match gemini");
    assert_eq!(gemini.provider, Provider::Gemini);
    assert_eq!(gemini.matched_by, super::ClassificationMatchKind::PaneTitle);

    let opencode_default =
        classify::classify_provider(None, "zsh", "OpenCode").expect("should match opencode");
    assert_eq!(opencode_default.provider, Provider::Opencode);
    assert_eq!(
        opencode_default.matched_by,
        super::ClassificationMatchKind::PaneTitle
    );

    let opencode_session = classify::classify_provider(None, "zsh", "OC | Query planner")
        .expect("should match opencode session title");
    assert_eq!(opencode_session.provider, Provider::Opencode);
    assert_eq!(
        opencode_session.matched_by,
        super::ClassificationMatchKind::PaneTitle
    );

    let copilot_default = classify::classify_provider(None, "zsh", "GitHub Copilot")
        .expect("should match default GitHub Copilot title");
    assert_eq!(copilot_default.provider, Provider::Copilot);
    assert_eq!(
        copilot_default.matched_by,
        super::ClassificationMatchKind::PaneTitle
    );
}

#[test]
fn classifies_cursor_agent_from_bare_title_when_command_is_generic() {
    let matched =
        classify::classify_provider(None, "zsh", "Cursor Agent").expect("should match cursor cli");
    assert_eq!(matched.provider, Provider::CursorCli);
    assert_eq!(
        matched.matched_by,
        super::ClassificationMatchKind::PaneTitle
    );
}

#[test]
fn opencode_title_matches_stay_exact() {
    assert!(
        classify::classify_provider(None, "zsh", "OpenCoder").is_none(),
        "nearby product names should not classify as opencode"
    );
    assert!(
        classify::classify_provider(None, "zsh", "Review opencode implementation").is_none(),
        "generic mentions should not classify as opencode"
    );
}

#[test]
fn gemini_mentions_in_titles_do_not_classify_generic_panes() {
    assert!(
        classify::classify_provider(None, "zsh", "Clone Gemini CLI open source library").is_none()
    );
    assert!(
        classify::classify_provider(None, "2.1.119", "✳ Clone Gemini CLI open source library")
            .is_none()
    );
    assert!(
        classify::classify_provider(None, "zsh", "✦ Process deployment").is_none(),
        "arbitrary sparkle titles without Gemini context should not classify"
    );
    assert!(
        classify::classify_provider(None, "zsh", "✦  Process deployment").is_none(),
        "two-space sparkle titles without Gemini context should not classify"
    );
    assert!(
        classify::classify_provider(None, "zsh", "◇ Ready for deploy").is_none(),
        "Gemini ready glyph must match the upstream title shape"
    );
    assert!(
        classify::classify_provider(None, "zsh", "◇ Ready").is_none(),
        "Gemini ready glyph without upstream context should not classify"
    );
}

#[test]
fn classifies_from_command_before_conflicting_title() {
    let cursor_title = classify::classify_provider(None, "codex", "Cursor | Working")
        .expect("command should beat conflicting cursor title");
    assert_eq!(cursor_title.provider, Provider::Codex);
    assert_eq!(
        cursor_title.matched_by,
        super::ClassificationMatchKind::PaneCurrentCommand
    );

    let pi_title = classify::classify_provider(None, "codex", "pi - refactor - agentscan")
        .expect("command should beat conflicting pi title");
    assert_eq!(pi_title.provider, Provider::Codex);
    assert_eq!(
        pi_title.matched_by,
        super::ClassificationMatchKind::PaneCurrentCommand
    );
}

#[test]
fn codex_shaped_titles_win_before_pi_heuristic() {
    let matched = classify::classify_provider(None, "zsh", "pi - refactor - agentscan: codex")
        .expect("codex-shaped title should still classify as codex");
    assert_eq!(matched.provider, Provider::Codex);
    assert_eq!(
        matched.matched_by,
        super::ClassificationMatchKind::PaneTitle
    );
}

#[test]
fn codex_shaped_titles_win_before_copilot_and_cursor_prefixes() {
    let copilot = classify::classify_provider(None, "zsh", "Copilot | review patch: codex")
        .expect("codex-shaped copilot wrapper title should classify as codex");
    assert_eq!(copilot.provider, Provider::Codex);
    assert_eq!(
        copilot.matched_by,
        super::ClassificationMatchKind::PaneTitle
    );

    let cursor = classify::classify_provider(None, "zsh", "Cursor CLI | parser work: codex")
        .expect("codex-shaped cursor wrapper title should classify as codex");
    assert_eq!(cursor.provider, Provider::Codex);
    assert_eq!(cursor.matched_by, super::ClassificationMatchKind::PaneTitle);
}

#[test]
fn classifies_from_pane_metadata_before_title_and_command() {
    let matched = classify::classify_provider(Some("codex"), "zsh", "Claude Code | Working")
        .expect("pane metadata should match codex");
    assert_eq!(matched.provider, Provider::Codex);
    assert_eq!(
        matched.matched_by,
        super::ClassificationMatchKind::PaneMetadata
    );
}

#[test]
fn classifies_copilot_and_cursor_cli_from_metadata_aliases() {
    let copilot = classify::classify_provider(Some("github-copilot"), "zsh", "Cursor CLI | Ready")
        .expect("pane metadata should match copilot");
    assert_eq!(copilot.provider, Provider::Copilot);
    assert_eq!(
        copilot.matched_by,
        super::ClassificationMatchKind::PaneMetadata
    );

    let cursor = classify::classify_provider(Some("cursor cli"), "zsh", "Copilot | Working")
        .expect("pane metadata should match cursor cli");
    assert_eq!(cursor.provider, Provider::CursorCli);
    assert_eq!(
        cursor.matched_by,
        super::ClassificationMatchKind::PaneMetadata
    );

    let cursor_agent =
        classify::classify_provider(Some("cursor-agent"), "zsh", "Copilot | Working")
            .expect("cursor-agent metadata should match cursor cli");
    assert_eq!(cursor_agent.provider, Provider::CursorCli);
    assert_eq!(
        cursor_agent.matched_by,
        super::ClassificationMatchKind::PaneMetadata
    );
}

#[test]
fn classifies_pi_from_metadata_aliases() {
    let pi = classify::classify_provider(Some("pi-coding-agent"), "zsh", "Claude Code | Working")
        .expect("pane metadata should match pi");
    assert_eq!(pi.provider, Provider::Pi);
    assert_eq!(pi.matched_by, super::ClassificationMatchKind::PaneMetadata);
}

#[test]
fn provider_metadata_table_covers_aliases_commands_and_summary_order() {
    for (alias, expected) in [
        ("codex", Provider::Codex),
        ("claude", Provider::Claude),
        ("gemini", Provider::Gemini),
        ("opencode", Provider::Opencode),
        ("github copilot", Provider::Copilot),
        ("cursor-cli", Provider::CursorCli),
        ("cursor cli", Provider::CursorCli),
        ("pi coding agent", Provider::Pi),
    ] {
        assert_eq!(
            super::provider_from_metadata(Some(alias)),
            Some(expected),
            "metadata alias: {alias}"
        );
    }

    assert_eq!(super::provider_from_metadata(Some(" unknown ")), None);
    assert_eq!(super::provider_from_command("codex-exec"), Some((Provider::Codex, false)));
    assert_eq!(
        super::provider_from_command("cursor-agent-beta"),
        None,
        "generic provider names should not accept suffixed binaries"
    );
    assert_eq!(
        super::provider_summary_order().collect::<Vec<_>>(),
        vec![
            Provider::Codex,
            Provider::Claude,
            Provider::Gemini,
            Provider::Opencode,
            Provider::Copilot,
            Provider::CursorCli,
            Provider::Pi,
        ]
    );
}

#[test]
fn parses_tmux_output_into_rows() {
    let input = concat!(
        "dotfiles\x1f1\x1f1\x1f%50\x1f438455\x1fcodex\x1f(bront) .dotfiles: codex\x1f/dev/pts/55\x1f/home/auro/.dotfiles\x1feditor\n",
        "notes\x1f4\x1f1\x1f%41\x1f324026\x1fclaude\x1fClaude Code\x1f/dev/pts/44\x1f/home/auro/notes\x1fquery\n"
    );

    let rows = tmux::parse_pane_rows(input).expect("tmux output should parse");

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].pane_id, "%50");
    assert_eq!(rows[1].pane_title_raw, "Claude Code");
}

#[test]
fn parses_tmux_output_with_session_and_window_ids() {
    let input = "notes\x1f4\x1f1\x1f%41\x1f324026\x1fclaude\x1fClaude Code\x1f/dev/pts/44\x1f/home/auro/notes\x1fquery\x1f$7\x1f@9\n";

    let rows = tmux::parse_pane_rows(input).expect("tmux output with ids should parse");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].session_id.as_deref(), Some("$7"));
    assert_eq!(rows[0].window_id.as_deref(), Some("@9"));
}

#[test]
fn parses_tmux_output_with_escaped_delimiters() {
    let input = r"notes\0374\0371\037%41\037324026\037claude\037Claude Code\037/dev/pts/44\037/home/auro/notes\037query\037$7\037@9\037codex\037Task\037/home/auro/notes\037busy\037session-1
";

    let rows = tmux::parse_pane_rows(input).expect("escaped tmux output should parse");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].pane_id, "%41");
    assert_eq!(rows[0].session_id.as_deref(), Some("$7"));
    assert_eq!(rows[0].agent_provider.as_deref(), Some("codex"));
    assert_eq!(rows[0].agent_state.as_deref(), Some("busy"));
}

#[test]
fn tmux_output_does_not_split_on_printable_field_content() {
    let input = r"notes\0374\0371\037%41\037324026\037claude\037Task ||AGENTSCAN|| Review\037/dev/pts/44\037/home/auro/notes\037query\037$7\037@9\037codex\037Task ||AGENTSCAN|| Review\037/home/auro/notes\037busy\037session-1
";

    let rows = tmux::parse_pane_rows(input).expect("tmux output with printable token should parse");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].pane_id, "%41");
    assert_eq!(rows[0].pane_title_raw, "Task ||AGENTSCAN|| Review");
    assert_eq!(rows[0].agent_provider.as_deref(), Some("codex"));
}

#[test]
fn parses_tmux_client_rows_and_selects_most_recent_tty() {
    let input = concat!(
        "/dev/pts/5\x1f1711671000\n",
        "/dev/pts/7\x1f1711672000\n",
        "\x1f1711673000\n"
    );

    let clients = tmux::parse_tmux_client_rows(input).expect("tmux client output should parse");
    assert_eq!(clients.len(), 2);
    assert_eq!(clients[0].client_tty, "/dev/pts/5");
    assert_eq!(
        tmux::select_best_client_tty(&clients),
        Some("/dev/pts/7".to_string())
    );
}

#[test]
fn parses_tmux_client_rows_with_escaped_delimiters() {
    let input = "/dev/pts/5\\0371711671000\n/dev/pts/7\\0371711672000\n";

    let clients =
        tmux::parse_tmux_client_rows(input).expect("escaped tmux client output should parse");

    assert_eq!(clients.len(), 2);
    assert_eq!(clients[0].client_tty, "/dev/pts/5");
    assert_eq!(
        tmux::select_best_client_tty(&clients),
        Some("/dev/pts/7".to_string())
    );
}

#[test]
fn pane_record_uses_canonical_shape() {
    let pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "notes".to_string(),
        window_index: 4,
        pane_index: 1,
        pane_id: "%41".to_string(),
        pane_pid: 324026,
        pane_current_command: "claude".to_string(),
        pane_title_raw: "Claude Code | Query".to_string(),
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

    assert_eq!(pane.provider, Some(Provider::Claude));
    assert_eq!(pane.location.session_name, "notes");
    assert_eq!(pane.display.label, "Query");
    assert_eq!(pane.display.activity_label.as_deref(), Some("Query"));
}

#[test]
fn list_json_exposes_the_machine_readable_pane_fields() {
    let pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "notes".to_string(),
        window_index: 4,
        pane_index: 1,
        pane_id: "%41".to_string(),
        pane_pid: 324026,
        pane_current_command: "claude".to_string(),
        pane_title_raw: "Claude Code | Query".to_string(),
        pane_tty: "/dev/pts/44".to_string(),
        pane_current_path: "/home/auro/notes".to_string(),
        window_name: "ai".to_string(),
        session_id: Some("$7".to_string()),
        window_id: Some("@9".to_string()),
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
    let status_kind = pane.status.kind;
    let snapshot = SnapshotEnvelope {
        schema_version: CACHE_SCHEMA_VERSION,
        generated_at: "2026-04-15T00:00:00Z".to_string(),
        source: super::SnapshotSource {
            kind: SourceKind::Snapshot,
            tmux_version: Some("3.4".to_string()),
            daemon_generated_at: None,
        },
        panes: vec![pane],
    };

    let json = serde_json::to_value(&snapshot).expect("snapshot should serialize");
    let pane = &json["panes"][0];

    assert_eq!(pane["pane_id"], "%41");
    assert_eq!(pane["provider"], "claude");
    assert_eq!(
        pane["status"]["kind"],
        serde_json::to_value(status_kind).expect("status kind should serialize")
    );
    assert_eq!(pane["location"]["session_name"], "notes");
    assert_eq!(pane["location"]["window_index"], 4);
    assert_eq!(pane["location"]["pane_index"], 1);
    assert_eq!(pane["display"]["label"], "Query");
}

#[test]
fn daemon_notifications_trigger_refresh() {
    assert!(daemon::should_resnapshot_from_notification(
        "%window-add @1"
    ));
    assert!(daemon::should_resnapshot_from_notification(
        "%unlinked-window-close @1"
    ));
    assert!(!daemon::should_resnapshot_from_notification(
        "%subscription-changed agentscan $174 @251 1 %251 : %251:Claude Code | Working:claude::::"
    ));
    assert!(!daemon::should_resnapshot_from_notification("%begin 1 1 0"));
}

#[test]
fn window_notifications_expose_window_targets() {
    assert_eq!(
        daemon::window_notification_target("%window-renamed @1 editor"),
        Some("@1")
    );
    assert_eq!(
        daemon::window_notification_target("%window-close @2"),
        Some("@2")
    );
    assert_eq!(
        daemon::window_notification_target("%unlinked-window-renamed @4 sh"),
        Some("@4")
    );
    assert_eq!(
        daemon::window_notification_target("%layout-change @3 a,b,c"),
        Some("@3")
    );
    assert_eq!(
        daemon::window_notification_target("%session-renamed $1 renamed"),
        None
    );
}

#[test]
fn session_notifications_expose_session_targets() {
    assert_eq!(
        daemon::session_notification_target("%session-renamed $1 renamed"),
        Some("$1")
    );
    assert_eq!(
        daemon::session_notification_target("%window-renamed @1 editor"),
        None
    );
}

#[test]
fn subscription_changed_notifications_expose_pane_id() {
    assert_eq!(
        daemon::subscription_changed_pane_id(
            "%subscription-changed agentscan $174 @251 1 %251 : %251:Claude Code | Working:claude::::"
        ),
        Some("%251")
    );
    assert_eq!(daemon::subscription_changed_pane_id("%window-add @1"), None);
}

#[test]
fn output_notifications_expose_title_change_pane_id() {
    assert_eq!(
        daemon::output_title_change_pane_id(
            "%output %0 printf '\\033]2;Claude Code | Working\\033\\\\'\r\n"
        ),
        Some("%0")
    );
    assert_eq!(
        daemon::output_title_change_pane_id("%output %0 plain shell output"),
        None
    );
}

#[test]
fn control_mode_reader_tolerates_non_utf8_pane_output() {
    let mut input = std::io::Cursor::new(b"%output %0 \xff\xfe plain bytes\r\n%exit\n");

    let first = daemon::read_control_mode_line(&mut input)
        .expect("line read should succeed")
        .expect("first line should exist");
    assert_eq!(daemon::output_title_change_pane_id(&first), None);
    assert!(first.starts_with("%output %0 "));
    assert!(first.contains("plain bytes"));

    let second = daemon::read_control_mode_line(&mut input)
        .expect("line read should succeed")
        .expect("second line should exist");
    assert_eq!(second, "%exit");

    assert!(
        daemon::read_control_mode_line(&mut input)
            .expect("eof read should succeed")
            .is_none()
    );
}

#[test]
fn daemon_subscription_format_includes_wrapper_metadata_fields() {
    assert!(DAEMON_SUBSCRIPTION_FORMAT.contains("#{{pane_current_command}}"));
    assert!(DAEMON_SUBSCRIPTION_FORMAT.contains("#{{pane_title}}"));
    assert!(DAEMON_SUBSCRIPTION_FORMAT.contains("#{{@agent.provider}}"));
    assert!(DAEMON_SUBSCRIPTION_FORMAT.contains("#{{@agent.state}}"));
    assert!(DAEMON_SUBSCRIPTION_FORMAT.contains("#{{@agent.session_id}}"));
}

#[test]
fn detects_notification_names() {
    assert_eq!(
        daemon::notification_name("%window-renamed @1 editor"),
        Some("%window-renamed")
    );
    assert_eq!(daemon::notification_name("plain output"), None);
}

#[test]
fn gemini_status_uses_generic_titles_when_provider_is_known() {
    let busy = classify::infer_title_status(
        Some(Provider::Gemini),
        Some(super::ClassificationMatchKind::PaneTitle),
        "Working",
    );
    let idle = classify::infer_title_status(
        Some(Provider::Gemini),
        Some(super::ClassificationMatchKind::PaneTitle),
        "Ready",
    );
    let dynamic_idle = classify::infer_title_status(
        Some(Provider::Gemini),
        Some(super::ClassificationMatchKind::PaneTitle),
        "◇  Ready (workspace)",
    );
    let dynamic_busy = classify::infer_title_status(
        Some(Provider::Gemini),
        Some(super::ClassificationMatchKind::PaneTitle),
        "✦  Working… (workspace)",
    );
    let thought_busy = classify::infer_title_status(
        Some(Provider::Gemini),
        Some(super::ClassificationMatchKind::PaneTitle),
        "✦  Processing request (workspace)",
    );
    let action_required = classify::infer_title_status(
        Some(Provider::Gemini),
        Some(super::ClassificationMatchKind::PaneTitle),
        "✋  Action Required (workspace)",
    );
    let silent_working = classify::infer_title_status(
        Some(Provider::Gemini),
        Some(super::ClassificationMatchKind::PaneTitle),
        "⏲  Working… (workspace)",
    );
    let unknown = classify::infer_title_status(
        Some(Provider::Gemini),
        Some(super::ClassificationMatchKind::PaneTitle),
        "Plan snapshot cache migration",
    );
    let not_ready = classify::infer_title_status(
        Some(Provider::Gemini),
        Some(super::ClassificationMatchKind::PaneTitle),
        "Not Ready for deploy",
    );

    assert_eq!(busy.kind, StatusKind::Busy);
    assert_eq!(idle.kind, StatusKind::Idle);
    assert_eq!(dynamic_idle.kind, StatusKind::Idle);
    assert_eq!(dynamic_busy.kind, StatusKind::Busy);
    assert_eq!(thought_busy.kind, StatusKind::Busy);
    assert_eq!(action_required.kind, StatusKind::Busy);
    assert_eq!(silent_working.kind, StatusKind::Busy);
    assert_eq!(unknown.kind, StatusKind::Unknown);
    assert_eq!(not_ready.kind, StatusKind::Unknown);
}

#[test]
fn codex_status_uses_title_only() {
    let busy = classify::infer_title_status(
        Some(Provider::Codex),
        Some(super::ClassificationMatchKind::PaneTitle),
        "⠹ agentscan | Working",
    );
    let default_spinner = classify::infer_title_status(
        Some(Provider::Codex),
        Some(super::ClassificationMatchKind::PaneTitle),
        "⠹ agentscan",
    );
    let waiting = classify::infer_title_status(
        Some(Provider::Codex),
        Some(super::ClassificationMatchKind::PaneTitle),
        "Waiting",
    );
    let status_first_busy = classify::infer_title_status(
        Some(Provider::Codex),
        Some(super::ClassificationMatchKind::PaneTitle),
        "Thinking | Review code quality in repository",
    );
    let status_first_idle = classify::infer_title_status(
        Some(Provider::Codex),
        Some(super::ClassificationMatchKind::PaneTitle),
        "Ready | Review code quality in repository",
    );
    let status_last_wins = classify::infer_title_status(
        Some(Provider::Codex),
        Some(super::ClassificationMatchKind::PaneTitle),
        "Ready | Working",
    );
    let status_last_with_codex_activity = classify::infer_title_status(
        Some(Provider::Codex),
        Some(super::ClassificationMatchKind::PaneTitle),
        "review codex login | Working",
    );
    let lgpt_status_last = classify::infer_title_status(
        Some(Provider::Codex),
        Some(super::ClassificationMatchKind::PaneTitle),
        "repo: /path/lgpt.sh | Working",
    );
    let wrapped_status_last_wins = classify::infer_title_status(
        Some(Provider::Codex),
        Some(super::ClassificationMatchKind::PaneTitle),
        "Ready | Working: codex",
    );
    let idle = classify::infer_title_status(
        Some(Provider::Codex),
        Some(super::ClassificationMatchKind::PaneTitle),
        "Ready",
    );

    assert_eq!(busy.kind, StatusKind::Busy);
    assert_eq!(default_spinner.kind, StatusKind::Busy);
    assert_eq!(waiting.kind, StatusKind::Busy);
    assert_eq!(status_first_busy.kind, StatusKind::Busy);
    assert_eq!(status_first_idle.kind, StatusKind::Idle);
    assert_eq!(status_last_wins.kind, StatusKind::Busy);
    assert_eq!(status_last_with_codex_activity.kind, StatusKind::Busy);
    assert_eq!(lgpt_status_last.kind, StatusKind::Busy);
    assert_eq!(wrapped_status_last_wins.kind, StatusKind::Busy);
    assert_eq!(idle.kind, StatusKind::Idle);
}

#[test]
fn claude_status_distinguishes_spinner_and_idle_marker() {
    let busy = classify::infer_title_status(
        Some(Provider::Claude),
        Some(super::ClassificationMatchKind::PaneTitle),
        "⠏ Building summary",
    );
    let idle = classify::infer_title_status(
        Some(Provider::Claude),
        Some(super::ClassificationMatchKind::PaneTitle),
        "✳ Review and summarize todo list",
    );

    assert_eq!(busy.kind, StatusKind::Busy);
    assert_eq!(idle.kind, StatusKind::Idle);
}

#[test]
fn claude_status_uses_textual_titles_without_spinner_glyphs() {
    let busy = classify::infer_title_status(
        Some(Provider::Claude),
        Some(super::ClassificationMatchKind::PaneTitle),
        "Claude Code | Working",
    );
    let idle = classify::infer_title_status(
        Some(Provider::Claude),
        Some(super::ClassificationMatchKind::PaneTitle),
        "Claude Code | Ready",
    );
    let unknown = classify::infer_title_status(
        Some(Provider::Claude),
        Some(super::ClassificationMatchKind::PaneTitle),
        "Claude Code | Query",
    );

    assert_eq!(busy.kind, StatusKind::Busy);
    assert_eq!(idle.kind, StatusKind::Idle);
    assert_eq!(unknown.kind, StatusKind::Unknown);
}

#[test]
fn opencode_status_does_not_infer_state_from_session_title() {
    let busy = classify::infer_title_status(
        Some(Provider::Opencode),
        Some(super::ClassificationMatchKind::PaneTitle),
        "OC | Working",
    );
    let idle = classify::infer_title_status(
        Some(Provider::Opencode),
        Some(super::ClassificationMatchKind::PaneTitle),
        "OC | Ready",
    );
    let unknown = classify::infer_title_status(
        Some(Provider::Opencode),
        Some(super::ClassificationMatchKind::PaneTitle),
        "OC | Query planner",
    );

    assert_eq!(busy.kind, StatusKind::Unknown);
    assert_eq!(idle.kind, StatusKind::Unknown);
    assert_eq!(unknown.kind, StatusKind::Unknown);
}

#[test]
fn copilot_and_cursor_cli_status_use_title_prefixes_when_present() {
    let copilot_busy = classify::infer_title_status(
        Some(Provider::Copilot),
        Some(super::ClassificationMatchKind::PaneTitle),
        "Copilot | Working",
    );
    let copilot_idle = classify::infer_title_status(
        Some(Provider::Copilot),
        Some(super::ClassificationMatchKind::PaneTitle),
        "GitHub Copilot | Ready",
    );
    let cursor_busy = classify::infer_title_status(
        Some(Provider::CursorCli),
        Some(super::ClassificationMatchKind::PaneTitle),
        "Cursor CLI | Working",
    );
    let cursor_unknown = classify::infer_title_status(
        Some(Provider::CursorCli),
        Some(super::ClassificationMatchKind::PaneTitle),
        "Cursor | Query planner",
    );
    let cursor_agent_busy = classify::infer_title_status(
        Some(Provider::CursorCli),
        Some(super::ClassificationMatchKind::PaneTitle),
        "Cursor Agent | Working",
    );

    assert_eq!(copilot_busy.kind, StatusKind::Busy);
    assert_eq!(copilot_idle.kind, StatusKind::Idle);
    assert_eq!(cursor_busy.kind, StatusKind::Busy);
    assert_eq!(cursor_unknown.kind, StatusKind::Unknown);
    assert_eq!(cursor_agent_busy.kind, StatusKind::Busy);
}

#[test]
fn command_first_status_ignores_stale_prefixed_titles_from_other_providers() {
    let stale_claude = classify::infer_title_status(
        Some(Provider::Codex),
        Some(super::ClassificationMatchKind::PaneCurrentCommand),
        "Claude Code | Ready",
    );
    let stale_opencode = classify::infer_title_status(
        Some(Provider::Codex),
        Some(super::ClassificationMatchKind::PaneCurrentCommand),
        "OC | Working",
    );
    let stale_copilot = classify::infer_title_status(
        Some(Provider::Codex),
        Some(super::ClassificationMatchKind::PaneCurrentCommand),
        "Copilot | Working",
    );
    let stale_pi = classify::infer_title_status(
        Some(Provider::Claude),
        Some(super::ClassificationMatchKind::PaneMetadata),
        "⠋ π - refactor - agentscan",
    );

    assert_eq!(stale_claude.kind, StatusKind::Unknown);
    assert_eq!(stale_opencode.kind, StatusKind::Unknown);
    assert_eq!(stale_copilot.kind, StatusKind::Unknown);
    assert_eq!(stale_pi.kind, StatusKind::Unknown);
}

#[test]
fn cursor_cli_generic_titles_fall_back_to_window_name_for_display() {
    let pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "cursorprobe".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%305".to_string(),
        pane_pid: 1060198,
        pane_current_command: "cursor-agent".to_string(),
        pane_title_raw: "bront".to_string(),
        pane_tty: "/dev/pts/99".to_string(),
        pane_current_path: "/home/auro/code/agentscan".to_string(),
        window_name: "ai".to_string(),
        session_id: Some("$0".to_string()),
        window_id: Some("@0".to_string()),
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });

    assert_eq!(pane.provider, Some(Provider::CursorCli));
    assert_eq!(pane.display.label, "ai");
    assert_eq!(pane.display.activity_label, None);
    assert_eq!(pane.status.kind, StatusKind::Unknown);
    assert_eq!(
        pane.classification.matched_by,
        Some(super::ClassificationMatchKind::PaneCurrentCommand)
    );
}

#[test]
fn cursor_cli_title_only_panes_classify_from_bare_cursor_titles() {
    for title in ["Cursor Agent", "Cursor CLI", "Cursor"] {
        let pane = classify::pane_from_row(super::TmuxPaneRow {
            session_name: "cursorprobe".to_string(),
            window_index: 1,
            pane_index: 1,
            pane_id: "%306".to_string(),
            pane_pid: 1060201,
            pane_current_command: "zsh".to_string(),
            pane_title_raw: title.to_string(),
            pane_tty: "/dev/pts/100".to_string(),
            pane_current_path: "/home/auro/code/agentscan".to_string(),
            window_name: "ai".to_string(),
            session_id: Some("$0".to_string()),
            window_id: Some("@0".to_string()),
            agent_provider: None,
            agent_label: None,
            agent_cwd: None,
            agent_state: None,
            agent_session_id: None,
        });

        assert_eq!(pane.provider, Some(Provider::CursorCli), "title: {title}");
        assert_eq!(pane.display.label, title, "title: {title}");
        assert_eq!(pane.display.activity_label, None, "title: {title}");
        assert_eq!(pane.status.kind, StatusKind::Unknown, "title: {title}");
        assert_eq!(
            pane.classification.matched_by,
            Some(super::ClassificationMatchKind::PaneTitle),
            "title: {title}"
        );
    }
}

#[test]
fn cursor_cli_generic_status_titles_fall_back_for_display() {
    let ready_pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "cursorprobe".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%307".to_string(),
        pane_pid: 1060202,
        pane_current_command: "cursor-agent".to_string(),
        pane_title_raw: "Cursor | Ready".to_string(),
        pane_tty: "/dev/pts/101".to_string(),
        pane_current_path: "/home/auro/code/agentscan".to_string(),
        window_name: "cursor-window".to_string(),
        session_id: Some("$0".to_string()),
        window_id: Some("@0".to_string()),
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
    assert_eq!(ready_pane.provider, Some(Provider::CursorCli));
    assert_eq!(ready_pane.display.label, "cursor-window");
    assert_eq!(ready_pane.display.activity_label, None);
    assert_eq!(ready_pane.status.kind, StatusKind::Idle);

    let working_pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "cursorprobe".to_string(),
        window_index: 1,
        pane_index: 2,
        pane_id: "%308".to_string(),
        pane_pid: 1060203,
        pane_current_command: "cursor-agent".to_string(),
        pane_title_raw: "Cursor CLI | Working".to_string(),
        pane_tty: "/dev/pts/102".to_string(),
        pane_current_path: "/home/auro/code/agentscan".to_string(),
        window_name: "cursor-window-2".to_string(),
        session_id: Some("$0".to_string()),
        window_id: Some("@0".to_string()),
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
    assert_eq!(working_pane.provider, Some(Provider::CursorCli));
    assert_eq!(working_pane.display.label, "cursor-window-2");
    assert_eq!(working_pane.display.activity_label, None);
    assert_eq!(working_pane.status.kind, StatusKind::Busy);
}

#[test]
fn cursor_cli_task_titles_still_drive_display_label() {
    let pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "cursorprobe".to_string(),
        window_index: 1,
        pane_index: 3,
        pane_id: "%309".to_string(),
        pane_pid: 1060204,
        pane_current_command: "cursor-agent".to_string(),
        pane_title_raw: "Cursor CLI | Query planner".to_string(),
        pane_tty: "/dev/pts/103".to_string(),
        pane_current_path: "/home/auro/code/agentscan".to_string(),
        window_name: "cursor-window-3".to_string(),
        session_id: Some("$0".to_string()),
        window_id: Some("@0".to_string()),
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });

    assert_eq!(pane.provider, Some(Provider::CursorCli));
    assert_eq!(pane.display.label, "Query planner");
    assert_eq!(
        pane.display.activity_label,
        Some("Query planner".to_string())
    );
    assert_eq!(pane.status.kind, StatusKind::Unknown);
}

#[test]
fn cursor_cli_metadata_alias_classifies_generic_shell_panes() {
    let pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "cursorprobe".to_string(),
        window_index: 1,
        pane_index: 4,
        pane_id: "%310".to_string(),
        pane_pid: 1060205,
        pane_current_command: "zsh".to_string(),
        pane_title_raw: "zsh".to_string(),
        pane_tty: "/dev/pts/104".to_string(),
        pane_current_path: "/home/auro/code/agentscan".to_string(),
        window_name: "cursor-window-4".to_string(),
        session_id: Some("$0".to_string()),
        window_id: Some("@0".to_string()),
        agent_provider: Some("cursor-agent".to_string()),
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });

    assert_eq!(pane.provider, Some(Provider::CursorCli));
    assert_eq!(pane.display.label, "cursor-window-4");
    assert_eq!(pane.display.activity_label, None);
    assert_eq!(pane.status.kind, StatusKind::Unknown);
    assert_eq!(
        pane.classification.matched_by,
        Some(super::ClassificationMatchKind::PaneMetadata)
    );
}

#[test]
fn cursor_agent_prefixed_task_titles_still_drive_display_label() {
    let pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "cursorprobe".to_string(),
        window_index: 1,
        pane_index: 5,
        pane_id: "%311".to_string(),
        pane_pid: 1060206,
        pane_current_command: "cursor-agent".to_string(),
        pane_title_raw: "Cursor Agent | Query planner".to_string(),
        pane_tty: "/dev/pts/105".to_string(),
        pane_current_path: "/home/auro/code/agentscan".to_string(),
        window_name: "cursor-window-5".to_string(),
        session_id: Some("$0".to_string()),
        window_id: Some("@0".to_string()),
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });

    assert_eq!(pane.provider, Some(Provider::CursorCli));
    assert_eq!(pane.display.label, "Query planner");
    assert_eq!(
        pane.display.activity_label,
        Some("Query planner".to_string())
    );
    assert_eq!(pane.status.kind, StatusKind::Unknown);
}

#[test]
fn pi_status_uses_spinner_title_when_present() {
    let busy = classify::infer_title_status(
        Some(Provider::Pi),
        Some(super::ClassificationMatchKind::PaneTitle),
        "⠋ π - refactor - agentscan",
    );
    let unknown = classify::infer_title_status(
        Some(Provider::Pi),
        Some(super::ClassificationMatchKind::PaneTitle),
        "π - refactor - agentscan",
    );

    assert_eq!(busy.kind, StatusKind::Busy);
    assert_eq!(unknown.kind, StatusKind::Unknown);
}

#[test]
fn pi_default_titles_do_not_invent_activity_labels() {
    let default_title = classify::display_metadata(
        Some(Provider::Pi),
        Some(super::ClassificationMatchKind::PaneTitle),
        None,
        "π - agentscan",
        "zsh",
        "ai",
    );
    let session_title = classify::display_metadata(
        Some(Provider::Pi),
        Some(super::ClassificationMatchKind::PaneTitle),
        None,
        "π - refactor - agentscan",
        "zsh",
        "ai",
    );
    let spinner_title = classify::display_metadata(
        Some(Provider::Pi),
        Some(super::ClassificationMatchKind::PaneTitle),
        None,
        "⠋ π - refactor - agentscan",
        "zsh",
        "ai",
    );

    assert_eq!(default_title.label, "agentscan");
    assert_eq!(default_title.activity_label, None);
    assert_eq!(session_title.label, "refactor - agentscan");
    assert_eq!(session_title.activity_label, None);
    assert_eq!(spinner_title.label, "refactor - agentscan");
    assert_eq!(spinner_title.activity_label, None);
}

#[test]
fn metadata_state_fills_unknown_status_without_overriding_title_signal() {
    let unknown_from_title = classify::infer_title_status(
        Some(Provider::Codex),
        Some(super::ClassificationMatchKind::PaneTitle),
        "(bront) repo: codex",
    );
    let busy_from_metadata = classify::infer_status(unknown_from_title, Some("busy"));
    assert_eq!(busy_from_metadata.kind, StatusKind::Busy);

    let idle_from_title = classify::infer_title_status(
        Some(Provider::Codex),
        Some(super::ClassificationMatchKind::PaneTitle),
        "Ready",
    );
    let still_idle = classify::infer_status(idle_from_title, Some("busy"));
    assert_eq!(still_idle.kind, StatusKind::Idle);
}

#[test]
fn tmux_metadata_updates_emit_expected_option_values() {
    let args = super::TmuxSetMetadataArgs {
        pane_id: Some("%41".to_string()),
        provider: Some(Provider::Claude),
        label: Some("Review notes".to_string()),
        cwd: Some("/tmp/notes".to_string()),
        state: Some(StatusKind::Busy),
        session_id: Some("sess-123".to_string()),
    };

    let updates = tmux::tmux_metadata_updates(&args);
    assert_eq!(
        updates,
        vec![
            ("@agent.provider", "claude".to_string()),
            ("@agent.label", "Review notes".to_string()),
            ("@agent.cwd", "/tmp/notes".to_string()),
            ("@agent.state", "busy".to_string()),
            ("@agent.session_id", "sess-123".to_string()),
        ]
    );
}

#[test]
fn tmux_metadata_fields_to_clear_defaults_to_all_fields() {
    assert_eq!(
        tmux::tmux_metadata_fields_to_clear(&[]),
        vec![
            "@agent.provider",
            "@agent.label",
            "@agent.cwd",
            "@agent.state",
            "@agent.session_id",
        ]
    );
}

#[test]
fn tmux_metadata_fields_to_clear_maps_selected_fields() {
    assert_eq!(
        tmux::tmux_metadata_fields_to_clear(&[
            TmuxMetadataField::Provider,
            TmuxMetadataField::State,
            TmuxMetadataField::SessionId,
        ]),
        vec!["@agent.provider", "@agent.state", "@agent.session_id"]
    );
}

#[test]
fn detects_codex_titles() {
    assert!(classify::looks_like_codex_title("(repo) task: codex"));
    assert!(classify::looks_like_codex_title(
        "(repo) task: /home/auro/.zshrc.d/scripts/lgpt.sh"
    ));
    assert!(!classify::looks_like_codex_title("(repo) task: shell"));
}

#[test]
fn detects_pi_titles() {
    let greek = classify::classify_provider(None, "zsh", "π - refactor - agentscan")
        .expect("greek pi title should match");
    let default_greek = classify::classify_provider(None, "zsh", "π - agentscan")
        .expect("default greek pi title should match");
    let ascii = classify::classify_provider(None, "pi", "pi - refactor - agentscan")
        .expect("ascii pi title should match with bare pi command");

    assert_eq!(greek.provider, Provider::Pi);
    assert_eq!(default_greek.provider, Provider::Pi);
    assert_eq!(ascii.provider, Provider::Pi);
}

#[test]
fn does_not_classify_ascii_pi_task_titles_without_extra_signal() {
    let ascii = classify::classify_provider(None, "zsh", "pi - refactor - agentscan");

    assert!(
        ascii.is_none(),
        "ascii pi title without metadata, spinner, or pi command should not match"
    );
}

#[test]
fn does_not_classify_ascii_or_empty_pi_prefix_titles() {
    let ascii = classify::classify_provider(None, "zsh", "pi - agentscan");
    let empty_greek = classify::classify_provider(None, "zsh", "π - ");

    assert!(ascii.is_none(), "generic ascii pi title should not match");
    assert!(
        empty_greek.is_none(),
        "empty greek pi title should not match"
    );
}

#[test]
fn cache_path_uses_override_when_present() {
    let actual = cache_path_for_test(Some("/tmp/agentscan-cache.json"), None, None)
        .expect("override path should work");
    assert_eq!(actual, PathBuf::from("/tmp/agentscan-cache.json"));
}

#[test]
fn cache_path_defaults_to_xdg_location() {
    let actual = cache_path_for_test(None, Some("/tmp/cache"), Some("/tmp/home"))
        .expect("xdg cache path should work");
    assert_eq!(
        actual,
        PathBuf::from("/tmp/cache").join(CACHE_RELATIVE_PATH)
    );
}

#[test]
fn source_kind_supports_daemon() {
    assert_eq!(
        serde_json::to_string(&SourceKind::Daemon).unwrap(),
        "\"daemon\""
    );
}

#[test]
fn daemon_cache_status_reports_health_states() {
    let mut snapshot: SnapshotEnvelope =
        serde_json::from_str(CACHE_SNAPSHOT_FIXTURE).expect("cache fixture should parse");

    assert_eq!(
        cache::daemon_cache_status(SourceKind::Daemon, Some(10), Some(60)),
        super::DaemonCacheStatus::Healthy
    );
    assert_eq!(
        super::DaemonCacheStatus::Healthy.as_str(),
        "healthy"
    );
    assert_eq!(
        super::DaemonCacheStatus::SnapshotOnly.as_str(),
        "snapshot_only"
    );

    snapshot.source.kind = SourceKind::Snapshot;
    snapshot.source.daemon_generated_at = None;
    assert_eq!(
        cache::daemon_cache_status(snapshot.source.kind, None, Some(60)),
        super::DaemonCacheStatus::SnapshotOnly
    );

    snapshot.source.daemon_generated_at = Some("2026-03-28T00:00:00Z".to_string());
    assert_eq!(
        cache::daemon_cache_status(snapshot.source.kind, Some(120), Some(60)),
        super::DaemonCacheStatus::Stale
    );
}

#[test]
fn daemon_cache_status_uses_last_daemon_refresh_even_after_snapshot_rewrite() {
    let mut snapshot: SnapshotEnvelope =
        serde_json::from_str(CACHE_SNAPSHOT_FIXTURE).expect("cache fixture should parse");

    snapshot.source.kind = SourceKind::Snapshot;
    snapshot.generated_at = "2026-03-29T00:00:00Z".to_string();
    snapshot.source.daemon_generated_at = Some("2026-03-28T00:00:00Z".to_string());

    assert_eq!(
        cache::daemon_cache_status(snapshot.source.kind, Some(10), Some(60)),
        super::DaemonCacheStatus::Healthy
    );
}

#[test]
fn cache_diagnostics_distinguish_daemon_and_snapshot_provenance() {
    let mut daemon_snapshot: SnapshotEnvelope =
        serde_json::from_str(CACHE_SNAPSHOT_FIXTURE).expect("cache fixture should parse");
    daemon_snapshot.generated_at = "2026-03-29T00:00:00Z".to_string();
    daemon_snapshot.source.daemon_generated_at = Some("2026-03-28T00:00:00Z".to_string());

    let daemon_diagnostics =
        cache::cache_diagnostics(&daemon_snapshot, Some(60)).expect("daemon diagnostics");
    assert_eq!(
        daemon_diagnostics.daemon_cache_status,
        super::DaemonCacheStatus::Stale
    );
    assert!(
        daemon_diagnostics
            .daemon_status_reason
            .contains("last daemon refresh is older than the 60s threshold"),
        "unexpected reason: {}",
        daemon_diagnostics.daemon_status_reason
    );

    daemon_snapshot.source.kind = SourceKind::Snapshot;
    let refreshed_diagnostics =
        cache::cache_diagnostics(&daemon_snapshot, Some(60)).expect("snapshot diagnostics");
    assert_eq!(
        refreshed_diagnostics.daemon_cache_status,
        super::DaemonCacheStatus::Stale
    );
    assert!(
        refreshed_diagnostics
            .daemon_status_reason
            .contains("cache was last refreshed directly from tmux"),
        "unexpected reason: {}",
        refreshed_diagnostics.daemon_status_reason
    );

    daemon_snapshot.source.daemon_generated_at = None;
    let snapshot_only_diagnostics =
        cache::cache_diagnostics(&daemon_snapshot, Some(60)).expect("snapshot-only diagnostics");
    assert_eq!(
        snapshot_only_diagnostics.daemon_cache_status,
        super::DaemonCacheStatus::SnapshotOnly
    );
    assert!(
        snapshot_only_diagnostics
            .daemon_status_reason
            .contains("does not include a daemon refresh timestamp"),
        "unexpected reason: {}",
        snapshot_only_diagnostics.daemon_status_reason
    );
}

#[test]
fn cache_diagnostics_treat_invalid_daemon_timestamp_as_unavailable() {
    let mut snapshot: SnapshotEnvelope =
        serde_json::from_str(CACHE_SNAPSHOT_FIXTURE).expect("cache fixture should parse");
    snapshot.source.daemon_generated_at = Some("not-a-timestamp".to_string());

    let diagnostics = cache::cache_diagnostics(&snapshot, Some(60))
        .expect("invalid daemon timestamp should degrade to unavailable diagnostics");
    assert_eq!(
        diagnostics.daemon_cache_status,
        super::DaemonCacheStatus::Unavailable
    );
    assert_eq!(diagnostics.daemon_age_seconds, None);
    assert!(
        diagnostics
            .daemon_status_reason
            .contains("does not include a usable daemon refresh timestamp"),
        "unexpected reason: {}",
        diagnostics.daemon_status_reason
    );
}
proptest! {
    #[test]
    fn parse_pane_rows_roundtrips_generated_rows(
        session_name in safe_tmux_field(),
        window_index in 0_u32..1000,
        pane_index in 0_u32..1000,
        pane_pid in 1_u32..u32::MAX,
        pane_current_command in safe_tmux_field(),
        pane_title_raw in safe_tmux_field(),
        pane_tty in safe_tmux_field(),
        pane_current_path in safe_tmux_field(),
        window_name in safe_tmux_field(),
    ) {
        let pane_id = format!("%{pane_pid}");
        let line = format!(
            "{session_name}\u{1f}{window_index}\u{1f}{pane_index}\u{1f}{pane_id}\u{1f}{pane_pid}\u{1f}{pane_current_command}\u{1f}{pane_title_raw}\u{1f}{pane_tty}\u{1f}{pane_current_path}\u{1f}{window_name}"
        );

        let rows = tmux::parse_pane_rows(&line).expect("generated tmux row should parse");
        prop_assert_eq!(rows.len(), 1);

        let row = &rows[0];
        prop_assert_eq!(&row.session_name, &session_name);
        prop_assert_eq!(row.window_index, window_index);
        prop_assert_eq!(row.pane_index, pane_index);
        prop_assert_eq!(&row.pane_id, &pane_id);
        prop_assert_eq!(row.pane_pid, pane_pid);
        prop_assert_eq!(&row.pane_current_command, &pane_current_command);
        prop_assert_eq!(&row.pane_title_raw, &pane_title_raw);
        prop_assert_eq!(&row.pane_tty, &pane_tty);
        prop_assert_eq!(&row.pane_current_path, &pane_current_path);
        prop_assert_eq!(&row.window_name, &window_name);
    }

    #[test]
    fn known_status_glyphs_strip_to_trimmed_tail(
        glyph in known_status_glyph(),
        padding in 0_usize..4,
        tail in any::<String>(),
    ) {
        let input = format!("{glyph}{}{tail}", " ".repeat(padding));
        prop_assert_eq!(classify::strip_known_status_glyph(&input), tail.trim_start());
    }
}
