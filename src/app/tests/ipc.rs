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
    let bytes = br#"{"type":"hello","protocol_version":1,"snapshot_schema_version":4,"mode":"subscribe"}"#;

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
    let bytes = br#"{"type":"hello","protocol_version":1,"snapshot_schema_version":4,"mode":"lifecycle_status"}"#;

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
        status: ipc::LifecycleStatusFrame {
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
            unavailable_reason: None,
            message: None,
        },
    };

    let encoded = ipc::encode_frame(&frame).expect("frame should encode");
    let decoded = ipc::decode_daemon_frame(encoded.trim_ascii()).expect("frame should decode");

    assert_eq!(decoded, frame);
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
    let bytes = br#"{"type":"hello","protocol_version":1,"snapshot_schema_version":4,"mode":"snapshot","extra":true}"#;

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
    let bytes = br#"{"type":"hello","protocol_version":"1","snapshot_schema_version":4,"mode":"snapshot"}"#;

    let error = ipc::decode_client_frame(bytes).expect_err("malformed version should fail");

    assert!(
        format!("{error:#}").contains("invalid type"),
        "expected invalid type detail, got {error:#}"
    );
}

#[test]
fn ipc_frame_rejects_unknown_mode() {
    let bytes = br#"{"type":"hello","protocol_version":1,"snapshot_schema_version":4,"mode":"stream"}"#;

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
