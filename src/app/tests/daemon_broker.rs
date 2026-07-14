fn broker_frame_id(command_number: &str) -> daemon::ControlModeCommandFrameId {
    daemon::ControlModeCommandFrameId {
        timestamp: "1777830000".to_string(),
        command_number: command_number.to_string(),
        flags: "0".to_string(),
    }
}

fn broker_pane_row_line(pane_id: &str) -> String {
    broker_pane_row_line_with_session("session", pane_id)
}

fn broker_pane_row_line_with_session(session_name: &str, pane_id: &str) -> String {
    [
        session_name,
        "1",
        "0",
        pane_id,
        "42000",
        "codex",
        "Codex",
        "/dev/ttys001",
        "/tmp/agentscan",
        "editor",
        "$1",
        "@1",
        "codex",
        "Codex",
        "/tmp/agentscan",
        "idle",
        "session-1",
    ]
    .join("\u{1f}")
}

#[test]
fn daemon_broker_transcript_collects_matching_command_response() {
    let expected_id = broker_frame_id("201");
    let mut harness = daemon::ControlModeBrokerTranscriptHarness::new([
        daemon::ControlModeBrokerTranscriptStep::line("%subscription-changed agentscan $1 @1 0 %1 : %1:Codex:codex::::"),
        daemon::ControlModeBrokerTranscriptStep::line("%begin 1777830000 201 0"),
        daemon::ControlModeBrokerTranscriptStep::line(
            "s\u{1f}0\u{1f}0\u{1f}%1\u{1f}100\u{1f}codex\u{1f}Codex",
        ),
        daemon::ControlModeBrokerTranscriptStep::line("%end 1777830000 201 0"),
    ]);

    let response = harness
        .collect_command_response(&expected_id)
        .expect("broker transcript should collect matching response");

    assert_eq!(
        response.deferred_events,
        vec!["%subscription-changed agentscan $1 @1 0 %1 : %1:Codex:codex::::"]
    );
    assert_eq!(
        response.output,
        vec!["s\u{1f}0\u{1f}0\u{1f}%1\u{1f}100\u{1f}codex\u{1f}Codex"]
    );
}

#[test]
fn daemon_broker_list_pane_records_command_and_parses_response() {
    let expected_id = broker_frame_id("208");
    let mut harness = daemon::ControlModeBrokerTranscriptHarness::new([
        daemon::ControlModeBrokerTranscriptStep::line("%subscription-changed agentscan $1 @1 0 %1 : %1:Codex:codex::::"),
        daemon::ControlModeBrokerTranscriptStep::line("%begin 1777830000 208 0"),
        daemon::ControlModeBrokerTranscriptStep::line(broker_pane_row_line("%1")),
        daemon::ControlModeBrokerTranscriptStep::line("%end 1777830000 208 0"),
    ]);

    let response = harness
        .list_pane("%1", &expected_id)
        .expect("list-pane response should parse");

    assert_eq!(
        harness.written_commands(),
        &[format!("list-panes -t %1 -F {}", PANE_FORMAT)]
    );
    assert_eq!(response.pane.expect("pane should exist").pane_id, "%1");
    assert_eq!(
        response.deferred_events,
        vec!["%subscription-changed agentscan $1 @1 0 %1 : %1:Codex:codex::::"]
    );
}

#[test]
fn daemon_broker_list_target_records_command_and_parses_rows() {
    let expected_id = broker_frame_id("213");
    let mut harness = daemon::ControlModeBrokerTranscriptHarness::new([
        daemon::ControlModeBrokerTranscriptStep::line("%window-pane-changed @1 %2"),
        daemon::ControlModeBrokerTranscriptStep::line("%begin 1777830000 213 0"),
        daemon::ControlModeBrokerTranscriptStep::line(broker_pane_row_line("%1")),
        daemon::ControlModeBrokerTranscriptStep::line(broker_pane_row_line("%2")),
        daemon::ControlModeBrokerTranscriptStep::line("%end 1777830000 213 0"),
    ]);

    let response = harness
        .list_target_panes(tmux::PaneListScope::Window, "@1", &expected_id)
        .expect("list-target response should parse");

    assert_eq!(
        harness.written_commands(),
        &[format!("list-panes -t @1 -F {}", PANE_FORMAT)]
    );
    let pane_ids: Vec<_> = response
        .rows
        .expect("target should exist")
        .into_iter()
        .map(|row| row.pane_id)
        .collect();
    assert_eq!(pane_ids, vec!["%1", "%2"]);
    assert_eq!(
        response.deferred_events,
        vec!["%window-pane-changed @1 %2"]
    );
}

