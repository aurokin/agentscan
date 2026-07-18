fn populated_contract_snapshot() -> SnapshotEnvelope {
    let mut pane = tmux_pane_row(324026)
        .session_name("notes")
        .window_index(4)
        .pane_index(2)
        .pane_id("%41")
        .command("claude")
        .title("Claude Code | Query")
        .tty("/dev/pts/44")
        .current_path("/home/auro/notes")
        .window_name("agents")
        .session_id("$7")
        .window_id("@9")
        .agent_provider("claude")
        .agent_label("Contract Query")
        .agent_cwd("/home/auro/notes")
        .pane_active(true)
        .window_active(true)
        .pane();

    pane.provider = Some(Provider::Claude);
    pane.status = PaneStatus::new(StatusKind::Waiting, super::StatusSource::PaneMetadata);
    pane.display.label = "Contract Query".to_string();
    pane.display.activity_label = Some("Query".to_string());
    pane.classification.matched_by = Some(super::ClassificationMatchKind::PaneMetadata);
    pane.classification.confidence = Some(super::ClassificationConfidence::High);
    pane.classification.reasons = vec!["explicit contract metadata".to_string()];
    pane.agent_metadata.provider = Some("claude".to_string());
    pane.agent_metadata.label = Some("Contract Query".to_string());
    pane.agent_metadata.cwd = Some("/home/auro/notes".to_string());
    pane.agent_metadata.state = Some("waiting".to_string());
    pane.agent_metadata.session_id = Some("agent-session-123".to_string());
    pane.diagnostics.cache_origin = "contract_fixture".to_string();
    pane.diagnostics.proc_fallback = super::ProcFallbackDiagnostics {
        outcome: ProcFallbackOutcome::Resolved,
        reason: "fixed contract evidence".to_string(),
        commands: vec!["claude --resume agent-session-123".to_string()],
    };
    pane.last_focus_seq = Some(9);

    SnapshotEnvelope {
        schema_version: CACHE_SCHEMA_VERSION,
        generated_at: "2026-04-15T00:00:00Z".to_string(),
        source: super::SnapshotSource {
            kind: SourceKind::Daemon,
            tmux_version: Some("3.4".to_string()),
            daemon_generated_at: Some("2026-04-15T00:00:00Z".to_string()),
        },
        panes: vec![pane],
    }
}

fn contract_picker_rows(snapshot: &SnapshotEnvelope) -> Vec<super::picker::PickerRow> {
    super::picker::picker_rows(
        &snapshot.panes,
        Some("notes"),
        2,
        super::picker::PickerGroupBy::Session,
        &super::picker::PickerKeySet::default(),
    )
}

fn assert_contract_fixture(name: &str, value: &impl serde::Serialize) {
    let mut actual = serde_json::to_string_pretty(value).expect("contract value should serialize");
    actual.push('\n');

    let fixture_path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/contract"
    ))
    .join(name);

    if std::env::var_os("UPDATE_CONTRACT_FIXTURES").as_deref() == Some(std::ffi::OsStr::new("1"))
    {
        fs::write(&fixture_path, actual).unwrap_or_else(|error| {
            panic!(
                "failed to update contract fixture {}: {error}",
                fixture_path.display()
            )
        });
        return;
    }

    let expected = fs::read_to_string(&fixture_path).unwrap_or_else(|error| {
        panic!(
            "failed to read contract fixture {}: {error}",
            fixture_path.display()
        )
    });
    assert_eq!(
        expected, actual,
        "contract fixture {name} is out of date. The machine-readable contract changed: bump the relevant schema_version and regenerate deliberately with \
         UPDATE_CONTRACT_FIXTURES=1 cargo test.\n--- expected\n{expected}+++ actual\n{actual}"
    );
}

#[test]
fn machine_readable_contract_matches_golden_fixtures() {
    let snapshot = populated_contract_snapshot();
    let rows = contract_picker_rows(&snapshot);

    assert_contract_fixture("snapshot_v1.json", &snapshot);
    assert_contract_fixture(
        "hotkeys_v1.json",
        &output::PickerRowsEnvelope {
            schema_version: super::PICKER_ROWS_SCHEMA_VERSION,
            rows: &rows,
        },
    );
    assert_contract_fixture(
        "subscribe_connecting_v1.json",
        &LiveClientEvent::Connecting {
            message: "connecting to agentscan daemon".to_string(),
        },
    );
    assert_contract_fixture(
        "subscribe_snapshot_v1.json",
        &LiveClientEvent::Snapshot {
            snapshot: Box::new(snapshot),
            rows,
        },
    );
    assert_contract_fixture(
        "subscribe_offline_v1.json",
        &LiveClientEvent::Offline {
            message: "agentscan daemon is offline".to_string(),
            retrying: true,
        },
    );
    assert_contract_fixture(
        "subscribe_shutdown_v1.json",
        &LiveClientEvent::Shutdown {
            message: "agentscan daemon shut down".to_string(),
        },
    );
    assert_contract_fixture(
        "subscribe_fatal_v1.json",
        &LiveClientEvent::Fatal {
            message: "agentscan daemon protocol error".to_string(),
        },
    );
}
