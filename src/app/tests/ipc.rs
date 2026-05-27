use std::fs;
use std::io::Cursor;

fn test_uid() -> u32 {
    #[cfg(unix)]
    {
        // SAFETY: geteuid has no preconditions and does not mutate Rust-managed memory.
        unsafe { libc::geteuid() }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

fn socket_config(platform: ipc::SocketPathPlatform) -> ipc::SocketPathConfig {
    ipc::SocketPathConfig {
        explicit_path: None,
        xdg_runtime_dir: None,
        tmpdir: None,
        home: None,
        xdg_state_home: None,
        platform,
        uid: test_uid(),
    }
}

#[test]
fn ipc_socket_path_uses_explicit_override() {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    #[cfg(unix)]
    fs::set_permissions(tempdir.path(), fs::Permissions::from_mode(0o700))
        .expect("permissions should be set");
    let socket_path = tempdir.path().join("agentscan.sock");
    let mut config = socket_config(ipc::SocketPathPlatform::Unix);
    config.explicit_path = Some(socket_path.clone());

    let resolved = ipc::resolve_socket_path_with_config(&config).expect("explicit path should work");

    assert_eq!(resolved, socket_path);
}

#[test]
fn ipc_socket_path_rejects_explicit_overlong_path() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let long_name = "s".repeat(128);
    let mut config = socket_config(ipc::SocketPathPlatform::Unix);
    config.explicit_path = Some(tempdir.path().join(long_name));

    let error =
        ipc::resolve_socket_path_with_config(&config).expect_err("overlong path should fail");

    assert!(
        error.to_string().contains("too long"),
        "expected overlong path error, got {error:#}"
    );
}

#[cfg(unix)]
#[test]
fn ipc_socket_path_rejects_explicit_world_accessible_parent() {
    use std::os::unix::fs::PermissionsExt;

    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    fs::set_permissions(tempdir.path(), fs::Permissions::from_mode(0o755))
        .expect("permissions should be set");
    let mut config = socket_config(ipc::SocketPathPlatform::Unix);
    config.explicit_path = Some(tempdir.path().join("agentscan.sock"));

    let error =
        ipc::resolve_socket_path_with_config(&config).expect_err("open parent should fail");

    assert!(
        error.to_string().contains("not private"),
        "expected private permissions error, got {error:#}"
    );
}

#[cfg(unix)]
#[test]
fn ipc_socket_path_tightens_owned_runtime_directory() {
    use std::os::unix::fs::PermissionsExt;

    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    fs::set_permissions(tempdir.path(), fs::Permissions::from_mode(0o755))
        .expect("permissions should be set");
    let mut config = socket_config(ipc::SocketPathPlatform::Unix);
    config.xdg_runtime_dir = Some(tempdir.path().to_path_buf());

    let resolved = ipc::resolve_socket_path_with_config(&config).expect("runtime path should work");

    assert_eq!(resolved, tempdir.path().join("agentscan").join("agentscan.sock"));
    let mode = fs::metadata(tempdir.path().join("agentscan"))
        .expect("socket dir should exist")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o700);
}

#[test]
fn ipc_socket_path_uses_macos_temp_fallback() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut config = socket_config(ipc::SocketPathPlatform::Macos);
    config.tmpdir = Some(tempdir.path().to_path_buf());

    let resolved = ipc::resolve_socket_path_with_config(&config).expect("temp fallback should work");

    assert_eq!(
        resolved,
        tempdir
            .path()
            .join(format!("agentscan-{}", test_uid()))
            .join("agentscan.sock")
    );
}

#[test]
fn ipc_socket_path_uses_macos_cache_fallback() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut config = socket_config(ipc::SocketPathPlatform::Macos);
    config.home = Some(tempdir.path().to_path_buf());

    let resolved =
        ipc::resolve_socket_path_with_config(&config).expect("macOS cache fallback should work");

    assert_eq!(
        resolved,
        tempdir
            .path()
            .join("Library")
            .join("Caches")
            .join("agentscan")
            .join("agentscan.sock")
    );
}

#[test]
fn ipc_socket_path_uses_unix_state_fallback() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut config = socket_config(ipc::SocketPathPlatform::Unix);
    config.xdg_state_home = Some(tempdir.path().join("state"));

    let resolved =
        ipc::resolve_socket_path_with_config(&config).expect("Unix state fallback should work");

    assert_eq!(
        resolved,
        tempdir
            .path()
            .join("state")
            .join("agentscan")
            .join("agentscan.sock")
    );
}