#[test]
fn daemon_broker_list_target_maps_missing_session_to_none() {
    let expected_id = broker_frame_id("214");
    let mut harness = daemon::ControlModeBrokerTranscriptHarness::new([
        daemon::ControlModeBrokerTranscriptStep::line("%session-window-changed $1 @1"),
        daemon::ControlModeBrokerTranscriptStep::line("%begin 1777830000 214 0"),
        daemon::ControlModeBrokerTranscriptStep::line("can't find session: $404"),
        daemon::ControlModeBrokerTranscriptStep::line("%error 1777830000 214 0"),
    ]);

    let response = harness
        .list_target_panes(tmux::PaneListScope::Session, "$404", &expected_id)
        .expect("missing session should not fail");

    assert_eq!(
        harness.written_commands(),
        &[format!("list-panes -s -t $404 -F {}", PANE_FORMAT)]
    );
    assert!(response.rows.is_none());
    assert_eq!(
        response.deferred_events,
        vec!["%session-window-changed $1 @1"]
    );
}

#[test]
fn daemon_broker_list_all_records_command_and_parses_rows() {
    let expected_id = broker_frame_id("215");
    let mut harness = daemon::ControlModeBrokerTranscriptHarness::new([
        daemon::ControlModeBrokerTranscriptStep::line("%session-changed $1"),
        daemon::ControlModeBrokerTranscriptStep::line("%begin 1777830000 215 0"),
        daemon::ControlModeBrokerTranscriptStep::line(broker_pane_row_line("%1")),
        daemon::ControlModeBrokerTranscriptStep::line(broker_pane_row_line("%2")),
        daemon::ControlModeBrokerTranscriptStep::line("%end 1777830000 215 0"),
    ]);

    let response = harness
        .list_all_panes(&expected_id)
        .expect("list-all response should parse");

    assert_eq!(
        harness.written_commands(),
        &[format!("list-panes -a -F {}", PANE_FORMAT)]
    );
    let pane_ids: Vec<_> = response.rows.into_iter().map(|row| row.pane_id).collect();
    assert_eq!(pane_ids, vec!["%1", "%2"]);
    assert_eq!(response.deferred_events, vec!["%session-changed $1"]);
}

