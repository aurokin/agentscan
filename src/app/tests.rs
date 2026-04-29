use std::cell::RefCell;
use std::path::{Path, PathBuf};

use anyhow::Context;
use proptest::{prelude::*, string::string_regex};
use unicode_width::UnicodeWidthStr;

const TMUX_SNAPSHOT_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/tmux_snapshot_titles.txt"
));
const CACHE_SNAPSHOT_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/cache_snapshot_v1.json"
));
const TMUX_METADATA_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/tmux_snapshot_with_metadata.txt"
));
const TMUX_AMBIGUOUS_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/tmux_snapshot_ambiguous.txt"
));

#[allow(unused_imports)]
use super::{
    CACHE_RELATIVE_PATH, CACHE_SCHEMA_VERSION, CLAUDE_SPINNER_GLYPHS, Cli,
    DAEMON_SUBSCRIPTION_FORMAT, IDLE_GLYPHS, OutputFormat, PaneRecord, Provider, SnapshotEnvelope,
    SourceKind, StatusKind, TmuxMetadataField, cache, classify, daemon, output, proc, tmux,
};

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
        super::daemon_cache_status_name(super::DaemonCacheStatus::Healthy),
        "healthy"
    );
    assert_eq!(
        super::daemon_cache_status_name(super::DaemonCacheStatus::SnapshotOnly),
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

#[test]
fn fixture_snapshot_parses_expected_provider_cases() {
    let rows = tmux::parse_pane_rows(TMUX_SNAPSHOT_FIXTURE).expect("fixture snapshot should parse");
    let panes: Vec<_> = rows.into_iter().map(classify::pane_from_row).collect();

    assert_fixture_codex_cases(&panes);
    assert_fixture_claude_cases(&panes);
    assert_fixture_gemini_cases(&panes);
    assert_fixture_opencode_case(&panes);
    assert_fixture_copilot_case(&panes);
    assert_fixture_cursor_cli_title_case(&panes);
    assert_fixture_cursor_cli_command_case(&panes);
    assert_fixture_pi_case(&panes);
}

#[test]
fn fixture_snapshot_preserves_wrapper_prefixes() {
    let rows = tmux::parse_pane_rows(TMUX_SNAPSHOT_FIXTURE).expect("fixture snapshot should parse");
    let panes: Vec<_> = rows.into_iter().map(classify::pane_from_row).collect();

    let wrapped_codex = pane_by_id(&panes, "%89");
    assert_eq!(wrapped_codex.provider, Some(Provider::Codex));
    assert_eq!(wrapped_codex.display.label, "(bront) parallel-n64");
    assert_eq!(
        wrapped_codex.display.activity_label.as_deref(),
        Some("(bront) parallel-n64")
    );
    assert_eq!(wrapped_codex.status.kind, StatusKind::Unknown);
    assert_eq!(wrapped_codex.tmux.session_id.as_deref(), Some("$8"));
    assert_eq!(wrapped_codex.tmux.window_id.as_deref(), Some("@8"));
}

#[test]
fn ambiguous_fixture_documents_current_unresolved_behavior() {
    let rows =
        tmux::parse_pane_rows(TMUX_AMBIGUOUS_FIXTURE).expect("ambiguous fixture should parse");
    let panes: Vec<_> = rows.into_iter().map(classify::pane_from_row).collect();

    assert_eq!(panes.len(), 5);
    assert_unresolved_ambiguous_pane(pane_by_id(&panes, "%600"), "(bront) ~/code/agent-wrapper");
    assert_unresolved_ambiguous_pane(pane_by_id(&panes, "%601"), "Working");
    assert_unresolved_ambiguous_pane(pane_by_id(&panes, "%602"), "agent bootstrap");
    assert_unresolved_ambiguous_pane(pane_by_id(&panes, "%603"), "pi - agentscan");
    assert_unresolved_ambiguous_pane(pane_by_id(&panes, "%604"), "review_auth_flow");
}

#[test]
fn proc_fallback_resolves_only_targeted_ambiguous_candidates() {
    let rows =
        tmux::parse_pane_rows(TMUX_AMBIGUOUS_FIXTURE).expect("ambiguous fixture should parse");
    let inspector = FakeProcessInspector::new([
        (602001, vec!["codex".to_string()]),
        (602002, vec!["cursor-agent".to_string()]),
        (602003, vec!["claude".to_string()]),
    ]);
    let panes = classify::panes_from_rows_with_proc_fallback(rows, &inspector);

    assert_unresolved_ambiguous_pane(pane_by_id(&panes, "%600"), "(bront) ~/code/agent-wrapper");

    let node_launcher = pane_by_id(&panes, "%601");
    assert_eq!(node_launcher.provider, Some(Provider::Codex));
    assert_eq!(node_launcher.status.kind, StatusKind::Busy);
    assert_eq!(
        node_launcher.classification.matched_by,
        Some(super::ClassificationMatchKind::ProcProcessTree)
    );
    assert_eq!(
        node_launcher.classification.reasons,
        vec!["proc_descendant_command=codex"]
    );
    assert_eq!(
        node_launcher.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::Resolved
    );
    assert_eq!(
        node_launcher.diagnostics.proc_fallback.commands,
        vec!["codex".to_string()]
    );

    let python_launcher = pane_by_id(&panes, "%602");
    assert_eq!(python_launcher.provider, Some(Provider::CursorCli));
    assert_eq!(python_launcher.status.kind, StatusKind::Unknown);
    assert_eq!(
        python_launcher.classification.matched_by,
        Some(super::ClassificationMatchKind::ProcProcessTree)
    );
    assert_eq!(
        python_launcher.classification.reasons,
        vec!["proc_descendant_command=cursor-agent"]
    );
    assert_eq!(
        python_launcher.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::Resolved
    );

    assert_unresolved_ambiguous_pane(pane_by_id(&panes, "%603"), "pi - agentscan");
    assert_unresolved_ambiguous_pane(pane_by_id(&panes, "%604"), "review_auth_flow");
    assert_eq!(inspector.calls(), vec![602001, 602002]);
}

#[test]
fn proc_fallback_leaves_candidate_unknown_without_provider_evidence() {
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%700".to_string(),
        pane_pid: 700,
        pane_current_command: "node".to_string(),
        pane_title_raw: "Working".to_string(),
        pane_tty: "/dev/pts/700".to_string(),
        pane_current_path: "/tmp/node-wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
    let inspector =
        FakeProcessInspector::new([(700, vec!["node".to_string(), "helper".to_string()])]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_unresolved_ambiguous_pane(&pane, "Working");
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::NoMatch
    );
    assert_eq!(
        pane.diagnostics.proc_fallback.reason,
        "no known provider evidence found in descendants"
    );
    assert_eq!(
        pane.diagnostics.proc_fallback.commands,
        vec!["node".to_string(), "helper".to_string()]
    );
    assert_eq!(inspector.calls(), vec![700]);
}

#[test]
fn proc_fallback_resolves_provider_from_argv0_when_command_is_interpreter() {
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%703".to_string(),
        pane_pid: 703,
        pane_current_command: "node".to_string(),
        pane_title_raw: "Ready".to_string(),
        pane_tty: "/dev/pts/703".to_string(),
        pane_current_path: "/tmp/node-wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
    let inspector = FakeProcessInspector::with_processes([(
        703,
        vec![proc::ProcessEvidence {
            pid: 704,
            command: "node".to_string(),
            argv: vec!["codex".to_string(), "/tmp/wrapper.js".to_string()],
            env: Vec::new(),
        }],
    )]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, Some(Provider::Codex));
    assert_eq!(
        pane.classification.reasons,
        vec!["proc_descendant_command=codex"]
    );
}

#[test]
fn proc_fallback_resolves_claude_from_node_cli_path_and_title_status() {
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%704".to_string(),
        pane_pid: 704,
        pane_current_command: "node".to_string(),
        pane_title_raw: "✳ Refactor auth flow".to_string(),
        pane_tty: "/dev/pts/704".to_string(),
        pane_current_path: "/tmp/node-wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
    let inspector = FakeProcessInspector::with_processes([(
        704,
        vec![proc::ProcessEvidence {
            pid: 705,
            command: "node".to_string(),
            argv: vec![
                "node".to_string(),
                "/Users/auro/.claude/local/node_modules/@anthropic-ai/claude-code/cli.mjs"
                    .to_string(),
            ],
            env: Vec::new(),
        }],
    )]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, Some(Provider::Claude));
    assert_eq!(pane.status.kind, StatusKind::Idle);
    assert_eq!(pane.status.source, super::StatusSource::TmuxTitle);
    assert_eq!(pane.display.label, "Refactor auth flow");
    assert_eq!(
        pane.classification.reasons,
        vec![
            "proc_descendant_argv=/Users/auro/.claude/local/node_modules/@anthropic-ai/claude-code/cli.mjs"
                .to_string()
        ]
    );
}

#[test]
fn proc_fallback_resolves_gemini_from_node_cli_path() {
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%705".to_string(),
        pane_pid: 705,
        pane_current_command: "node".to_string(),
        pane_title_raw: "Review deployment plan".to_string(),
        pane_tty: "/dev/pts/705".to_string(),
        pane_current_path: "/tmp/node-wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
    let inspector = FakeProcessInspector::with_processes([(
        705,
        vec![proc::ProcessEvidence {
            pid: 706,
            command: "node".to_string(),
            argv: vec![
                "node".to_string(),
                "/opt/homebrew/lib/node_modules/@google/gemini-cli/dist/index.js".to_string(),
            ],
            env: Vec::new(),
        }],
    )]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, Some(Provider::Gemini));
    assert_eq!(pane.display.label, "Review deployment plan");
    assert_eq!(
        pane.classification.reasons,
        vec![
            "proc_descendant_argv=/opt/homebrew/lib/node_modules/@google/gemini-cli/dist/index.js"
                .to_string()
        ]
    );
}

#[test]
fn proc_fallback_resolves_gemini_from_node_bin_shim() {
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%706".to_string(),
        pane_pid: 706,
        pane_current_command: "node".to_string(),
        pane_title_raw: "Review deployment plan".to_string(),
        pane_tty: "/dev/pts/706".to_string(),
        pane_current_path: "/tmp/node-wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
    let inspector = FakeProcessInspector::with_processes([(
        706,
        vec![proc::ProcessEvidence {
            pid: 707,
            command: "node".to_string(),
            argv: vec!["node".to_string(), "/opt/homebrew/bin/gemini".to_string()],
            env: Vec::new(),
        }],
    )]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, Some(Provider::Gemini));
    assert_eq!(
        pane.classification.reasons,
        vec!["proc_descendant_argv=/opt/homebrew/bin/gemini"]
    );
}

#[test]
fn proc_fallback_does_not_treat_arbitrary_gemini_paths_as_gemini() {
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%707".to_string(),
        pane_pid: 707,
        pane_current_command: "node".to_string(),
        pane_title_raw: "Working".to_string(),
        pane_tty: "/dev/pts/707".to_string(),
        pane_current_path: "/tmp/node-wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
    let inspector = FakeProcessInspector::with_processes([(
        707,
        vec![proc::ProcessEvidence {
            pid: 708,
            command: "node".to_string(),
            argv: vec!["node".to_string(), "/workspace/tools/gemini".to_string()],
            env: Vec::new(),
        }],
    )]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_unresolved_ambiguous_pane(&pane, "Working");
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::NoMatch
    );
}

#[test]
fn proc_fallback_resolves_opencode_from_node_package_shim() {
    let mut pane = proc_fallback_pane(720, "node", "Review plan");
    let inspector = FakeProcessInspector::with_processes([(
        720,
        vec![proc::ProcessEvidence {
            pid: 721,
            command: "node".to_string(),
            argv: vec![
                "node".to_string(),
                "/Users/auro/project/node_modules/opencode/bin/opencode".to_string(),
            ],
            env: Vec::new(),
        }],
    )]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, Some(Provider::Opencode));
    assert_eq!(pane.status.kind, StatusKind::Unknown);
    assert_eq!(pane.display.label, "Review plan");
    assert_eq!(pane.display.activity_label.as_deref(), Some("Review plan"));
    assert_eq!(
        pane.classification.reasons,
        vec!["proc_descendant_argv=/Users/auro/project/node_modules/opencode/bin/opencode"]
    );
}

#[test]
fn proc_fallback_resolves_opencode_from_published_npm_package() {
    let mut pane = proc_fallback_pane(728, "node", "Review plan");
    let inspector = FakeProcessInspector::with_processes([(
        728,
        vec![proc::ProcessEvidence {
            pid: 729,
            command: "node".to_string(),
            argv: vec![
                "node".to_string(),
                "/Users/auro/project/node_modules/opencode-ai/bin/opencode".to_string(),
            ],
            env: Vec::new(),
        }],
    )]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, Some(Provider::Opencode));
    assert_eq!(
        pane.classification.reasons,
        vec!["proc_descendant_argv=/Users/auro/project/node_modules/opencode-ai/bin/opencode"]
    );
}

#[test]
fn proc_fallback_resolves_opencode_from_platform_binary_package() {
    let mut pane = proc_fallback_pane(721, "node", "Review plan");
    let inspector = FakeProcessInspector::with_processes([(
        721,
        vec![proc::ProcessEvidence {
            pid: 722,
            command: "node".to_string(),
            argv: vec![
                "node".to_string(),
                "/Users/auro/project/node_modules/opencode-darwin-arm64/bin/opencode".to_string(),
            ],
            env: Vec::new(),
        }],
    )]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, Some(Provider::Opencode));
    assert_eq!(
        pane.classification.reasons,
        vec![
            "proc_descendant_argv=/Users/auro/project/node_modules/opencode-darwin-arm64/bin/opencode"
                .to_string()
        ]
    );
}

#[test]
fn proc_fallback_resolves_opencode_from_source_entrypoint() {
    let mut pane = proc_fallback_pane(722, "bun", "Review plan");
    let inspector = FakeProcessInspector::with_processes([(
        722,
        vec![proc::ProcessEvidence {
            pid: 723,
            command: "bun".to_string(),
            argv: vec![
                "bun".to_string(),
                "/Users/auro/code/upstream/opencode/packages/opencode/src/index.ts".to_string(),
            ],
            env: Vec::new(),
        }],
    )]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, Some(Provider::Opencode));
    assert_eq!(pane.display.label, "Review plan");
    assert_eq!(pane.display.activity_label.as_deref(), Some("Review plan"));
    assert_eq!(
        pane.classification.reasons,
        vec![
            "proc_descendant_argv=/Users/auro/code/upstream/opencode/packages/opencode/src/index.ts"
                .to_string()
        ]
    );
}

#[test]
fn proc_fallback_resolves_opencode_from_known_bin_shim() {
    let mut pane = proc_fallback_pane(723, "node", "Review plan");
    let inspector = FakeProcessInspector::with_processes([(
        723,
        vec![proc::ProcessEvidence {
            pid: 724,
            command: "node".to_string(),
            argv: vec!["node".to_string(), "/opt/homebrew/bin/opencode".to_string()],
            env: Vec::new(),
        }],
    )]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, Some(Provider::Opencode));
    assert_eq!(
        pane.classification.reasons,
        vec!["proc_descendant_argv=/opt/homebrew/bin/opencode"]
    );
}

#[test]
fn proc_fallback_resolves_opencode_from_env_marker() {
    let mut pane = proc_fallback_pane(724, "node", "");
    let inspector = FakeProcessInspector::with_processes([(
        724,
        vec![proc::ProcessEvidence {
            pid: 724,
            command: "node".to_string(),
            argv: vec!["node".to_string()],
            env: vec![
                ("OPENCODE".to_string(), "1".to_string()),
                ("OPENCODE_PID".to_string(), "724".to_string()),
            ],
        }],
    )]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, Some(Provider::Opencode));
    assert_eq!(
        pane.classification.reasons,
        vec!["proc_descendant_env=OPENCODE"]
    );
}

#[test]
fn proc_fallback_does_not_treat_arbitrary_opencode_paths_as_opencode() {
    for (pid, argv_path) in [
        (725, "/workspace/tools/opencode"),
        (
            726,
            "/Users/auro/project/node_modules/opencode/bin/opencode-helper",
        ),
        (
            727,
            "/Users/auro/project/node_modules/opencode-helper/bin/opencode",
        ),
        (
            728,
            "/Users/auro/project/node_modules/opencode-ai-helper/bin/opencode",
        ),
    ] {
        let mut pane = proc_fallback_pane(pid, "node", "Review plan");
        let inspector = FakeProcessInspector::with_processes([(
            pid,
            vec![proc::ProcessEvidence {
                pid: pid + 100,
                command: "node".to_string(),
                argv: vec!["node".to_string(), argv_path.to_string()],
                env: Vec::new(),
            }],
        )]);

        classify::apply_proc_fallback(&mut pane, &inspector);

        assert_unresolved_ambiguous_pane(&pane, "Review plan");
        assert_eq!(
            pane.diagnostics.proc_fallback.outcome,
            super::ProcFallbackOutcome::NoMatch,
            "unexpected opencode match for {argv_path}"
        );
    }
}

#[test]
fn proc_fallback_does_not_treat_opencode_env_text_in_argv_as_opencode() {
    let mut pane = proc_fallback_pane(726, "node", "Review plan");
    let inspector = FakeProcessInspector::with_processes([(
        726,
        vec![proc::ProcessEvidence {
            pid: 727,
            command: "node".to_string(),
            argv: vec![
                "node".to_string(),
                "script.js".to_string(),
                "--data".to_string(),
                "OPENCODE=1 OPENCODE_PID=727".to_string(),
            ],
            env: Vec::new(),
        }],
    )]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_unresolved_ambiguous_pane(&pane, "Review plan");
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::NoMatch
    );
}

#[test]
fn proc_fallback_requires_correlated_opencode_env() {
    let mut pane = proc_fallback_pane(727, "node", "Review plan");
    let inspector = FakeProcessInspector::with_processes([(
        727,
        vec![proc::ProcessEvidence {
            pid: 728,
            command: "node".to_string(),
            argv: vec!["node".to_string()],
            env: vec![
                ("OPENCODE".to_string(), "1".to_string()),
                ("AGENT".to_string(), "1".to_string()),
            ],
        }],
    )]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_unresolved_ambiguous_pane(&pane, "Review plan");
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::NoMatch
    );
}

#[test]
fn proc_fallback_resolves_pi_from_env_marker() {
    let mut pane = proc_fallback_pane(730, "pi", "");
    let inspector = FakeProcessInspector::with_processes([(
        730,
        vec![proc::ProcessEvidence {
            pid: 730,
            command: "pi".to_string(),
            argv: vec!["pi".to_string()],
            env: vec![("PI_CODING_AGENT".to_string(), "true".to_string())],
        }],
    )]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, Some(Provider::Pi));
    assert_eq!(
        pane.classification.reasons,
        vec!["proc_descendant_env=PI_CODING_AGENT"]
    );
    assert_eq!(pane.status.kind, StatusKind::Unknown);
}

#[test]
fn proc_fallback_resolves_pi_from_package_cli_path() {
    let mut pane = proc_fallback_pane(731, "node", "Review plan");
    let inspector = FakeProcessInspector::with_processes([(
        731,
        vec![proc::ProcessEvidence {
            pid: 732,
            command: "node".to_string(),
            argv: vec![
                "node".to_string(),
                "/opt/homebrew/lib/node_modules/@mariozechner/pi-coding-agent/dist/cli.js"
                    .to_string(),
            ],
            env: Vec::new(),
        }],
    )]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, Some(Provider::Pi));
    assert_eq!(pane.display.label, "Review plan");
    assert_eq!(
        pane.classification.reasons,
        vec![
            "proc_descendant_argv=/opt/homebrew/lib/node_modules/@mariozechner/pi-coding-agent/dist/cli.js"
                .to_string()
        ]
    );
}

#[test]
fn proc_fallback_resolves_pi_from_build_binary_path() {
    let mut pane = proc_fallback_pane(732, "bun", "Review plan");
    let inspector = FakeProcessInspector::with_processes([(
        732,
        vec![proc::ProcessEvidence {
            pid: 733,
            command: "bun".to_string(),
            argv: vec![
                "bun".to_string(),
                "/Users/auro/code/upstream/pi-mono/packages/coding-agent/dist/pi".to_string(),
            ],
            env: Vec::new(),
        }],
    )]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, Some(Provider::Pi));
    assert_eq!(
        pane.classification.reasons,
        vec![
            "proc_descendant_argv=/Users/auro/code/upstream/pi-mono/packages/coding-agent/dist/pi"
                .to_string()
        ]
    );
}

#[test]
fn proc_fallback_resolves_pi_from_known_bin_shim() {
    let mut pane = proc_fallback_pane(733, "node", "Review plan");
    let inspector = FakeProcessInspector::with_processes([(
        733,
        vec![proc::ProcessEvidence {
            pid: 734,
            command: "node".to_string(),
            argv: vec!["node".to_string(), "/opt/homebrew/bin/pi".to_string()],
            env: Vec::new(),
        }],
    )]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, Some(Provider::Pi));
    assert_eq!(
        pane.classification.reasons,
        vec!["proc_descendant_argv=/opt/homebrew/bin/pi"]
    );
}

#[test]
fn proc_fallback_does_not_treat_arbitrary_pi_paths_as_pi() {
    let mut pane = proc_fallback_pane(734, "node", "Review plan");
    let inspector = FakeProcessInspector::with_processes([(
        734,
        vec![proc::ProcessEvidence {
            pid: 735,
            command: "node".to_string(),
            argv: vec!["node".to_string(), "/workspace/tools/pi".to_string()],
            env: Vec::new(),
        }],
    )]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_unresolved_ambiguous_pane(&pane, "Review plan");
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::NoMatch
    );
}

#[test]
fn proc_fallback_does_not_treat_bare_pi_process_as_pi() {
    let mut pane = proc_fallback_pane(735, "pi", "");
    let inspector = FakeProcessInspector::with_processes([(
        735,
        vec![proc::ProcessEvidence {
            pid: 735,
            command: "pi".to_string(),
            argv: vec!["pi".to_string()],
            env: Vec::new(),
        }],
    )]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, None);
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::NoMatch
    );
}