#[test]
fn ipc_socket_path_uses_unix_home_state_fallback() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut config = socket_config(ipc::SocketPathPlatform::Unix);
    config.home = Some(tempdir.path().to_path_buf());

    let resolved =
        ipc::resolve_socket_path_with_config(&config).expect("home state fallback should work");

    assert_eq!(
        resolved,
        tempdir
            .path()
            .join(".local")
            .join("state")
            .join("agentscan")
            .join("agentscan.sock")
    );
}

#[test]
fn ipc_frame_roundtrips_snapshot_hello() {
    let frame = ipc::ClientFrame::Hello {
        protocol_version: ipc::WIRE_PROTOCOL_VERSION,
        snapshot_schema_version: CACHE_SCHEMA_VERSION,
        mode: ipc::ClientMode::Snapshot,
    };

    let encoded = ipc::encode_frame(&frame).expect("frame should encode");
    let decoded = ipc::decode_client_frame(encoded.trim_ascii()).expect("frame should decode");

    assert_eq!(decoded, frame);
    assert_eq!(
        ipc::validate_client_hello(&decoded),
        ipc::DaemonFrame::HelloAck {
            protocol_version: ipc::WIRE_PROTOCOL_VERSION,
            snapshot_schema_version: CACHE_SCHEMA_VERSION
        }
    );
}

#[test]
fn ipc_frame_accepts_subscribe_mode() {
    let bytes = br#"{"type":"hello","protocol_version":1,"snapshot_schema_version":5,"mode":"subscribe"}"#;

    let decoded = ipc::decode_client_frame(bytes).expect("subscribe hello should decode");

    assert_eq!(
        decoded,
        ipc::ClientFrame::Hello {
            protocol_version: ipc::WIRE_PROTOCOL_VERSION,
            snapshot_schema_version: CACHE_SCHEMA_VERSION,
            mode: ipc::ClientMode::Subscribe,
        }
    );
}

#[test]
fn ipc_frame_accepts_lifecycle_status_mode() {
    let bytes = br#"{"type":"hello","protocol_version":1,"snapshot_schema_version":5,"mode":"lifecycle_status"}"#;

    let decoded = ipc::decode_client_frame(bytes).expect("lifecycle status hello should decode");

    assert_eq!(
        decoded,
        ipc::ClientFrame::Hello {
            protocol_version: ipc::WIRE_PROTOCOL_VERSION,
            snapshot_schema_version: CACHE_SCHEMA_VERSION,
            mode: ipc::ClientMode::LifecycleStatus,
        }
    );
}