#[test]
fn daemon_broker_list_all_preserves_unexpected_errors() {
    let expected_id = broker_frame_id("216");
    let mut harness = daemon::ControlModeBrokerTranscriptHarness::new([
        daemon::ControlModeBrokerTranscriptStep::line("%begin 1777830000 216 0"),
        daemon::ControlModeBrokerTranscriptStep::line(
            "%error 1777830000 216 0 invalid format string",
        ),
    ]);

    let error = harness
        .list_all_panes(&expected_id)
        .expect_err("unexpected tmux error should fail");

    assert!(
        error.to_string().contains("invalid format string"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn daemon_broker_health_disables_after_unexpected_error() {
    let (enabled, reason, fallback_count) = daemon::test_broker_health_after_error("broken pipe");

    assert!(!enabled);
    assert_eq!(reason.as_deref(), Some("broken pipe"));
    assert_eq!(fallback_count, 1);
}

#[test]
fn daemon_broker_health_counts_only_fallback_transition() {
    let (reason, fallback_count) = daemon::test_broker_health_after_repeated_error("broken pipe");

    assert_eq!(reason.as_deref(), Some("broken pipe"));
    assert_eq!(fallback_count, 1);
}

#[test]
fn daemon_subscriber_exit_is_local_and_not_forwarded_as_server_exit() {
    // A subscriber (Quiet) client's `%exit` is a local detach and must be
    // swallowed so it never reaches the shared stream as a server-wide exit that
    // would stop the daemon loop.
    assert!(daemon::test_subscriber_local_exit(true, "%exit"));
    assert!(daemon::test_subscriber_local_exit(
        true,
        "%exit client detached"
    ));
    // The primary (Fatal) client still forwards `%exit` to drive shutdown.
    assert!(!daemon::test_subscriber_local_exit(false, "%exit"));
    // Non-exit subscriber lines are always forwarded.
    assert!(!daemon::test_subscriber_local_exit(
        true,
        "%subscription-changed agentscan $1 @1 0 %1 : %1:Codex:codex::::"
    ));
    assert!(!daemon::test_subscriber_local_exit(true, "%output %1 data"));
    // Match the exact `%exit` token, not a bare prefix: a hypothetical
    // `%exit`-prefixed token must not be swallowed (and stays consistent with the
    // main control-event parser, which uses the same predicate).
    assert!(!daemon::test_subscriber_local_exit(true, "%exited"));
    assert!(!daemon::test_subscriber_local_exit(true, "%exitfoo"));
}

#[test]
fn daemon_broker_health_returns_active_after_reconnect() {
    let status = daemon::test_broker_health_after_reconnect("broken pipe");

    assert_eq!(status.mode, ipc::ControlModeBrokerMode::Active);
    assert_eq!(status.disabled_reason, None);
    assert_eq!(status.reconnect_count, 1);
    assert_eq!(status.fallback_count, Some(1));
}

#[test]
fn daemon_broker_reconnect_preserves_deferred_events() {
    let deferred_events = daemon::test_reconnect_preserves_deferred_lines();

    assert_eq!(
        deferred_events,
        vec!["%subscription-changed agentscan $1 @1 0 %1 : %1:Codex:codex::::"]
    );
}

#[test]
fn daemon_broker_reconnect_drains_stale_command_frames() {
    // The retained shared channel is not replaced on reconnect, so leftover
    // `%begin`/`%end` from a timed-out command must be drained or a later brokered
    // command would consume the stale response. The drain must clear every stale
    // primary frame.
    assert_eq!(
        daemon::test_drain_control_mode_channel_clears_stale_frames(),
        0
    );
}

#[test]
fn daemon_broker_reconnect_preserves_queued_subscriber_events() {
    // Subscribers share the event channel but are independent control clients;
    // reconnecting the primary broker must not discard their queued events.
    assert_eq!(
        daemon::test_drain_control_mode_channel_preserves_subscriber_frames(),
        1
    );
}

#[test]
fn daemon_broker_status_omits_recovered_dead_subscriber_tombstone() {
    let (dead_count, subscribers) =
        daemon::test_subscriber_status_drops_recovered_dead_tombstone();

    assert_eq!(dead_count, 1);
    assert_eq!(
        subscribers,
        vec![("$2".to_string(), false), ("$3".to_string(), true)]
    );
}

#[test]
fn daemon_broker_status_keeps_dead_tombstone_until_recovery() {
    assert_eq!(
        daemon::test_recent_dead_subscriber_tombstone_persists_without_new_dead(),
        vec!["$2".to_string()]
    );
}

#[test]
fn daemon_broker_list_pane_keeps_event_shaped_rows_as_output() {
    let expected_id = broker_frame_id("211");
    let mut harness = daemon::ControlModeBrokerTranscriptHarness::new([
        daemon::ControlModeBrokerTranscriptStep::line("%begin 1777830000 211 0"),
        daemon::ControlModeBrokerTranscriptStep::line(broker_pane_row_line_with_session(
            "%window-pane-changed @999",
            "%1",
        )),
        daemon::ControlModeBrokerTranscriptStep::line("%end 1777830000 211 0"),
    ]);

    let response = harness
        .list_pane("%1", &expected_id)
        .expect("event-shaped row should still parse as command output");

    let pane = response.pane.expect("pane should exist");
    assert_eq!(pane.session_name, "%window-pane-changed @999");
    assert_eq!(pane.pane_id, "%1");
    assert!(response.deferred_events.is_empty());
}

#[test]
fn daemon_broker_list_pane_keeps_command_marker_shaped_rows_as_output() {
    let expected_id = broker_frame_id("212");
    let mut harness = daemon::ControlModeBrokerTranscriptHarness::new([
        daemon::ControlModeBrokerTranscriptStep::line("%begin 1777830000 212 0"),
        daemon::ControlModeBrokerTranscriptStep::line(broker_pane_row_line_with_session(
            "%begin 1777830001 999 0",
            "%1",
        )),
        daemon::ControlModeBrokerTranscriptStep::line("%end 1777830000 212 0"),
    ]);

    let response = harness
        .list_pane("%1", &expected_id)
        .expect("command-marker-shaped row should still parse as command output");

    let pane = response.pane.expect("pane should exist");
    assert_eq!(pane.session_name, "%begin 1777830001 999 0");
    assert_eq!(pane.pane_id, "%1");
    assert!(response.deferred_events.is_empty());
}

#[test]
fn daemon_broker_list_pane_maps_missing_target_to_none() {
    let expected_id = broker_frame_id("209");
    let mut harness = daemon::ControlModeBrokerTranscriptHarness::new([
        daemon::ControlModeBrokerTranscriptStep::line("%subscription-changed agentscan $1 @1 0 %1 : %1:Codex:codex::::"),
        daemon::ControlModeBrokerTranscriptStep::line("%begin 1777830000 209 0"),
        daemon::ControlModeBrokerTranscriptStep::line("%window-pane-changed @1 %2"),
        daemon::ControlModeBrokerTranscriptStep::line("can't find pane: %404"),
        daemon::ControlModeBrokerTranscriptStep::line("%error 1777830000 209 0"),
    ]);

    let response = harness
        .list_pane("%404", &expected_id)
        .expect("missing pane should not fail");

    assert_eq!(
        harness.written_commands(),
        &[format!("list-panes -t %404 -F {}", PANE_FORMAT)]
    );
    assert!(response.pane.is_none());
    assert_eq!(
        response.deferred_events,
        vec![
            "%subscription-changed agentscan $1 @1 0 %1 : %1:Codex:codex::::",
            "%window-pane-changed @1 %2",
        ]
    );
}

#[test]
fn daemon_broker_list_pane_preserves_unexpected_errors() {
    let expected_id = broker_frame_id("210");
    let mut harness = daemon::ControlModeBrokerTranscriptHarness::new([
        daemon::ControlModeBrokerTranscriptStep::line("%begin 1777830000 210 0"),
        daemon::ControlModeBrokerTranscriptStep::line(
            "%error 1777830000 210 0 invalid format string",
        ),
    ]);

    let error = harness
        .list_pane("%1", &expected_id)
        .expect_err("unexpected tmux error should fail");

    assert!(
        error.to_string().contains("invalid format string"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn daemon_broker_transcript_defers_async_events_inside_command_frame() {
    let expected_id = broker_frame_id("207");
    let mut harness = daemon::ControlModeBrokerTranscriptHarness::new([
        daemon::ControlModeBrokerTranscriptStep::line("%begin 1777830000 207 0"),
        daemon::ControlModeBrokerTranscriptStep::line(
            "s\u{1f}0\u{1f}0\u{1f}%1\u{1f}100\u{1f}codex\u{1f}Codex",
        ),
        daemon::ControlModeBrokerTranscriptStep::line(
            "%subscription-changed agentscan $1 @1 0 %1 : %1:Codex:codex::::",
        ),
        daemon::ControlModeBrokerTranscriptStep::line("%end 1777830000 207 0"),
    ]);

    let response = harness
        .collect_command_response(&expected_id)
        .expect("broker transcript should collect matching response");

    assert_eq!(
        response.deferred_events,
        vec!["%subscription-changed agentscan $1 @1 0 %1 : %1:Codex:codex::::"]
    );
    assert_eq!(
        response.output,
        vec!["s\u{1f}0\u{1f}0\u{1f}%1\u{1f}100\u{1f}codex\u{1f}Codex"]
    );
}

#[test]
fn daemon_broker_transcript_reports_command_error() {
    let expected_id = broker_frame_id("202");
    let mut harness = daemon::ControlModeBrokerTranscriptHarness::new([
        daemon::ControlModeBrokerTranscriptStep::line("%begin 1777830000 202 0"),
        daemon::ControlModeBrokerTranscriptStep::line(
            "%error 1777830000 202 0 can't find pane: %404",
        ),
    ]);

    let error = harness
        .collect_command_response(&expected_id)
        .expect_err("matching error frame should fail");

    assert!(
        error.to_string().contains("can't find pane: %404"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn daemon_broker_transcript_rejects_interleaved_command_frames() {
    let expected_id = broker_frame_id("203");
    let mut harness = daemon::ControlModeBrokerTranscriptHarness::new([
        daemon::ControlModeBrokerTranscriptStep::line("%begin 1777830000 203 0"),
        daemon::ControlModeBrokerTranscriptStep::line("%begin 1777830000 204 0"),
        daemon::ControlModeBrokerTranscriptStep::line("%end 1777830000 204 0"),
        daemon::ControlModeBrokerTranscriptStep::line("%end 1777830000 203 0"),
    ]);

    let error = harness
        .collect_command_response(&expected_id)
        .expect_err("interleaved command frame should fail");

    assert!(
        error
            .to_string()
            .contains("interleaved control-mode command frame"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn daemon_broker_transcript_reports_timeout_and_eof() {
    let timeout_id = broker_frame_id("205");
    let mut timeout_harness = daemon::ControlModeBrokerTranscriptHarness::new([
        daemon::ControlModeBrokerTranscriptStep::line("%begin 1777830000 205 0"),
        daemon::ControlModeBrokerTranscriptStep::Timeout,
    ]);

    let timeout = timeout_harness
        .collect_command_response(&timeout_id)
        .expect_err("timeout should fail command collection");
    assert!(
        timeout
            .to_string()
            .contains("timed out waiting for control-mode command response"),
        "unexpected error: {timeout:#}"
    );

    let eof_id = broker_frame_id("206");
    let mut eof_harness = daemon::ControlModeBrokerTranscriptHarness::new([
        daemon::ControlModeBrokerTranscriptStep::line("%begin 1777830000 206 0"),
        daemon::ControlModeBrokerTranscriptStep::Eof,
    ]);

    let eof = eof_harness
        .collect_command_response(&eof_id)
        .expect_err("EOF should fail command collection");
    assert!(
        eof.to_string()
            .contains("stream ended before command response completed"),
        "unexpected error: {eof:#}"
    );
}