#[test]
fn proc_fallback_does_not_treat_pi_env_text_in_argv_as_pi() {
    let mut pane = proc_fallback_pane(736, "node", "Review plan");
    let inspector = FakeProcessInspector::with_processes([(
        736,
        vec![proc::ProcessEvidence {
            pid: 737,
            command: "node".to_string(),
            argv: vec![
                "node".to_string(),
                "script.js".to_string(),
                "--data".to_string(),
                "PI_CODING_AGENT=true".to_string(),
            ],
            env: Vec::new(),
        }],
    )]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_unresolved_ambiguous_pane(&pane, "Review plan");
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::NoMatch
    );
}

#[test]
fn proc_fallback_resolves_claude_from_title_glyph_and_descendant_command() {
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%711".to_string(),
        pane_pid: 711,
        pane_current_command: "2.1.119".to_string(),
        pane_title_raw: "✳ Analyze Linear Issue AUR-126 and plan implementation".to_string(),
        pane_tty: "/dev/pts/711".to_string(),
        pane_current_path: "/tmp/claude-wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
    let inspector = FakeProcessInspector::new([(711, vec!["claude".to_string()])]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, Some(Provider::Claude));
    assert_eq!(pane.status.kind, StatusKind::Idle);
    assert_eq!(pane.status.source, super::StatusSource::TmuxTitle);
    assert_eq!(
        pane.display.label,
        "Analyze Linear Issue AUR-126 and plan implementation"
    );
    assert_eq!(
        pane.classification.reasons,
        vec!["proc_descendant_command=claude"]
    );
    assert_eq!(inspector.calls(), vec![711]);
}