#[test]
fn ipc_lifecycle_status_frame_roundtrips() {
    let frame = ipc::DaemonFrame::LifecycleStatus {
        status: Box::new(ipc::LifecycleStatusFrame {
            state: ipc::LifecycleDaemonState::Ready,
            identity: ipc::DaemonIdentityFrame {
                pid: 42,
                daemon_start_time: "2026-05-03T00:00:00Z".to_string(),
                executable: "/tmp/agentscan".to_string(),
                executable_canonical: Some("/tmp/agentscan".to_string()),
                socket_path: "/tmp/agentscan.sock".to_string(),
                protocol_version: ipc::WIRE_PROTOCOL_VERSION,
                snapshot_schema_version: CACHE_SCHEMA_VERSION,
            },
            subscriber_count: 3,
            latest_snapshot_generated_at: Some("2026-05-03T00:00:01Z".to_string()),
            latest_snapshot_pane_count: Some(5),
            latest_snapshot_update_source: Some("reconcile".to_string()),
            latest_snapshot_update_detail: Some("interval".to_string()),
            latest_snapshot_update_duration_ms: Some(12),
            control_mode_broker: Some(ipc::ControlModeBrokerStatusFrame {
                mode: ipc::ControlModeBrokerMode::Active,
                disabled_reason: None,
                reconnect_count: 1,
                fallback_count: Some(2),
                subscriber_count: Some(3),
            }),
            runtime_telemetry: Some(ipc::RuntimeTelemetryFrame {
                control_event_refresh_count: 3,
                control_event_batch_count: 4,
                control_event_line_count: 5,
                control_event_output_line_count: 0,
                control_event_output_byte_count: 0,
                control_event_pane_count: 6,
                control_event_title_count: 7,
                control_event_window_count: 8,
                control_event_session_count: 9,
                control_event_resnapshot_count: 10,
                control_event_ignored_count: 11,
                reconcile_attempt_count: 4,
                reconcile_noop_count: 5,
                reconcile_changed_snapshot_count: 6,
                targeted_title_update_count: 7,
                targeted_pane_refresh_count: 8,
                targeted_scope_refresh_count: 9,
                full_snapshot_refresh_count: 10,
                targeted_refresh_fallback_to_full_count: 7,
                broker_fallback_count: 2,
                pane_output_capture_attempt_count: 0,
                pane_output_capture_hit_count: 0,
                pane_output_capture_error_count: 0,
            }),
            latest_snapshot_observability: Some(ipc::SnapshotObservabilityFrame {
                provider_known_count: 1,
                provider_unknown_count: 2,
                status_source_pane_metadata_count: 3,
                status_source_tmux_title_count: 4,
                status_source_pane_output_count: 5,
                status_source_not_checked_count: 6,
                proc_fallback_not_run_count: 7,
                proc_fallback_skipped_count: 8,
                proc_fallback_no_match_count: 9,
                proc_fallback_error_count: 10,
                proc_fallback_resolved_count: 11,
                per_provider: std::collections::BTreeMap::new(),
            }),
            recent_events: vec![ipc::DaemonObservabilityEventFrame {
                at: "2026-05-03T00:00:02Z".to_string(),
                source: "reconcile".to_string(),
                detail: Some("interval".to_string()),
                refresh: "full_snapshot".to_string(),
                changed: true,
                published: true,
                duration_ms: Some(12),
                diff: Some(ipc::SnapshotDiffFrame {
                    added_pane_ids: vec!["%2".to_string()],
                    removed_pane_ids: vec!["%1".to_string()],
                    changed_panes: vec![ipc::SnapshotPaneDiffFrame {
                        pane_id: "%3".to_string(),
                        fields: vec!["status".to_string()],
                    }],
                    truncated: false,
                }),
            }],
            unavailable_reason: None,
            message: None,
        }),
    };

    let encoded = ipc::encode_frame(&frame).expect("frame should encode");
    let decoded = ipc::decode_daemon_frame(encoded.trim_ascii()).expect("frame should decode");

    assert_eq!(decoded, frame);
}

#[test]
fn ipc_lifecycle_status_preserves_missing_runtime_telemetry() {
    let bytes = br#"{"type":"lifecycle_status","status":{"state":"ready","identity":{"pid":42,"daemon_start_time":"2026-05-03T00:00:00Z","executable":"/tmp/agentscan","executable_canonical":null,"socket_path":"/tmp/agentscan.sock","protocol_version":1,"snapshot_schema_version":5},"subscriber_count":0,"latest_snapshot_generated_at":null,"latest_snapshot_pane_count":null,"latest_snapshot_update_source":null,"latest_snapshot_update_detail":null,"latest_snapshot_update_duration_ms":null,"control_mode_broker":null,"unavailable_reason":null,"message":null}}"#;

    let decoded = ipc::decode_daemon_frame(bytes).expect("status frame should decode");
    let ipc::DaemonFrame::LifecycleStatus { status } = decoded else {
        panic!("expected lifecycle status frame");
    };

    assert_eq!(status.runtime_telemetry, None);
}

#[test]
fn ipc_lifecycle_status_preserves_missing_broker_fallback_count() {
    let bytes = br#"{"type":"lifecycle_status","status":{"state":"ready","identity":{"pid":42,"daemon_start_time":"2026-05-03T00:00:00Z","executable":"/tmp/agentscan","executable_canonical":null,"socket_path":"/tmp/agentscan.sock","protocol_version":1,"snapshot_schema_version":5},"subscriber_count":0,"latest_snapshot_generated_at":null,"latest_snapshot_pane_count":null,"latest_snapshot_update_source":null,"latest_snapshot_update_detail":null,"latest_snapshot_update_duration_ms":null,"control_mode_broker":{"mode":"fallback","disabled_reason":"old daemon","reconnect_count":1},"runtime_telemetry":null,"unavailable_reason":null,"message":null}}"#;

    let decoded = ipc::decode_daemon_frame(bytes).expect("status frame should decode");
    let ipc::DaemonFrame::LifecycleStatus { status } = decoded else {
        panic!("expected lifecycle status frame");
    };

    assert_eq!(
        status
            .control_mode_broker
            .expect("broker status should decode")
            .fallback_count,
        None
    );
}

