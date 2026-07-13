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
fn activity_subscription_changes_are_distinct_from_metadata_changes() {
    assert_eq!(
        daemon::test_control_event_pane_kind(
            "%subscription-changed agentscan-activity $174 @251 1 %251 : %251:1783956107"
        ),
        Some(("activity", "%251".to_string()))
    );
    assert_eq!(
        daemon::test_control_event_pane_kind(
            "%subscription-changed agentscan $174 @251 1 %251 : %251:codex:Working"
        ),
        Some(("metadata", "%251".to_string()))
    );
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
fn output_notifications_expose_title_payload() {
    assert_eq!(
        daemon::output_title_change_title("%output %0 \\033]0;Working\\007sh-3.2$ ")
            .as_deref(),
        Some("Working")
    );
    assert_eq!(
        daemon::output_title_change_title("%output %0 \\033]2;Review patch\\033\\\\")
            .as_deref(),
        Some("Review patch")
    );
}

#[test]
fn output_title_payload_ignores_typed_backslash_escapes() {
    assert_eq!(
        daemon::output_title_change_title("%output %0 printf '\\134033]0;Working\\134007'"),
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
fn control_mode_command_markers_parse_frame_ids_and_errors() {
    assert_eq!(
        daemon::control_mode_command_marker("%begin 1777830000 101 0"),
        Some(daemon::ControlModeCommandMarker::Begin(
            daemon::ControlModeCommandFrameId {
                timestamp: "1777830000".to_string(),
                command_number: "101".to_string(),
                flags: "0".to_string(),
            }
        ))
    );
    assert_eq!(
        daemon::control_mode_command_marker("%end 1777830000 101 0"),
        Some(daemon::ControlModeCommandMarker::End(
            daemon::ControlModeCommandFrameId {
                timestamp: "1777830000".to_string(),
                command_number: "101".to_string(),
                flags: "0".to_string(),
            }
        ))
    );
    assert_eq!(
        daemon::control_mode_command_marker("%error 1777830000 101 0 can't find pane: %404"),
        Some(daemon::ControlModeCommandMarker::Error {
            id: daemon::ControlModeCommandFrameId {
                timestamp: "1777830000".to_string(),
                command_number: "101".to_string(),
                flags: "0".to_string(),
            },
            message: "can't find pane: %404".to_string(),
        })
    );
    assert_eq!(daemon::control_mode_command_marker("%window-add @1"), None);
}

#[test]
fn control_mode_command_response_collects_output_and_defers_prior_events() {
    let expected_id = daemon::ControlModeCommandFrameId {
        timestamp: "1777830000".to_string(),
        command_number: "102".to_string(),
        flags: "0".to_string(),
    };

    let response = daemon::test_collect_control_mode_command_response(
        &expected_id,
        [
            "%subscription-changed agentscan $174 @251 1 %251 : %251:Claude Code | Working:claude::::",
            "%begin 1777830000 102 0",
            "s\u{1f}0\u{1f}0\u{1f}%251\u{1f}100\u{1f}claude\u{1f}Claude Code | Working",
            "%end 1777830000 102 0",
        ],
    )
    .expect("matching frame should parse");

    assert_eq!(
        response.deferred_events,
        vec![
            "%subscription-changed agentscan $174 @251 1 %251 : %251:Claude Code | Working:claude::::"
        ]
    );
    assert_eq!(
        response.output,
        vec!["s\u{1f}0\u{1f}0\u{1f}%251\u{1f}100\u{1f}claude\u{1f}Claude Code | Working"]
    );
}

#[test]
fn control_mode_command_response_reports_errors_and_interleaved_frames() {
    let expected_id = daemon::ControlModeCommandFrameId {
        timestamp: "1777830000".to_string(),
        command_number: "103".to_string(),
        flags: "0".to_string(),
    };

    let missing = daemon::test_collect_control_mode_command_response(
        &expected_id,
        [
            "%begin 1777830000 103 0",
            "%error 1777830000 103 0 can't find pane: %404",
        ],
    )
    .expect_err("matching error frame should fail");
    assert!(
        missing.to_string().contains("can't find pane: %404"),
        "unexpected error: {missing:#}"
    );

    let interleaved = daemon::test_collect_control_mode_command_response(
        &expected_id,
        [
            "%begin 1777830000 103 0",
            "%begin 1777830000 104 0",
            "%end 1777830000 104 0",
            "%end 1777830000 103 0",
        ],
    )
    .expect_err("interleaved command frames should fail");
    assert!(
        interleaved
            .to_string()
            .contains("interleaved control-mode command frame"),
        "unexpected error: {interleaved:#}"
    );

    let unexpected_end = daemon::test_collect_control_mode_command_response(
        &expected_id,
        [
            "%begin 1777830000 103 0",
            "%end 1777830000 104 0",
            "%end 1777830000 103 0",
        ],
    )
    .expect_err("unexpected end frame should fail");
    assert!(
        unexpected_end
            .to_string()
            .contains("interleaved control-mode command frame"),
        "unexpected error: {unexpected_end:#}"
    );

    let unexpected_error = daemon::test_collect_control_mode_command_response(
        &expected_id,
        [
            "%begin 1777830000 103 0",
            "%error 1777830000 104 0 other command failed",
            "%end 1777830000 103 0",
        ],
    )
    .expect_err("unexpected error frame should fail");
    assert!(
        unexpected_error
            .to_string()
            .contains("interleaved control-mode command frame"),
        "unexpected error: {unexpected_error:#}"
    );
}

#[test]
fn daemon_subscription_format_includes_wrapper_metadata_fields() {
    // Single-brace `#{...}` directives: the string is sent to tmux verbatim, so
    // doubled braces would render every field as a literal `}` (see the constant's
    // doc comment). These assertions guard against regressing back to that.
    assert!(DAEMON_SUBSCRIPTION_FORMAT.contains("#{pane_current_command}"));
    assert!(DAEMON_SUBSCRIPTION_FORMAT.contains("#{pane_title}"));
    assert!(DAEMON_SUBSCRIPTION_FORMAT.contains("#{@agent.provider}"));
    assert!(DAEMON_SUBSCRIPTION_FORMAT.contains("#{@agent.state}"));
    assert!(DAEMON_SUBSCRIPTION_FORMAT.contains("#{@agent.session_id}"));
    assert!(!DAEMON_SUBSCRIPTION_FORMAT.contains("#{window_activity}"));
    assert!(DAEMON_ACTIVITY_SUBSCRIPTION_FORMAT.contains("#{window_activity}"));
    assert!(DAEMON_ACTIVITY_SUBSCRIPTION_FORMAT.starts_with("agentscan-activity:%*:"));
    // Explicitly reject the doubled-brace form that broke the subscription.
    assert!(!DAEMON_SUBSCRIPTION_FORMAT.contains("#{{"));
}

#[test]
fn detects_notification_names() {
    assert_eq!(
        daemon::notification_name("%window-renamed @1 editor"),
        Some("%window-renamed")
    );
    assert_eq!(daemon::notification_name("plain output"), None);
}