#[test]
fn proc_fallback_ignores_version_like_current_command_without_other_signal() {
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%712".to_string(),
        pane_pid: 712,
        pane_current_command: "2.1.119".to_string(),
        pane_title_raw: "Ready".to_string(),
        pane_tty: "/dev/pts/712".to_string(),
        pane_current_path: "/tmp/claude-wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
    let inspector = FakeProcessInspector::new([(712, vec!["claude".to_string()])]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_unresolved_ambiguous_pane(&pane, "Ready");
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::Skipped
    );
    assert_eq!(
        pane.diagnostics.proc_fallback.reason,
        "pane_current_command is version-shaped and ignored"
    );
    assert!(inspector.calls().is_empty());
}

#[test]
fn proc_fallback_resolves_claude_teammate_flags_with_claudecode_env() {
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%705".to_string(),
        pane_pid: 705,
        pane_current_command: "node".to_string(),
        pane_title_raw: "worker-a".to_string(),
        pane_tty: "/dev/pts/705".to_string(),
        pane_current_path: "/tmp/node-wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
    let inspector = FakeProcessInspector::with_processes([(
        705,
        vec![proc::ProcessEvidence {
            pid: 706,
            command: "node".to_string(),
            argv: vec![
                "node".to_string(),
                "/tmp/cli.mjs".to_string(),
                "--agent-id".to_string(),
                "worker-a@team".to_string(),
                "--agent-name".to_string(),
                "worker-a".to_string(),
                "--team-name".to_string(),
                "team".to_string(),
            ],
            env: vec![("CLAUDECODE".to_string(), "1".to_string())],
        }],
    )]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, Some(Provider::Claude));
    assert_eq!(
        pane.classification.reasons,
        vec!["proc_descendant_argv=claude teammate flags"]
    );
    assert_eq!(pane.display.label, "worker-a");
}

#[test]
fn proc_fallback_resolves_claude_teammate_from_shell_env_assignment() {
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%707".to_string(),
        pane_pid: 707,
        pane_current_command: "node".to_string(),
        pane_title_raw: "worker-a".to_string(),
        pane_tty: "/dev/pts/707".to_string(),
        pane_current_path: "/tmp/node-wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
    let inspector = FakeProcessInspector::with_processes([(
        707,
        vec![proc::ProcessEvidence {
            pid: 708,
            command: "sh".to_string(),
            argv: vec![
                "sh".to_string(),
                "-c".to_string(),
                "env CLAUDECODE=1 claude --agent-id worker-a --agent-name worker-a --team-name team"
                    .to_string(),
            ],
            env: Vec::new(),
        }],
    )]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, Some(Provider::Claude));
    assert_eq!(
        pane.classification.reasons,
        vec!["proc_descendant_argv=claude teammate flags"]
    );
    assert_eq!(pane.display.label, "worker-a");
}

#[test]
fn proc_fallback_does_not_treat_teammate_flags_without_claude_env_as_claude() {
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%706".to_string(),
        pane_pid: 706,
        pane_current_command: "node".to_string(),
        pane_title_raw: "worker-a".to_string(),
        pane_tty: "/dev/pts/706".to_string(),
        pane_current_path: "/tmp/node-wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
    let inspector = FakeProcessInspector::with_processes([(
        706,
        vec![proc::ProcessEvidence {
            pid: 707,
            command: "node".to_string(),
            argv: vec![
                "node".to_string(),
                "/tmp/not-claude.js".to_string(),
                "--agent-id=worker-a".to_string(),
                "--agent-name=worker-a".to_string(),
                "--team-name=team".to_string(),
            ],
            env: Vec::new(),
        }],
    )]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_unresolved_ambiguous_pane(&pane, "worker-a");
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::NoMatch
    );
}

#[test]
fn proc_fallback_does_not_treat_claude_substrings_as_claude() {
    for (pid, argv_path) in [
        (708, "/project/node_modules/.bin/claude-lint"),
        (709, "/work/claude-helper/cli.mjs"),
        (710, "/workspace/tools/claude"),
        (711, "/workspace/tools/claude-code"),
    ] {
        let mut pane = classify::pane_from_row(super::TmuxPaneRow {
            session_name: "ambiguous".to_string(),
            window_index: 1,
            pane_index: 1,
            pane_id: format!("%{pid}"),
            pane_pid: pid,
            pane_current_command: "node".to_string(),
            pane_title_raw: "Working".to_string(),
            pane_tty: format!("/dev/pts/{pid}"),
            pane_current_path: "/tmp/node-wrapper".to_string(),
            window_name: "ai".to_string(),
            session_id: None,
            window_id: None,
            agent_provider: None,
            agent_label: None,
            agent_cwd: None,
            agent_state: None,
            agent_session_id: None,
        });
        let inspector = FakeProcessInspector::with_processes([(
            pid,
            vec![proc::ProcessEvidence {
                pid: pid + 100,
                command: "node".to_string(),
                argv: vec!["node".to_string(), argv_path.to_string()],
                env: Vec::new(),
            }],
        )]);

        classify::apply_proc_fallback(&mut pane, &inspector);

        assert_unresolved_ambiguous_pane(&pane, "Working");
        assert_eq!(
            pane.diagnostics.proc_fallback.outcome,
            super::ProcFallbackOutcome::NoMatch
        );
    }
}

#[test]
fn proc_fallback_skips_panes_resolved_by_existing_precedence() {
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "metadata".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%701".to_string(),
        pane_pid: 701,
        pane_current_command: "node".to_string(),
        pane_title_raw: "Working".to_string(),
        pane_tty: "/dev/pts/701".to_string(),
        pane_current_path: "/tmp/node-wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: Some("claude".to_string()),
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
    let inspector = FakeProcessInspector::new([(701, vec!["codex".to_string()])]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, Some(Provider::Claude));
    assert_eq!(
        pane.classification.matched_by,
        Some(super::ClassificationMatchKind::PaneMetadata)
    );
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::Skipped
    );
    assert_eq!(
        pane.diagnostics.proc_fallback.reason,
        "provider already resolved by pane_metadata"
    );
    assert!(inspector.calls().is_empty());
}

#[test]
fn proc_fallback_records_skip_reason_for_untargeted_unresolved_pane() {
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%702".to_string(),
        pane_pid: 702,
        pane_current_command: "make".to_string(),
        pane_title_raw: "(bront) ~/code/agent-wrapper".to_string(),
        pane_tty: "/dev/pts/702".to_string(),
        pane_current_path: "/tmp/wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
    let inspector = FakeProcessInspector::new([(702, vec!["codex".to_string()])]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_unresolved_ambiguous_pane(&pane, "(bront) ~/code/agent-wrapper");
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::Skipped
    );
    assert_eq!(
        pane.diagnostics.proc_fallback.reason,
        "pane_current_command=make is not a targeted proc fallback launcher"
    );
    assert!(inspector.calls().is_empty());
}

