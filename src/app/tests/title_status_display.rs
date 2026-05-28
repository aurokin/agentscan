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
        pane_active: false,
        window_active: false,
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
fn cursor_cli_title_only_panes_stay_unknown_from_bare_cursor_titles() {
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
            pane_active: false,
            window_active: false,
        });

        assert_eq!(pane.provider, None, "title: {title}");
        assert_eq!(pane.display.label, "ai", "title: {title}");
        assert_eq!(pane.display.activity_label, None, "title: {title}");
        assert_eq!(pane.status.kind, StatusKind::Unknown, "title: {title}");
        assert_eq!(
            pane.classification.matched_by,
            None,
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
        pane_active: false,
        window_active: false,
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
        pane_active: false,
        window_active: false,
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
        pane_active: false,
        window_active: false,
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
        pane_active: false,
        window_active: false,
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
        pane_active: false,
        window_active: false,
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
fn detects_codex_titles() {
    assert!(classify::looks_like_codex_title("(repo) task: codex"));
    assert!(classify::looks_like_codex_title(
        "(repo) task: /home/auro/.zshrc.d/scripts/lgpt.sh"
    ));
    assert!(!classify::looks_like_codex_title("(repo) task: shell"));
}

#[test]
fn detects_pi_titles() {
    // A running pi reports its runtime (`node`/`pi`/`bun`) as the foreground command, so the
    // greek title classifies. Over a bare shell the title is stale residue (see
    // `pi_greek_title_over_shell_defers_to_process_evidence`).
    let greek = classify::classify_provider(None, "node", "π - refactor - agentscan")
        .expect("greek pi title should match a running pi");
    let default_greek = classify::classify_provider(None, "node", "π - agentscan")
        .expect("default greek pi title should match a running pi");
    let ascii = classify::classify_provider(None, "pi", "pi - refactor - agentscan")
        .expect("ascii pi title should match with bare pi command");

    assert_eq!(greek.provider, Provider::Pi);
    assert_eq!(default_greek.provider, Provider::Pi);
    assert_eq!(ascii.provider, Provider::Pi);
}

#[test]
fn pi_greek_title_over_shell_defers_to_process_evidence() {
    // pi paints `π - <cwd>` via an OSC escape; tmux keeps it after pi exits and the pane
    // returns to the shell prompt. A bare interactive shell foreground is positive evidence
    // pi is gone, so the stale title must not classify on its own — leaving the pane to
    // process evidence, which drops it when no pi process remains.
    assert!(
        classify::classify_provider(None, "zsh", "π - agentscan").is_none(),
        "stale greek pi title over a shell prompt should not classify"
    );
    assert!(
        classify::classify_provider(None, "-zsh", "π - refactor - agentscan").is_none(),
        "stale greek pi title over a login shell should not classify"
    );
    assert!(
        classify::classify_provider(None, "fish", "π - agentscan").is_none(),
        "stale greek pi title over fish should not classify"
    );
}

#[test]
fn detects_grok_provider_and_working_titles() {
    let exact_command =
        classify::classify_provider(None, "grok", "grok").expect("grok command should match");
    let versioned_command = classify::classify_provider(
        None,
        "grok-0.1.212-ma",
        "⠹ - Running: shell - agentscan - grok",
    )
    .expect("versioned grok command should match");
    let active_title_match =
        classify::classify_provider(None, "zsh", "⠹ - Running: shell - agentscan - grok")
            .expect("active grok title suffix should match");
    let inactive_title_match = classify::classify_provider(None, "zsh", "agentscan - grok");
    let home_title = classify::classify_provider(None, "zsh", "grok");
    let busy_status = classify::infer_title_status(
        Some(Provider::Grok),
        Some(super::ClassificationMatchKind::PaneTitle),
        "⠹ - Running: shell - agentscan - grok",
    );
    let idle_title_status = classify::infer_title_status(
        Some(Provider::Grok),
        Some(super::ClassificationMatchKind::PaneTitle),
        "agentscan - grok",
    );
    let home_title_status = classify::infer_title_status(
        Some(Provider::Grok),
        Some(super::ClassificationMatchKind::PaneTitle),
        "grok",
    );

    assert_eq!(exact_command.provider, Provider::Grok);
    assert_eq!(
        exact_command.confidence,
        super::ClassificationConfidence::High
    );
    assert_eq!(versioned_command.provider, Provider::Grok);
    assert_eq!(
        versioned_command.confidence,
        super::ClassificationConfidence::Medium
    );
    assert_eq!(active_title_match.provider, Provider::Grok);
    assert!(
        inactive_title_match.is_none(),
        "plain grok title suffix without command/process evidence should not match"
    );
    assert!(
        home_title.is_none(),
        "bare grok title without command/process evidence should not match"
    );
    assert_eq!(busy_status.kind, StatusKind::Busy);
    assert_eq!(busy_status.source, super::StatusSource::TmuxTitle);
    assert_eq!(idle_title_status.kind, StatusKind::Unknown);
    assert_eq!(home_title_status.kind, StatusKind::Unknown);
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
