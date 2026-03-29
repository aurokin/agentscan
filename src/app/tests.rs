use std::path::{Path, PathBuf};

use anyhow::Context;
use proptest::{prelude::*, string::string_regex};

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

#[allow(unused_imports)]
use super::{
    CACHE_RELATIVE_PATH, CACHE_SCHEMA_VERSION, CLAUDE_SPINNER_GLYPHS, Cli,
    DAEMON_SUBSCRIPTION_FORMAT, IDLE_GLYPHS, PaneRecord, Provider, SnapshotEnvelope, SourceKind,
    StatusKind, TmuxMetadataField, cache, classify, daemon, output, tmux,
};

#[test]
fn classifies_from_command() {
    let matched = classify::classify_provider(None, "codex", "").expect("should match codex");
    assert_eq!(matched.provider, Provider::Codex);
    assert_eq!(
        matched.matched_by,
        super::ClassificationMatchKind::PaneCurrentCommand
    );
}

#[test]
fn classifies_from_title_before_command() {
    let matched = classify::classify_provider(None, "zsh", "Claude Code | Working")
        .expect("should match claude");
    assert_eq!(matched.provider, Provider::Claude);
    assert_eq!(
        matched.matched_by,
        super::ClassificationMatchKind::PaneTitle
    );
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
fn daemon_subscription_format_includes_wrapper_metadata_fields() {
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
fn infers_status_from_title_only() {
    let status = classify::infer_title_status(Some(Provider::Gemini), "Working");
    assert_eq!(status.kind, StatusKind::Busy);
}

#[test]
fn codex_status_uses_title_only() {
    let busy = classify::infer_title_status(Some(Provider::Codex), "⠹ agentscan | Working");
    let idle = classify::infer_title_status(Some(Provider::Codex), "Ready");

    assert_eq!(busy.kind, StatusKind::Busy);
    assert_eq!(idle.kind, StatusKind::Idle);
}

#[test]
fn claude_status_distinguishes_spinner_and_idle_marker() {
    let busy = classify::infer_title_status(Some(Provider::Claude), "⠏ Building summary");
    let idle =
        classify::infer_title_status(Some(Provider::Claude), "✳ Review and summarize todo list");

    assert_eq!(busy.kind, StatusKind::Busy);
    assert_eq!(idle.kind, StatusKind::Idle);
}

#[test]
fn claude_status_uses_textual_titles_without_spinner_glyphs() {
    let busy = classify::infer_title_status(Some(Provider::Claude), "Claude Code | Working");
    let idle = classify::infer_title_status(Some(Provider::Claude), "Claude Code | Ready");
    let unknown = classify::infer_title_status(Some(Provider::Claude), "Claude Code | Query");

    assert_eq!(busy.kind, StatusKind::Busy);
    assert_eq!(idle.kind, StatusKind::Idle);
    assert_eq!(unknown.kind, StatusKind::Unknown);
}

#[test]
fn opencode_status_uses_title_prefix_when_present() {
    let busy = classify::infer_title_status(Some(Provider::Opencode), "OC | Working");
    let idle = classify::infer_title_status(Some(Provider::Opencode), "OC | Ready");
    let unknown = classify::infer_title_status(Some(Provider::Opencode), "OC | Query planner");

    assert_eq!(busy.kind, StatusKind::Busy);
    assert_eq!(idle.kind, StatusKind::Idle);
    assert_eq!(unknown.kind, StatusKind::Unknown);
}

#[test]
fn metadata_state_fills_unknown_status_without_overriding_title_signal() {
    let unknown_from_title =
        classify::infer_title_status(Some(Provider::Codex), "(bront) repo: codex");
    let busy_from_metadata = classify::infer_status(unknown_from_title, Some("busy"));
    assert_eq!(busy_from_metadata.kind, StatusKind::Busy);

    let idle_from_title = classify::infer_title_status(Some(Provider::Codex), "Ready");
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
        cache::daemon_cache_status(Some(10), Some(60)),
        super::DaemonCacheStatus::Healthy
    );
    assert_eq!(
        super::daemon_cache_status_name(super::DaemonCacheStatus::Healthy),
        "healthy"
    );

    snapshot.source.kind = SourceKind::Snapshot;
    snapshot.source.daemon_generated_at = None;
    assert_eq!(
        cache::daemon_cache_status(None, Some(60)),
        super::DaemonCacheStatus::Unavailable
    );

    snapshot.source.daemon_generated_at = Some("2026-03-28T00:00:00Z".to_string());
    assert_eq!(
        cache::daemon_cache_status(Some(120), Some(60)),
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
        cache::daemon_cache_status(Some(10), Some(60)),
        super::DaemonCacheStatus::Healthy
    );
}

#[test]
fn popup_entries_include_location_and_status() {
    let pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "notes".to_string(),
        window_index: 4,
        pane_index: 1,
        pane_id: "%41".to_string(),
        pane_pid: 324026,
        pane_current_command: "claude".to_string(),
        pane_title_raw: "Working".to_string(),
        pane_tty: "/dev/pts/44".to_string(),
        pane_current_path: "/home/auro/notes".to_string(),
        window_name: "ai".to_string(),
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });

    let entries = cache::popup_entries(&[pane]);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].location_tag, "notes:4.1");
    assert_eq!(entries[0].session_name, "notes");
}