#[test]
fn proc_fallback_resolves_shell_pane_from_foreground_process() {
    let mut pane = proc_fallback_pane(740, "zsh", "agent wrapper");
    pane.tmux.pane_tty = "/dev/ttys740".to_string();
    let inspector = FakeProcessInspector::with_foreground(
        [(740, vec!["background-codex".to_string()])],
        [("/dev/ttys740".to_string(), vec!["copilot".to_string()])],
    );

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, Some(Provider::Copilot));
    assert_eq!(
        pane.classification.reasons,
        vec!["proc_foreground_command=copilot"]
    );
    assert_eq!(inspector.calls(), Vec::<u32>::new());
    assert_eq!(
        inspector.foreground_calls(),
        vec!["/dev/ttys740".to_string()]
    );
}

#[test]
fn inspect_text_reports_provider_status_and_fallback_provenance() {
    let rows =
        tmux::parse_pane_rows(TMUX_AMBIGUOUS_FIXTURE).expect("ambiguous fixture should parse");
    let inspector = FakeProcessInspector::new([(602001, vec!["codex".to_string()])]);
    let panes = classify::panes_from_rows_with_proc_fallback(rows, &inspector);
    let text = output::inspect_text(pane_by_id(&panes, "%601"));

    assert!(text.contains("provider: codex"));
    assert!(text.contains("provider_source: proc_process_tree"));
    assert!(text.contains("provider_confidence: high"));
    assert!(text.contains("status: busy"));
    assert!(text.contains("status_source: tmux_title"));
    assert!(text.contains("classification:\n  - proc_descendant_command=codex"));
    assert!(text.contains("proc_fallback:\n  outcome: resolved"));
    assert!(text.contains("  reason: resolved provider from process evidence"));
    assert!(text.contains("  commands:\n    - codex"));
}

#[test]
fn inspect_text_reports_unresolved_fallback_decision() {
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%703".to_string(),
        pane_pid: 703,
        pane_current_command: "node".to_string(),
        pane_title_raw: "Working".to_string(),
        pane_tty: "/dev/pts/703".to_string(),
        pane_current_path: "/tmp/node-wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
    let inspector = FakeProcessInspector::new([(703, vec!["node".to_string()])]);

    classify::apply_proc_fallback(&mut pane, &inspector);
    let text = output::inspect_text(&pane);

    assert!(text.contains("provider: unknown"));
    assert!(text.contains("provider_source: none"));
    assert!(text.contains("status_source: not_checked"));
    assert!(text.contains("classification: none"));
    assert!(text.contains("proc_fallback:\n  outcome: no_match"));
    assert!(text.contains("  reason: no known provider evidence found in descendants"));
    assert!(text.contains("  commands:\n    - node"));
}

#[test]
fn pane_metadata_overrides_display_provider_and_status_when_title_is_ambiguous() {
    let pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "wrapper".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%500".to_string(),
        pane_pid: 500,
        pane_current_command: "zsh".to_string(),
        pane_title_raw: "(bront) ~/code/wrapper".to_string(),
        pane_tty: "/dev/pts/500".to_string(),
        pane_current_path: "/home/auro/code/wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: Some("claude".to_string()),
        agent_label: Some("Wrapper Claude Task".to_string()),
        agent_cwd: Some("/tmp/wrapper".to_string()),
        agent_state: Some("idle".to_string()),
        agent_session_id: Some("sess-123".to_string()),
    });

    assert_eq!(pane.provider, Some(Provider::Claude));
    assert_eq!(pane.display.label, "Wrapper Claude Task");
    assert_eq!(
        pane.display.activity_label.as_deref(),
        Some("Wrapper Claude Task")
    );
    assert_eq!(pane.status.kind, StatusKind::Idle);
    assert_eq!(pane.status.source, super::StatusSource::PaneMetadata);
    assert_eq!(
        pane.classification.matched_by,
        Some(super::ClassificationMatchKind::PaneMetadata)
    );
    assert_eq!(pane.agent_metadata.provider.as_deref(), Some("claude"));
    assert_eq!(pane.agent_metadata.session_id.as_deref(), Some("sess-123"));
}

#[test]
fn fixture_snapshot_with_metadata_parses_wrapper_fields() {
    let rows = tmux::parse_pane_rows(TMUX_METADATA_FIXTURE).expect("metadata fixture should parse");
    let panes: Vec<_> = rows.into_iter().map(classify::pane_from_row).collect();

    let pane = pane_by_id(&panes, "%400");
    assert_eq!(pane.provider, Some(Provider::Claude));
    assert_eq!(pane.display.label, "Wrapped Claude Task");
    assert_eq!(pane.status.kind, StatusKind::Busy);
    assert_eq!(pane.status.source, super::StatusSource::PaneMetadata);
    assert_eq!(
        pane.agent_metadata.cwd.as_deref(),
        Some("/tmp/wrapper-meta")
    );
    assert_eq!(pane.agent_metadata.session_id.as_deref(), Some("sess-123"));
}

#[test]
fn cache_fixture_deserializes_into_current_schema() {
    let snapshot: SnapshotEnvelope =
        serde_json::from_str(CACHE_SNAPSHOT_FIXTURE).expect("cache fixture should parse");

    assert_eq!(snapshot.schema_version, CACHE_SCHEMA_VERSION);
    assert_eq!(snapshot.source.kind, SourceKind::Daemon);
    assert_eq!(
        snapshot.source.daemon_generated_at.as_deref(),
        Some("2026-03-28T00:00:00Z")
    );
    assert_eq!(snapshot.panes.len(), 1);
    assert_eq!(snapshot.panes[0].pane_id, "%67");
    assert_eq!(snapshot.panes[0].status.kind, StatusKind::Idle);
    assert_eq!(snapshot.panes[0].tmux.session_id.as_deref(), Some("$1"));
    assert_eq!(snapshot.panes[0].tmux.window_id.as_deref(), Some("@1"));
    assert_eq!(
        snapshot.panes[0].diagnostics.cache_origin,
        "daemon_snapshot"
    );
}

#[test]
fn cache_summary_counts_fixture_contents() {
    let snapshot: SnapshotEnvelope =
        serde_json::from_str(CACHE_SNAPSHOT_FIXTURE).expect("cache fixture should parse");

    let summary = cache::summarize_snapshot(&snapshot).expect("cache fixture should summarize");
    assert_eq!(summary.pane_count, 1);
    assert_eq!(summary.agent_pane_count, 1);
    assert_eq!(summary.provider_counts, vec![(Provider::Codex, 1)]);
    assert_eq!(summary.status_counts, vec![(StatusKind::Idle, 1)]);
}

#[test]
fn snapshot_sort_orders_panes_by_location() {
    let mut snapshot = SnapshotEnvelope {
        schema_version: CACHE_SCHEMA_VERSION,
        generated_at: "2026-03-28T00:00:00Z".to_string(),
        source: super::SnapshotSource {
            kind: SourceKind::Snapshot,
            tmux_version: None,
            daemon_generated_at: None,
        },
        panes: vec![
            classify::pane_from_row(super::TmuxPaneRow {
                session_name: "zeta".to_string(),
                window_index: 2,
                pane_index: 1,
                pane_id: "%3".to_string(),
                pane_pid: 3,
                pane_current_command: "codex".to_string(),
                pane_title_raw: "Ready".to_string(),
                pane_tty: "/dev/pts/3".to_string(),
                pane_current_path: "/tmp/zeta".to_string(),
                window_name: "editor".to_string(),
                session_id: None,
                window_id: None,
                agent_provider: None,
                agent_label: None,
                agent_cwd: None,
                agent_state: None,
                agent_session_id: None,
            }),
            classify::pane_from_row(super::TmuxPaneRow {
                session_name: "alpha".to_string(),
                window_index: 1,
                pane_index: 2,
                pane_id: "%2".to_string(),
                pane_pid: 2,
                pane_current_command: "claude".to_string(),
                pane_title_raw: "✳ Review".to_string(),
                pane_tty: "/dev/pts/2".to_string(),
                pane_current_path: "/tmp/alpha".to_string(),
                window_name: "ai".to_string(),
                session_id: None,
                window_id: None,
                agent_provider: None,
                agent_label: None,
                agent_cwd: None,
                agent_state: None,
                agent_session_id: None,
            }),
            classify::pane_from_row(super::TmuxPaneRow {
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
            }),
        ],
    };

    cache::sort_snapshot_panes(&mut snapshot);

    let ordered_ids: Vec<_> = snapshot
        .panes
        .iter()
        .map(|pane| pane.pane_id.as_str())
        .collect();
    assert_eq!(ordered_ids, vec!["%1", "%2", "%3"]);
}

#[test]
fn validate_snapshot_rejects_unsupported_schema_version() {
    let mut snapshot: SnapshotEnvelope =
        serde_json::from_str(CACHE_SNAPSHOT_FIXTURE).expect("cache fixture should parse");
    snapshot.schema_version = CACHE_SCHEMA_VERSION - 1;

    let error =
        cache::validate_snapshot(&snapshot, None).expect_err("old schema version should fail");
    assert!(
        error
            .to_string()
            .contains("unsupported cache schema version"),
        "unexpected error: {error}"
    );
}

#[test]
fn validate_snapshot_rejects_future_schema_version() {
    let mut snapshot: SnapshotEnvelope =
        serde_json::from_str(CACHE_SNAPSHOT_FIXTURE).expect("cache fixture should parse");
    snapshot.schema_version = CACHE_SCHEMA_VERSION + 1;

    let error =
        cache::validate_snapshot(&snapshot, None).expect_err("future schema version should fail");
    assert!(
        error
            .to_string()
            .contains("unsupported cache schema version"),
        "unexpected error: {error}"
    );
}