#[test]
fn ipc_hello_validation_rejects_protocol_mismatch() {
    let frame = ipc::ClientFrame::Hello {
        protocol_version: ipc::WIRE_PROTOCOL_VERSION + 1,
        snapshot_schema_version: CACHE_SCHEMA_VERSION,
        mode: ipc::ClientMode::Snapshot,
    };

    let response = ipc::validate_client_hello(&frame);

    assert!(matches!(
        response,
        ipc::DaemonFrame::Shutdown {
            reason: ipc::ShutdownReason::ProtocolMismatch,
            ..
        }
    ));
}

#[test]
fn ipc_hello_validation_rejects_schema_mismatch() {
    let frame = ipc::ClientFrame::Hello {
        protocol_version: ipc::WIRE_PROTOCOL_VERSION,
        snapshot_schema_version: CACHE_SCHEMA_VERSION + 1,
        mode: ipc::ClientMode::Snapshot,
    };

    let response = ipc::validate_client_hello(&frame);

    assert!(matches!(
        response,
        ipc::DaemonFrame::Shutdown {
            reason: ipc::ShutdownReason::SchemaMismatch,
            ..
        }
    ));
}

#[test]
fn ipc_frame_rejects_unknown_fields() {
    let bytes = br#"{"type":"hello","protocol_version":1,"snapshot_schema_version":5,"mode":"snapshot","extra":true}"#;

    let error = ipc::decode_client_frame(bytes).expect_err("unknown field should fail");

    assert!(
        error.to_string().contains("failed to decode client IPC frame"),
        "expected decode context, got {error:#}"
    );
    assert!(
        format!("{error:#}").contains("unknown field"),
        "expected unknown field detail, got {error:#}"
    );
}

#[test]
fn ipc_frame_rejects_missing_required_fields() {
    let bytes = br#"{"type":"hello","protocol_version":1,"mode":"snapshot"}"#;

    let error = ipc::decode_client_frame(bytes).expect_err("missing field should fail");

    assert!(
        format!("{error:#}").contains("missing field"),
        "expected missing field detail, got {error:#}"
    );
}

#[test]
fn ipc_frame_rejects_malformed_version() {
    let bytes = br#"{"type":"hello","protocol_version":"1","snapshot_schema_version":5,"mode":"snapshot"}"#;

    let error = ipc::decode_client_frame(bytes).expect_err("malformed version should fail");

    assert!(
        format!("{error:#}").contains("invalid type"),
        "expected invalid type detail, got {error:#}"
    );
}

#[test]
fn ipc_frame_rejects_unknown_mode() {
    let bytes = br#"{"type":"hello","protocol_version":1,"snapshot_schema_version":5,"mode":"stream"}"#;

    let error = ipc::decode_client_frame(bytes).expect_err("unknown mode should fail");

    assert!(
        format!("{error:#}").contains("unknown variant"),
        "expected unknown variant detail, got {error:#}"
    );
}

#[test]
fn ipc_read_client_frame_enforces_hello_byte_limit() {
    let oversized = format!("{{\"type\":\"hello\",\"padding\":\"{}\"}}\n", "x".repeat(5000));
    let mut reader = Cursor::new(oversized.into_bytes());

    let error = ipc::read_client_frame(&mut reader).expect_err("oversize hello should fail");

    assert!(
        error.to_string().contains("byte limit"),
        "expected byte limit error, got {error:#}"
    );
}

#[test]
fn ipc_read_daemon_frame_enforces_configured_byte_limit_before_newline() {
    let mut reader = Cursor::new(b"abcdef".to_vec());

    let error = ipc::read_bounded_json_line(&mut reader, 4).expect_err("oversize read should fail");

    assert!(
        error.to_string().contains("byte limit"),
        "expected byte limit error, got {error:#}"
    );
}

#[test]
fn ipc_read_daemon_frame_decodes_ack() {
    let frame = ipc::DaemonFrame::HelloAck {
        protocol_version: ipc::WIRE_PROTOCOL_VERSION,
        snapshot_schema_version: CACHE_SCHEMA_VERSION,
    };
    let encoded = ipc::encode_frame(&frame).expect("frame should encode");
    let mut reader = Cursor::new(encoded);

    let decoded = ipc::read_daemon_frame(&mut reader)
        .expect("read should succeed")
        .expect("frame should be present");

    assert_eq!(decoded, frame);
}
