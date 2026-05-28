#[test]
fn list_json_exposes_the_machine_readable_pane_fields() {
    let pane = tmux_pane_row(324026)
        .session_name("notes")
        .window_index(4)
        .pane_id("%41")
        .command("claude")
        .title("Claude Code | Query")
        .tty("/dev/pts/44")
        .current_path("/home/auro/notes")
        .session_id("$7")
        .window_id("@9")
        .pane();
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
fn source_kind_supports_daemon() {
    assert_eq!(
        serde_json::to_string(&SourceKind::Daemon).unwrap(),
        "\"daemon\""
    );
}