#[test]
fn validate_snapshot_rejects_stale_cache_when_max_age_is_exceeded() {
    let mut snapshot: SnapshotEnvelope =
        serde_json::from_str(CACHE_SNAPSHOT_FIXTURE).expect("cache fixture should parse");
    snapshot.generated_at = "2020-01-01T00:00:00Z".to_string();

    let error = cache::validate_snapshot(&snapshot, Some(1)).expect_err("stale cache should fail");
    assert!(
        error.to_string().contains("cache is stale"),
        "unexpected error: {error}"
    );
}

#[test]
fn status_names_match_serialized_values() {
    assert_eq!(super::status_kind_name(StatusKind::Busy), "busy");
    assert_eq!(super::status_kind_name(StatusKind::Idle), "idle");
    assert_eq!(super::status_kind_name(StatusKind::Unknown), "unknown");
    assert_eq!(
        super::status_source_name(super::StatusSource::PaneOutput),
        "pane_output"
    );
}

#[test]
fn known_status_glyph_stripping_preserves_normal_prefixes() {
    assert_eq!(
        classify::strip_known_status_glyph("(bront) parallel-n64: codex"),
        "(bront) parallel-n64: codex"
    );
    assert_eq!(
        classify::strip_known_status_glyph("✳ Review and summarize todo list"),
        "Review and summarize todo list"
    );
}

#[test]
fn title_normalization_strips_claude_and_opencode_prefixes() {
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Claude), "Claude Code | Query"),
        "Query"
    );
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Claude), "Claude | Ready"),
        "Ready"
    );
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Opencode), "OC | Query planner"),
        "Query planner"
    );
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Copilot), "Copilot | Review patch"),
        "Review patch"
    );
    assert_eq!(
        classify::normalize_title_for_display(
            Some(Provider::CursorCli),
            "Cursor CLI | Query planner"
        ),
        "Query planner"
    );
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Pi), "π - refactor - agentscan"),
        "refactor - agentscan"
    );
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Pi), "pi - agentscan"),
        "pi - agentscan"
    );
    assert_eq!(
        classify::normalize_title_for_display(
            Some(Provider::Codex),
            "(bront) parallel-n64: /home/auro/.zshrc.d/scripts/lgpt.sh"
        ),
        "(bront) parallel-n64"
    );
    assert_eq!(
        classify::normalize_title_for_display(
            Some(Provider::Codex),
            "(repo) task: codex --model gpt-5"
        ),
        "(repo) task"
    );
    assert_eq!(
        classify::normalize_title_for_display(
            Some(Provider::Codex),
            "pi - refactor - agentscan: codex"
        ),
        "pi - refactor - agentscan"
    );
    assert_eq!(
        classify::normalize_title_for_display(
            Some(Provider::Codex),
            "Copilot | Review patch: codex"
        ),
        "Copilot | Review patch"
    );
    assert_eq!(
        classify::normalize_title_for_display(
            Some(Provider::Codex),
            "Cursor CLI | Parser work: codex"
        ),
        "Cursor CLI | Parser work"
    );
    assert_eq!(
        classify::normalize_title_for_display(
            Some(Provider::Codex),
            "Working | Review code quality in repository"
        ),
        "Review code quality in repository"
    );
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Codex), "agentscan | Waiting"),
        "agentscan"
    );
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Codex), "Ready | Working"),
        "Ready"
    );
    assert_eq!(
        classify::normalize_title_for_display(
            Some(Provider::Codex),
            "review codex login | Working"
        ),
        "review codex login"
    );
    assert_eq!(
        classify::normalize_title_for_display(
            Some(Provider::Codex),
            "(repo) task: codex --model gpt-5 | Working"
        ),
        "(repo) task"
    );
    assert_eq!(
        classify::normalize_title_for_display(
            Some(Provider::Codex),
            "repo: /path/lgpt.sh | Working"
        ),
        "repo"
    );
    assert_eq!(
        classify::normalize_title_for_display(
            Some(Provider::Codex),
            "Working | (repo) task: codex --model gpt-5"
        ),
        "(repo) task"
    );
    assert_eq!(
        classify::normalize_title_for_display(None, "Working | deploy notes"),
        "Working | deploy notes"
    );
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Claude), "Working | deploy notes"),
        "Working | deploy notes"
    );
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Codex), "Copilot | Review patch"),
        "Copilot | Review patch"
    );
}

#[test]
fn title_normalization_strips_gemini_status_titles() {
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Gemini), "◇  Ready (workspace)"),
        "workspace"
    );
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Gemini), "✦  Working… (workspace)"),
        "workspace"
    );
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Gemini), "✦  Working… (repo (copy))"),
        "repo (copy)"
    );
    assert_eq!(
        classify::normalize_title_for_display(
            Some(Provider::Gemini),
            "✦  Processing request (workspace)"
        ),
        "Processing request"
    );
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Gemini), "Gemini CLI (workspace)"),
        "workspace"
    );
}

#[test]
fn display_metadata_extracts_activity_labels_from_titles() {
    let codex = classify::display_metadata(
        Some(Provider::Codex),
        None,
        None,
        "⠹ agentscan | Working",
        "codex",
        "editor",
    );
    assert_eq!(codex.label, "agentscan");
    assert_eq!(codex.activity_label.as_deref(), Some("agentscan"));

    let codex_status_first = classify::display_metadata(
        Some(Provider::Codex),
        None,
        None,
        "Working | Review code quality in repository",
        "codex",
        "editor",
    );
    assert_eq!(
        codex_status_first.label,
        "Review code quality in repository"
    );
    assert_eq!(
        codex_status_first.activity_label.as_deref(),
        Some("Review code quality in repository")
    );

    let codex_status_last_wins = classify::display_metadata(
        Some(Provider::Codex),
        None,
        None,
        "Ready | Working",
        "codex",
        "editor",
    );
    assert_eq!(codex_status_last_wins.label, "Ready");
    assert_eq!(
        codex_status_last_wins.activity_label.as_deref(),
        Some("Ready")
    );

    let codex_wrapped = classify::display_metadata(
        Some(Provider::Codex),
        None,
        None,
        "pi - refactor - agentscan: codex",
        "zsh",
        "editor",
    );
    assert_eq!(codex_wrapped.label, "pi - refactor - agentscan");
    assert_eq!(
        codex_wrapped.activity_label.as_deref(),
        Some("pi - refactor - agentscan")
    );

    let claude = classify::display_metadata(
        Some(Provider::Claude),
        None,
        None,
        "✳ Review and summarize todo list",
        "claude",
        "ai",
    );
    assert_eq!(claude.label, "Review and summarize todo list");
    assert_eq!(
        claude.activity_label.as_deref(),
        Some("Review and summarize todo list")
    );

    let generic = classify::display_metadata(
        Some(Provider::Codex),
        None,
        None,
        "Ready",
        "codex",
        "editor",
    );
    assert_eq!(generic.label, "Ready");
    assert_eq!(generic.activity_label, None);

    let wrapped_codex = classify::display_metadata(
        Some(Provider::Codex),
        None,
        None,
        "(bront) parallel-n64: /home/auro/.zshrc.d/scripts/lgpt.sh",
        "zsh",
        "editor",
    );
    assert_eq!(wrapped_codex.label, "(bront) parallel-n64");
    assert_eq!(
        wrapped_codex.activity_label.as_deref(),
        Some("(bront) parallel-n64")
    );

    let published = classify::display_metadata(
        Some(Provider::Claude),
        None,
        Some("Wrapper Claude Task"),
        "Claude Code | Working",
        "zsh",
        "ai",
    );
    assert_eq!(published.label, "Wrapper Claude Task");
    assert_eq!(
        published.activity_label.as_deref(),
        Some("Wrapper Claude Task")
    );

    let copilot = classify::display_metadata(
        Some(Provider::Copilot),
        None,
        None,
        "Copilot | Review patch",
        "copilot",
        "ai",
    );
    assert_eq!(copilot.label, "Review patch");
    assert_eq!(copilot.activity_label.as_deref(), Some("Review patch"));

    let cursor = classify::display_metadata(
        Some(Provider::CursorCli),
        Some(super::ClassificationMatchKind::PaneCurrentCommand),
        None,
        "Cursor CLI | Query planner",
        "cursor-agent",
        "ai",
    );
    assert_eq!(cursor.label, "Query planner");
    assert_eq!(cursor.activity_label.as_deref(), Some("Query planner"));

    let pi = classify::display_metadata(
        Some(Provider::Pi),
        None,
        None,
        "π - refactor - agentscan",
        "pi",
        "ai",
    );
    assert_eq!(pi.label, "refactor - agentscan");
    assert_eq!(pi.activity_label, None);

    let prefixed_codex = classify::display_metadata(
        Some(Provider::Codex),
        Some(super::ClassificationMatchKind::PaneTitle),
        None,
        "Copilot | Review patch: codex",
        "zsh",
        "ai",
    );
    assert_eq!(prefixed_codex.label, "Copilot | Review patch");
    assert_eq!(
        prefixed_codex.activity_label.as_deref(),
        Some("Copilot | Review patch")
    );
}

#[test]
fn copilot_default_title_does_not_invent_activity_label() {
    let copilot_default = classify::display_metadata(
        Some(Provider::Copilot),
        Some(super::ClassificationMatchKind::PaneTitle),
        None,
        "GitHub Copilot",
        "node",
        "ai",
    );

    assert_eq!(copilot_default.label, "GitHub Copilot");
    assert_eq!(copilot_default.activity_label, None);
}

#[test]
fn copilot_pane_output_marks_busy_only_after_provider_is_known() {
    let mut copilot = proc_fallback_pane(745, "node", "GitHub Copilot");
    copilot.provider = Some(Provider::Copilot);
    copilot.status = super::PaneStatus {
        kind: StatusKind::Unknown,
        source: super::StatusSource::NotChecked,
    };

    classify::apply_pane_output_status_fallback(
        &mut copilot,
        "❯ Review patch\n\n\
         ● Thinking (Esc to cancel · 616 B)\n\
         /tmp/probe [main]\n\
         ────────────────────\n\
         ❯\n\
         ────────────────────\n\
         / commands · ? help\n",
    );

    assert_eq!(copilot.status.kind, StatusKind::Busy);
    assert_eq!(copilot.status.source, super::StatusSource::PaneOutput);

    let mut unknown = proc_fallback_pane(746, "node", "custom title");
    classify::apply_pane_output_status_fallback(
        &mut unknown,
        "❯ Review patch\n\n\
         ● Thinking (Esc to cancel · 616 B)\n\
         /tmp/probe [main]\n\
         ────────────────────\n\
         ❯\n\
         ────────────────────\n\
         / commands · ? help\n",
    );

    assert_eq!(unknown.status.kind, StatusKind::Unknown);
    assert_eq!(unknown.status.source, super::StatusSource::NotChecked);
}