#[test]
fn fixture_snapshot_parses_expected_provider_cases() {
    let rows = tmux::parse_pane_rows(TMUX_SNAPSHOT_FIXTURE).expect("fixture snapshot should parse");
    let panes: Vec<_> = rows.into_iter().map(classify::pane_from_row).collect();

    assert_fixture_codex_cases(&panes);
    assert_fixture_claude_cases(&panes);
    assert_fixture_opencode_case(&panes);
}

#[test]
fn fixture_snapshot_preserves_wrapper_prefixes() {
    let rows = tmux::parse_pane_rows(TMUX_SNAPSHOT_FIXTURE).expect("fixture snapshot should parse");
    let panes: Vec<_> = rows.into_iter().map(classify::pane_from_row).collect();

    let wrapped_codex = pane_by_id(&panes, "%89");
    assert_eq!(wrapped_codex.provider, Some(Provider::Codex));
    assert_eq!(wrapped_codex.display.label, "(bront) parallel-n64");
    assert_eq!(wrapped_codex.status.kind, StatusKind::Unknown);
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
        agent_provider: Some("claude".to_string()),
        agent_label: Some("Wrapper Claude Task".to_string()),
        agent_cwd: Some("/tmp/wrapper".to_string()),
        agent_state: Some("idle".to_string()),
        agent_session_id: Some("sess-123".to_string()),
    });

    assert_eq!(pane.provider, Some(Provider::Claude));
    assert_eq!(pane.display.label, "Wrapper Claude Task");
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
    snapshot.schema_version = CACHE_SCHEMA_VERSION + 1;

    let error = cache::validate_snapshot(&snapshot, None).expect_err("schema mismatch should fail");
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
fn tsv_escape_removes_control_whitespace() {
    assert_eq!(output::tsv_escape("a\tb\nc\rd"), "a b c d");
}

#[test]
fn status_names_match_serialized_values() {
    assert_eq!(super::status_kind_name(StatusKind::Busy), "busy");
    assert_eq!(super::status_kind_name(StatusKind::Idle), "idle");
    assert_eq!(super::status_kind_name(StatusKind::Unknown), "unknown");
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
        classify::normalize_title_for_display("Claude Code | Query"),
        "Query"
    );
    assert_eq!(
        classify::normalize_title_for_display("Claude | Ready"),
        "Ready"
    );
    assert_eq!(
        classify::normalize_title_for_display("OC | Query planner"),
        "Query planner"
    );
    assert_eq!(
        classify::normalize_title_for_display(
            "(bront) parallel-n64: /home/auro/.zshrc.d/scripts/lgpt.sh"
        ),
        "(bront) parallel-n64"
    );
    assert_eq!(
        classify::normalize_title_for_display("(repo) task: codex --model gpt-5"),
        "(repo) task"
    );
}

#[test]
fn display_metadata_extracts_activity_labels_from_titles() {
    let codex = classify::display_metadata(
        Some(Provider::Codex),
        None,
        "⠹ agentscan | Working",
        "codex",
        "editor",
    );
    assert_eq!(codex.label, "agentscan | Working");
    assert_eq!(codex.activity_label.as_deref(), Some("agentscan"));

    let claude = classify::display_metadata(
        Some(Provider::Claude),
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

    let generic =
        classify::display_metadata(Some(Provider::Codex), None, "Ready", "codex", "editor");
    assert_eq!(generic.label, "Ready");
    assert_eq!(generic.activity_label, None);
}

#[test]
fn cli_refresh_flag_is_global() {
    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "-f", "list"]);
    assert!(cli.refresh);

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "list", "-f"]);
    assert!(cli.refresh);

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "-f"]);
    assert!(cli.refresh);
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

fn assert_fixture_codex_cases(panes: &[PaneRecord]) {
    let codex_working = pane_by_id(panes, "%191");
    assert_eq!(codex_working.provider, Some(Provider::Codex));
    assert_eq!(codex_working.status.kind, StatusKind::Busy);
    assert_eq!(codex_working.display.label, "agentscan | Working");
    assert_eq!(
        codex_working.display.activity_label.as_deref(),
        Some("agentscan")
    );

    let codex_ready = pane_by_id(panes, "%67");
    assert_eq!(codex_ready.status.kind, StatusKind::Idle);
    assert_eq!(codex_ready.display.activity_label, None);

    let codex_waiting = pane_by_id(panes, "%194");
    assert_eq!(codex_waiting.provider, Some(Provider::Codex));
    assert_eq!(codex_waiting.status.kind, StatusKind::Idle);
    assert_eq!(codex_waiting.display.label, "agentscan | Waiting");
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
}

fn assert_fixture_opencode_case(panes: &[PaneRecord]) {
    let opencode = pane_by_id(panes, "%301");
    assert_eq!(opencode.provider, Some(Provider::Opencode));
    assert_eq!(opencode.display.label, "Query planner");
    assert_eq!(
        opencode.display.activity_label.as_deref(),
        Some("Query planner")
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
    fn tsv_escape_is_idempotent_and_removes_control_whitespace(value in any::<String>()) {
        let escaped = output::tsv_escape(&value);

        prop_assert!(!escaped.contains('\t'));
        prop_assert!(!escaped.contains('\n'));
        prop_assert!(!escaped.contains('\r'));
        prop_assert_eq!(output::tsv_escape(&escaped), escaped);
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