#[test]
fn copilot_pane_output_ignores_stale_thinking_lines() {
    let mut copilot = proc_fallback_pane(748, "node", "GitHub Copilot");
    copilot.provider = Some(Provider::Copilot);
    copilot.status = super::PaneStatus {
        kind: StatusKind::Unknown,
        source: super::StatusSource::NotChecked,
    };

    classify::apply_pane_output_status_fallback(
        &mut copilot,
        "● Thinking (Esc to cancel · 616 B)\n\
         ● Done! Created result.txt.\n\
         \n\
         /tmp/probe [main]\n\
         ────────────────────\n\
         ❯\n\
         ────────────────────\n\
         / commands · ? help\n",
    );

    assert_eq!(copilot.status.kind, StatusKind::Unknown);
    assert_eq!(copilot.status.source, super::StatusSource::NotChecked);
}

#[test]
fn copilot_pane_output_marks_current_trust_prompt_busy() {
    let mut copilot = proc_fallback_pane(749, "node", "GitHub Copilot");
    copilot.provider = Some(Provider::Copilot);
    copilot.status = super::PaneStatus {
        kind: StatusKind::Unknown,
        source: super::StatusSource::NotChecked,
    };

    classify::apply_pane_output_status_fallback(
        &mut copilot,
        "╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ Confirm folder trust                                                         │\n\
         │ Do you trust the files in this folder?                                       │\n\
         │ ❯ 1. Yes                                                                     │\n\
         │   2. Yes, and remember this folder for future sessions                       │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n",
    );

    assert_eq!(copilot.status.kind, StatusKind::Busy);
    assert_eq!(copilot.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn copilot_pane_output_does_not_infer_idle_from_prompt() {
    let mut copilot = proc_fallback_pane(747, "node", "GitHub Copilot");
    copilot.provider = Some(Provider::Copilot);
    copilot.status = super::PaneStatus {
        kind: StatusKind::Unknown,
        source: super::StatusSource::NotChecked,
    };

    classify::apply_pane_output_status_fallback(
        &mut copilot,
        "/tmp/probe [main]\n────────────────────\n❯\n",
    );

    assert_eq!(copilot.status.kind, StatusKind::Unknown);
    assert_eq!(copilot.status.source, super::StatusSource::NotChecked);
}

#[test]
fn opencode_display_metadata_uses_title_without_activity_state() {
    let opencode = classify::display_metadata(
        Some(Provider::Opencode),
        Some(super::ClassificationMatchKind::PaneTitle),
        None,
        "OC | Review patch",
        "zsh",
        "ai",
    );
    assert_eq!(opencode.label, "Review patch");
    assert_eq!(opencode.activity_label, None);

    let opencode_default = classify::display_metadata(
        Some(Provider::Opencode),
        Some(super::ClassificationMatchKind::PaneTitle),
        None,
        "OpenCode",
        "zsh",
        "ai",
    );
    assert_eq!(opencode_default.label, "OpenCode");
    assert_eq!(opencode_default.activity_label, None);
}

#[test]
fn gemini_display_metadata_separates_context_from_activity() {
    let idle = classify::display_metadata(
        Some(Provider::Gemini),
        Some(super::ClassificationMatchKind::PaneTitle),
        None,
        "◇  Ready (workspace)",
        "zsh",
        "ai",
    );
    assert_eq!(idle.label, "workspace");
    assert_eq!(idle.activity_label, None);

    let static_title = classify::display_metadata(
        Some(Provider::Gemini),
        Some(super::ClassificationMatchKind::PaneTitle),
        None,
        "Gemini CLI (workspace)",
        "zsh",
        "ai",
    );
    assert_eq!(static_title.label, "workspace");
    assert_eq!(static_title.activity_label, None);

    let thought = classify::display_metadata(
        Some(Provider::Gemini),
        Some(super::ClassificationMatchKind::PaneTitle),
        None,
        "✦  Processing request (workspace)",
        "zsh",
        "ai",
    );
    assert_eq!(thought.label, "Processing request");
    assert_eq!(
        thought.activity_label.as_deref(),
        Some("Processing request")
    );
}

#[test]
fn codex_status_activity_labels_strip_wrapper_suffixes() {
    let status_first = classify::display_metadata(
        Some(Provider::Codex),
        None,
        None,
        "Working | Review patch: codex",
        "codex",
        "editor",
    );
    assert_eq!(status_first.label, "Review patch");
    assert_eq!(status_first.activity_label.as_deref(), Some("Review patch"));

    let status_last = classify::display_metadata(
        Some(Provider::Codex),
        None,
        None,
        "Ready | Working: codex",
        "codex",
        "editor",
    );
    assert_eq!(status_last.label, "Ready");
    assert_eq!(status_last.activity_label.as_deref(), Some("Ready"));
}

#[test]
fn non_codex_status_shaped_titles_preserve_display_label() {
    let unresolved =
        classify::display_metadata(None, None, None, "Working | deploy notes", "zsh", "editor");
    assert_eq!(unresolved.label, "Working | deploy notes");
    assert_eq!(unresolved.activity_label, None);
}

#[test]
fn display_metadata_prefers_window_name_for_metadata_only_cursor_cli() {
    let cursor = classify::display_metadata(
        Some(Provider::CursorCli),
        Some(super::ClassificationMatchKind::PaneMetadata),
        None,
        "zsh",
        "zsh",
        "agent-pane",
    );
    assert_eq!(cursor.label, "agent-pane");
    assert_eq!(cursor.activity_label, None);
}

#[test]
fn display_metadata_keeps_task_titles_for_metadata_only_cursor_cli() {
    let cursor = classify::display_metadata(
        Some(Provider::CursorCli),
        Some(super::ClassificationMatchKind::PaneMetadata),
        None,
        "Implement parser",
        "zsh",
        "agent-pane",
    );
    assert_eq!(cursor.label, "Implement parser");
    assert_eq!(cursor.activity_label.as_deref(), Some("Implement parser"));
}

#[test]
fn display_metadata_ignores_stale_prefixed_titles_for_other_providers() {
    let stale_codex = classify::display_metadata(
        Some(Provider::Claude),
        Some(super::ClassificationMatchKind::PaneCurrentCommand),
        None,
        "repo: codex",
        "claude",
        "review",
    );
    assert_eq!(stale_codex.label, "review");
    assert_eq!(stale_codex.activity_label, None);

    let stale_claude = classify::display_metadata(
        Some(Provider::Codex),
        Some(super::ClassificationMatchKind::PaneCurrentCommand),
        None,
        "Claude Code | Ready",
        "codex",
        "agent-pane",
    );
    assert_eq!(stale_claude.label, "agent-pane");
    assert_eq!(stale_claude.activity_label, None);

    let stale_opencode = classify::display_metadata(
        Some(Provider::Codex),
        Some(super::ClassificationMatchKind::PaneCurrentCommand),
        None,
        "OC | Query planner",
        "codex",
        "agent-pane",
    );
    assert_eq!(stale_opencode.label, "agent-pane");
    assert_eq!(stale_opencode.activity_label, None);

    let stale_copilot = classify::display_metadata(
        Some(Provider::Codex),
        Some(super::ClassificationMatchKind::PaneCurrentCommand),
        None,
        "Copilot | Working",
        "codex",
        "agent-pane",
    );
    assert_eq!(stale_copilot.label, "agent-pane");
    assert_eq!(stale_copilot.activity_label, None);

    let stale_cursor = classify::display_metadata(
        Some(Provider::Claude),
        Some(super::ClassificationMatchKind::PaneMetadata),
        None,
        "Cursor CLI | Query planner",
        "zsh",
        "review",
    );
    assert_eq!(stale_cursor.label, "review");
    assert_eq!(stale_cursor.activity_label, None);

    let stale_pi = classify::display_metadata(
        Some(Provider::Claude),
        Some(super::ClassificationMatchKind::PaneMetadata),
        None,
        "π - refactor - agentscan",
        "zsh",
        "review",
    );
    assert_eq!(stale_pi.label, "review");
    assert_eq!(stale_pi.activity_label, None);
}

#[test]
fn display_metadata_keeps_plain_ascii_pi_task_titles_for_non_pi_providers() {
    let plain_ascii_pi = classify::display_metadata(
        Some(Provider::Claude),
        Some(super::ClassificationMatchKind::PaneCurrentCommand),
        None,
        "pi - refactor - agentscan",
        "claude",
        "review",
    );
    assert_eq!(plain_ascii_pi.label, "pi - refactor - agentscan");
    assert_eq!(
        plain_ascii_pi.activity_label.as_deref(),
        Some("pi - refactor - agentscan")
    );
}

#[test]
fn root_list_args_parse_for_default_list_flow() {
    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "-f", "--all", "--format", "json"]);
    assert!(cli.list_args.refresh.refresh);
    assert!(cli.list_args.all);
    assert_eq!(cli.list_args.format, OutputFormat::Json);
}

#[test]
fn root_list_args_merge_into_list_like_commands() {
    let cli =
        <Cli as clap::Parser>::parse_from(["agentscan", "--all", "--format", "json", "list", "-f"]);
    match cli.command {
        Some(super::Commands::List(mut args)) => {
            super::commands::merge_list_args(&mut args, &cli.list_args);
            assert!(args.refresh.refresh);
            assert!(args.all);
            assert_eq!(args.format, OutputFormat::Json);
        }
        other => panic!("expected list command, got {other:?}"),
    }

    let cli =
        <Cli as clap::Parser>::parse_from(["agentscan", "--all", "--format", "json", "scan", "-f"]);
    match cli.command {
        Some(super::Commands::Scan(mut args)) => {
            super::commands::merge_list_args(&mut args, &cli.list_args);
            assert!(args.refresh.refresh);
            assert!(args.all);
            assert_eq!(args.format, OutputFormat::Json);
        }
        other => panic!("expected scan command, got {other:?}"),
    }
}

#[test]
fn root_list_args_merge_into_other_refresh_capable_commands() {
    let cli =
        <Cli as clap::Parser>::parse_from(["agentscan", "--format", "json", "inspect", "%1", "-f"]);
    match cli.command {
        Some(super::Commands::Inspect(mut args)) => {
            super::commands::merge_inspect_args(&mut args, &cli.list_args).unwrap();
            assert!(args.refresh.refresh);
            assert_eq!(args.format, OutputFormat::Json);
        }
        other => panic!("expected inspect command, got {other:?}"),
    }

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "-f", "focus", "%1"]);
    match cli.command {
        Some(super::Commands::Focus(mut args)) => {
            super::commands::merge_focus_args(&mut args, &cli.list_args).unwrap();
            assert!(args.refresh.refresh);
        }
        other => panic!("expected focus command, got {other:?}"),
    }

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "--all", "popup", "-f"]);
    match cli.command {
        Some(super::Commands::Popup(mut args)) => {
            super::commands::merge_popup_args(&mut args, &cli.list_args).unwrap();
            assert!(args.refresh.refresh);
            assert!(args.all);
        }
        other => panic!("expected popup command, got {other:?}"),
    }

    let cli =
        <Cli as clap::Parser>::parse_from(["agentscan", "--format", "json", "cache", "show", "-f"]);
    match cli.command {
        Some(super::Commands::Cache(args)) => match args.command {
            super::CacheCommands::Show(show_args) => {
                assert!(show_args.refresh.refresh);
                assert_eq!(cli.list_args.format, OutputFormat::Json);
            }
            other => panic!("expected cache show command, got {other:?}"),
        },
        other => panic!("expected cache command, got {other:?}"),
    }
}

#[test]
fn unsupported_root_list_args_are_rejected_for_other_commands() {
    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "--all", "daemon", "status"]);
    assert!(cli.list_args.all);
    assert!(super::commands::reject_root_all(&cli.list_args, "daemon").is_err());

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "--format", "json", "cache", "path"]);
    assert_eq!(cli.list_args.format, OutputFormat::Json);
    assert!(super::commands::reject_root_format(&cli.list_args, "cache path").is_err());

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "--format", "json", "popup"]);
    assert_eq!(cli.list_args.format, OutputFormat::Json);
    match cli.command {
        Some(super::Commands::Popup(mut args)) => {
            let error = super::commands::merge_popup_args(&mut args, &cli.list_args).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("`agentscan popup` is interactive-only"),
                "expected popup guidance, got {error:#}"
            );
            assert!(
                error.to_string().contains("`agentscan list --format json`"),
                "expected list-json migration guidance, got {error:#}"
            );
        }
        other => panic!("expected popup command, got {other:?}"),
    }

    let error = <Cli as clap::Parser>::try_parse_from(["agentscan", "popup", "--format", "json"])
        .expect_err("popup should reject local --format during clap parsing");
    assert!(
        error.to_string().contains("unexpected argument '--format'"),
        "expected clap parse error for popup-local --format, got {error:#}"
    );

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "-f", "tmux", "set-metadata"]);
    assert!(cli.list_args.refresh.refresh);
    assert!(super::commands::reject_root_refresh(&cli.list_args, "tmux set-metadata").is_err());
}

#[test]
fn tmux_set_metadata_accepts_provider_aliases() {
    for (value, expected) in [
        ("cursor_cli", Provider::CursorCli),
        ("cursor-cli", Provider::CursorCli),
        ("cursor-agent", Provider::CursorCli),
        ("copilot", Provider::Copilot),
        ("github-copilot", Provider::Copilot),
        ("pi", Provider::Pi),
        ("pi-coding-agent", Provider::Pi),
    ] {
        let cli = <Cli as clap::Parser>::parse_from([
            "agentscan",
            "tmux",
            "set-metadata",
            "--provider",
            value,
        ]);
        match cli.command {
            Some(super::Commands::Tmux(args)) => match args.command {
                super::TmuxCommands::SetMetadata(set_args) => {
                    assert_eq!(set_args.provider, Some(expected), "value: {value}");
                }
                other => panic!("expected tmux set-metadata command, got {other:?}"),
            },
            other => panic!("expected tmux command, got {other:?}"),
        }
    }
}

fn cache_path_for_test(
    override_path: Option<&str>,
    xdg_cache_home: Option<&str>,
    home: Option<&str>,
) -> Result<PathBuf, anyhow::Error> {
    if let Some(path) = override_path {
        return Ok(PathBuf::from(path));
    }

    if let Some(cache_home) = xdg_cache_home {
        return Ok(PathBuf::from(cache_home).join(CACHE_RELATIVE_PATH));
    }

    let home = home.context("missing home")?;
    Ok(Path::new(home).join(".cache").join(CACHE_RELATIVE_PATH))
}

fn proc_fallback_pane(pid: u32, command: &str, title: &str) -> PaneRecord {
    classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: format!("%{pid}"),
        pane_pid: pid,
        pane_current_command: command.to_string(),
        pane_title_raw: title.to_string(),
        pane_tty: format!("/dev/pts/{pid}"),
        pane_current_path: "/tmp/proc-wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    })
}

struct FakeProcessInspector {
    processes_by_pid: std::collections::HashMap<u32, Vec<proc::ProcessEvidence>>,
    foreground_by_tty: std::collections::HashMap<String, Vec<proc::ProcessEvidence>>,
    calls: RefCell<Vec<u32>>,
    foreground_calls: RefCell<Vec<String>>,
}

impl FakeProcessInspector {
    fn new(entries: impl IntoIterator<Item = (u32, Vec<String>)>) -> Self {
        Self {
            processes_by_pid: entries
                .into_iter()
                .map(|(pid, commands)| {
                    (
                        pid,
                        commands
                            .into_iter()
                            .map(|command| proc::ProcessEvidence {
                                pid,
                                command: command.clone(),
                                argv: vec![command],
                                env: Vec::new(),
                            })
                            .collect(),
                    )
                })
                .collect(),
            foreground_by_tty: std::collections::HashMap::new(),
            calls: RefCell::new(Vec::new()),
            foreground_calls: RefCell::new(Vec::new()),
        }
    }

    fn with_processes(
        entries: impl IntoIterator<Item = (u32, Vec<proc::ProcessEvidence>)>,
    ) -> Self {
        Self {
            processes_by_pid: entries.into_iter().collect(),
            foreground_by_tty: std::collections::HashMap::new(),
            calls: RefCell::new(Vec::new()),
            foreground_calls: RefCell::new(Vec::new()),
        }
    }

    fn with_foreground(
        descendants: impl IntoIterator<Item = (u32, Vec<String>)>,
        foreground: impl IntoIterator<Item = (String, Vec<String>)>,
    ) -> Self {
        let mut inspector = Self::new(descendants);
        inspector.foreground_by_tty = foreground
            .into_iter()
            .map(|(tty, commands)| {
                (
                    tty,
                    commands
                        .into_iter()
                        .map(|command| proc::ProcessEvidence {
                            pid: 0,
                            command: command.clone(),
                            argv: vec![command],
                            env: Vec::new(),
                        })
                        .collect(),
                )
            })
            .collect();
        inspector
    }

    fn calls(&self) -> Vec<u32> {
        self.calls.borrow().clone()
    }

    fn foreground_calls(&self) -> Vec<String> {
        self.foreground_calls.borrow().clone()
    }
}

impl proc::ProcessInspector for FakeProcessInspector {
    fn descendant_processes(&self, root_pid: u32) -> anyhow::Result<Vec<proc::ProcessEvidence>> {
        self.calls.borrow_mut().push(root_pid);
        Ok(self
            .processes_by_pid
            .get(&root_pid)
            .cloned()
            .unwrap_or_default())
    }

    fn foreground_processes(&self, pane_tty: &str) -> anyhow::Result<Vec<proc::ProcessEvidence>> {
        self.foreground_calls
            .borrow_mut()
            .push(pane_tty.to_string());
        Ok(self
            .foreground_by_tty
            .get(pane_tty)
            .cloned()
            .unwrap_or_default())
    }
}

fn assert_fixture_codex_cases(panes: &[PaneRecord]) {
    let codex_plain_working = pane_by_id(panes, "%178");
    assert_eq!(codex_plain_working.provider, Some(Provider::Codex));
    assert_eq!(codex_plain_working.status.kind, StatusKind::Busy);
    assert_eq!(codex_plain_working.display.label, "Working");
    assert_eq!(codex_plain_working.display.activity_label, None);

    let codex_working = pane_by_id(panes, "%191");
    assert_eq!(codex_working.provider, Some(Provider::Codex));
    assert_eq!(codex_working.status.kind, StatusKind::Busy);
    assert_eq!(codex_working.display.label, "agentscan");
    assert_eq!(
        codex_working.display.activity_label.as_deref(),
        Some("agentscan")
    );

    let codex_ready = pane_by_id(panes, "%67");
    assert_eq!(codex_ready.status.kind, StatusKind::Idle);
    assert_eq!(codex_ready.display.activity_label, None);

    let codex_waiting = pane_by_id(panes, "%194");
    assert_eq!(codex_waiting.provider, Some(Provider::Codex));
    assert_eq!(codex_waiting.status.kind, StatusKind::Busy);
    assert_eq!(codex_waiting.display.label, "agentscan");
    assert_eq!(
        codex_waiting.display.activity_label.as_deref(),
        Some("agentscan")
    );
}

fn assert_fixture_claude_cases(panes: &[PaneRecord]) {
    let claude_idle = pane_by_id(panes, "%41");
    assert_eq!(claude_idle.provider, Some(Provider::Claude));
    assert_eq!(claude_idle.status.kind, StatusKind::Idle);
    assert_eq!(claude_idle.display.label, "Review and summarize todo list");
    assert_eq!(
        claude_idle.display.activity_label.as_deref(),
        Some("Review and summarize todo list")
    );

    let claude_busy = pane_by_id(panes, "%223");
    assert_eq!(claude_busy.status.kind, StatusKind::Busy);

    let claude_title_busy = pane_by_id(panes, "%224");
    assert_eq!(claude_title_busy.provider, Some(Provider::Claude));
    assert_eq!(claude_title_busy.status.kind, StatusKind::Busy);
    assert_eq!(claude_title_busy.display.label, "Working");
    assert_eq!(claude_title_busy.display.activity_label, None);

    let claude_title_idle = pane_by_id(panes, "%225");
    assert_eq!(claude_title_idle.provider, Some(Provider::Claude));
    assert_eq!(claude_title_idle.status.kind, StatusKind::Idle);
    assert_eq!(claude_title_idle.display.label, "Ready");
    assert_eq!(claude_title_idle.display.activity_label, None);

    let claude_task_idle = pane_by_id(panes, "%275");
    assert_eq!(claude_task_idle.provider, Some(Provider::Claude));
    assert_eq!(claude_task_idle.status.kind, StatusKind::Idle);
    assert_eq!(
        claude_task_idle.display.label,
        "Design GitHub bot with Claude agent dashboard"
    );
    assert_eq!(
        claude_task_idle.display.activity_label.as_deref(),
        Some("Design GitHub bot with Claude agent dashboard")
    );
}

fn assert_fixture_gemini_cases(panes: &[PaneRecord]) {
    let gemini_idle = pane_by_id(panes, "%300");
    assert_eq!(gemini_idle.provider, Some(Provider::Gemini));
    assert_eq!(gemini_idle.status.kind, StatusKind::Idle);
    assert_eq!(gemini_idle.status.source, super::StatusSource::TmuxTitle);
    assert_eq!(gemini_idle.display.label, "Ready");
    assert_eq!(gemini_idle.display.activity_label, None);
    assert_eq!(
        gemini_idle.classification.matched_by,
        Some(super::ClassificationMatchKind::PaneCurrentCommand)
    );

    let gemini_busy = pane_by_id(panes, "%306");
    assert_eq!(gemini_busy.provider, Some(Provider::Gemini));
    assert_eq!(gemini_busy.status.kind, StatusKind::Busy);
    assert_eq!(gemini_busy.status.source, super::StatusSource::TmuxTitle);
    assert_eq!(gemini_busy.display.label, "Working");
    assert_eq!(gemini_busy.display.activity_label, None);

    let gemini_task = pane_by_id(panes, "%307");
    assert_eq!(gemini_task.provider, Some(Provider::Gemini));
    assert_eq!(gemini_task.status.kind, StatusKind::Unknown);
    assert_eq!(gemini_task.status.source, super::StatusSource::NotChecked);
    assert_eq!(gemini_task.display.label, "Plan snapshot cache migration");
    assert_eq!(
        gemini_task.display.activity_label.as_deref(),
        Some("Plan snapshot cache migration")
    );
}

fn assert_fixture_opencode_case(panes: &[PaneRecord]) {
    let opencode = pane_by_id(panes, "%301");
    assert_eq!(opencode.provider, Some(Provider::Opencode));
    assert_eq!(opencode.status.kind, StatusKind::Unknown);
    assert_eq!(opencode.status.source, super::StatusSource::NotChecked);
    assert_eq!(opencode.display.label, "Query planner");
    assert_eq!(opencode.display.activity_label, None);

    let opencode_busy = pane_by_id(panes, "%308");
    assert_eq!(opencode_busy.provider, Some(Provider::Opencode));
    assert_eq!(opencode_busy.status.kind, StatusKind::Unknown);
    assert_eq!(opencode_busy.status.source, super::StatusSource::NotChecked);
    assert_eq!(opencode_busy.display.label, "Working");
    assert_eq!(opencode_busy.display.activity_label, None);

    let opencode_idle = pane_by_id(panes, "%309");
    assert_eq!(opencode_idle.provider, Some(Provider::Opencode));
    assert_eq!(opencode_idle.status.kind, StatusKind::Unknown);
    assert_eq!(opencode_idle.status.source, super::StatusSource::NotChecked);
    assert_eq!(opencode_idle.display.label, "Ready");
    assert_eq!(opencode_idle.display.activity_label, None);

    let opencode_default = pane_by_id(panes, "%314");
    assert_eq!(opencode_default.provider, Some(Provider::Opencode));
    assert_eq!(opencode_default.status.kind, StatusKind::Unknown);
    assert_eq!(
        opencode_default.status.source,
        super::StatusSource::NotChecked
    );
    assert_eq!(opencode_default.display.label, "OpenCode");
    assert_eq!(opencode_default.display.activity_label, None);
}

fn assert_fixture_copilot_case(panes: &[PaneRecord]) {
    let copilot = pane_by_id(panes, "%302");
    assert_eq!(copilot.provider, Some(Provider::Copilot));
    assert_eq!(copilot.status.kind, StatusKind::Busy);
    assert_eq!(copilot.status.source, super::StatusSource::TmuxTitle);
    assert_eq!(copilot.display.label, "Working");
    assert_eq!(copilot.display.activity_label, None);

    let copilot_idle = pane_by_id(panes, "%310");
    assert_eq!(copilot_idle.provider, Some(Provider::Copilot));
    assert_eq!(copilot_idle.status.kind, StatusKind::Idle);
    assert_eq!(copilot_idle.status.source, super::StatusSource::TmuxTitle);
    assert_eq!(copilot_idle.display.label, "Ready");
    assert_eq!(copilot_idle.display.activity_label, None);
    assert_eq!(
        copilot_idle.classification.matched_by,
        Some(super::ClassificationMatchKind::PaneTitle)
    );

    let copilot_task = pane_by_id(panes, "%311");
    assert_eq!(copilot_task.provider, Some(Provider::Copilot));
    assert_eq!(copilot_task.status.kind, StatusKind::Unknown);
    assert_eq!(copilot_task.status.source, super::StatusSource::NotChecked);
    assert_eq!(copilot_task.display.label, "Review patch");
    assert_eq!(
        copilot_task.display.activity_label.as_deref(),
        Some("Review patch")
    );
}

fn assert_fixture_cursor_cli_title_case(panes: &[PaneRecord]) {
    let cursor = pane_by_id(panes, "%303");
    assert_eq!(cursor.provider, Some(Provider::CursorCli));
    assert_eq!(cursor.display.label, "Query planner");
    assert_eq!(cursor.status.kind, StatusKind::Unknown);
    assert_eq!(cursor.status.source, super::StatusSource::NotChecked);
    assert_eq!(
        cursor.display.activity_label.as_deref(),
        Some("Query planner")
    );
}

fn assert_fixture_cursor_cli_command_case(panes: &[PaneRecord]) {
    let cursor = pane_by_id(panes, "%305");
    assert_eq!(cursor.provider, Some(Provider::CursorCli));
    assert_eq!(cursor.display.label, "cursor-agent");
    assert_eq!(cursor.status.kind, StatusKind::Unknown);
    assert_eq!(cursor.display.activity_label, None);
    assert_eq!(
        cursor.classification.matched_by,
        Some(super::ClassificationMatchKind::PaneCurrentCommand)
    );
}

fn assert_fixture_pi_case(panes: &[PaneRecord]) {
    let pi = pane_by_id(panes, "%304");
    assert_eq!(pi.provider, Some(Provider::Pi));
    assert_eq!(pi.display.label, "refactor - pi_proj");
    assert_eq!(pi.status.kind, StatusKind::Unknown);
    assert_eq!(pi.status.source, super::StatusSource::NotChecked);
    assert_eq!(pi.display.activity_label, None);

    let pi_busy = pane_by_id(panes, "%312");
    assert_eq!(pi_busy.provider, Some(Provider::Pi));
    assert_eq!(pi_busy.status.kind, StatusKind::Busy);
    assert_eq!(pi_busy.status.source, super::StatusSource::TmuxTitle);
    assert_eq!(pi_busy.display.label, "refactor - pi_proj");
    assert_eq!(pi_busy.display.activity_label, None);

    let pi_command = pane_by_id(panes, "%313");
    assert_eq!(pi_command.provider, Some(Provider::Pi));
    assert_eq!(pi_command.status.kind, StatusKind::Unknown);
    assert_eq!(pi_command.status.source, super::StatusSource::NotChecked);
    assert_eq!(pi_command.display.label, "ship cache docs - followup");
    assert_eq!(
        pi_command.display.activity_label.as_deref(),
        Some("ship cache docs - followup")
    );
    assert_eq!(
        pi_command.classification.matched_by,
        Some(super::ClassificationMatchKind::PaneCurrentCommand)
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

fn safe_tmux_field() -> impl Strategy<Value = String> {
    string_regex(r"[A-Za-z0-9_./()|: -]{0,32}").expect("safe tmux field regex should compile")
}

fn known_status_glyph() -> impl Strategy<Value = char> {
    prop::sample::select(
        CLAUDE_SPINNER_GLYPHS
            .iter()
            .copied()
            .chain(IDLE_GLYPHS.iter().copied())
            .collect::<Vec<_>>(),
    )
}

fn pane_by_id<'a>(panes: &'a [PaneRecord], pane_id: &str) -> &'a PaneRecord {
    panes
        .iter()
        .find(|pane| pane.pane_id == pane_id)
        .unwrap_or_else(|| panic!("missing pane fixture entry {pane_id}"))
}

fn assert_unresolved_ambiguous_pane(pane: &PaneRecord, expected_label: &str) {
    assert_eq!(pane.provider, None, "pane_id: {}", pane.pane_id);
    assert_eq!(
        pane.status.kind,
        StatusKind::Unknown,
        "pane_id: {}",
        pane.pane_id
    );
    assert_eq!(
        pane.status.source,
        super::StatusSource::NotChecked,
        "pane_id: {}",
        pane.pane_id
    );
    assert_eq!(
        pane.classification.matched_by, None,
        "pane_id: {}",
        pane.pane_id
    );
    assert_eq!(
        pane.classification.confidence, None,
        "pane_id: {}",
        pane.pane_id
    );
    assert!(
        pane.classification.reasons.is_empty(),
        "pane_id: {}",
        pane.pane_id
    );
    assert_eq!(
        pane.display.label, expected_label,
        "pane_id: {}",
        pane.pane_id
    );
    assert_eq!(
        pane.display.activity_label, None,
        "pane_id: {}",
        pane.pane_id
    );
}
