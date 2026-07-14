use std::io::{BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

fn empty_socket_snapshot(generated_at: &str) -> SnapshotEnvelope {
    SnapshotEnvelope {
        schema_version: CACHE_SCHEMA_VERSION,
        generated_at: generated_at.to_string(),
        source: super::SnapshotSource {
            kind: SourceKind::Daemon,
            tmux_version: Some("3.4".to_string()),
            daemon_generated_at: Some(generated_at.to_string()),
        },
        panes: Vec::new(),
    }
}

fn encoded_snapshot_frame(seq: u64, snapshot: &SnapshotEnvelope) -> daemon::EncodedDaemonFrame {
    let encoded = ipc::encode_frame(&ipc::DaemonFrame::Snapshot {
        seq,
        snapshot: snapshot.clone(),
    })
    .expect("snapshot frame should encode");
    std::sync::Arc::<[u8]>::from(encoded)
}

/// Builds a `snapshot_diff` fan-out broadcast whose primary is the given diff
/// frame and whose coalesce fallback is a full snapshot at `seq`.
fn diff_broadcast(
    primary: daemon::EncodedDaemonFrame,
    full_snapshot: &SnapshotEnvelope,
    seq: u64,
) -> daemon::DaemonBroadcast {
    daemon::DaemonBroadcast::test_diff(primary, full_snapshot.clone(), seq)
}

fn socket_hello(mode: ipc::ClientMode) -> ipc::ClientFrame {
    ipc::ClientFrame::Hello {
        protocol_version: ipc::WIRE_PROTOCOL_VERSION,
        snapshot_schema_version: CACHE_SCHEMA_VERSION,
        mode,
    }
}

fn socket_client_event(event: ipc::ClientEventFrame) -> ipc::ClientFrame {
    ipc::ClientFrame::ClientEvent {
        protocol_version: ipc::WIRE_PROTOCOL_VERSION,
        snapshot_schema_version: CACHE_SCHEMA_VERSION,
        event,
    }
}

fn exchange_daemon_frames(
    state: daemon::DaemonSocketState,
    hello: ipc::ClientFrame,
) -> Vec<ipc::DaemonFrame> {
    let (mut client, server) = UnixStream::pair().expect("socket pair should be created");
    let handle = std::thread::spawn(move || {
        daemon::handle_daemon_socket_client(server, &state).expect("client should be handled");
    });

    client
        .write_all(&ipc::encode_frame(&hello).expect("hello should encode"))
        .expect("hello should be written");
    client
        .shutdown(std::net::Shutdown::Write)
        .expect("client write side should close");

    let mut reader = BufReader::new(client);
    let mut frames = Vec::new();
    while let Some(frame) =
        ipc::read_daemon_frame(&mut reader).expect("daemon frame should decode")
    {
        frames.push(frame);
    }

    handle.join().expect("handler should join");
    frames
}

fn read_all_daemon_frames(client: UnixStream) -> Vec<ipc::DaemonFrame> {
    client
        .shutdown(std::net::Shutdown::Write)
        .expect("client write side should close");
    let mut reader = BufReader::new(client);
    let mut frames = Vec::new();
    while let Some(frame) =
        ipc::read_daemon_frame(&mut reader).expect("daemon frame should decode")
    {
        frames.push(frame);
    }
    frames
}

struct LiveSubscriber {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
    handle: Option<JoinHandle<()>>,
}

impl LiveSubscriber {
    fn connect(state: daemon::DaemonSocketState) -> Self {
        let (mut client, server) = UnixStream::pair().expect("socket pair should be created");
        let server_state = state;
        let handle = std::thread::spawn(move || {
            daemon::handle_daemon_socket_client(server, &server_state)
                .expect("subscriber client should be handled");
        });
        client
            .write_all(
                &ipc::encode_frame(&socket_hello(ipc::ClientMode::Subscribe))
                    .expect("hello should encode"),
            )
            .expect("hello should be written");
        let reader_stream = client.try_clone().expect("client should clone");
        reader_stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("read timeout should set");
        Self {
            writer: client,
            reader: BufReader::new(reader_stream),
            handle: Some(handle),
        }
    }

    fn read_frame(&mut self) -> ipc::DaemonFrame {
        ipc::read_daemon_frame(&mut self.reader)
            .expect("daemon frame should decode")
            .expect("daemon should send a frame")
    }

    fn join(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.join().expect("subscriber handler should join");
        }
    }
}

impl Drop for LiveSubscriber {
    fn drop(&mut self) {
        let _ = self.writer.shutdown(std::net::Shutdown::Write);
        self.join();
    }
}

fn open_ready_subscriber(
    state: daemon::DaemonSocketState,
    expected_snapshot: SnapshotEnvelope,
) -> LiveSubscriber {
    let mut subscriber = LiveSubscriber::connect(state);
    assert_eq!(
        subscriber.read_frame(),
        ipc::DaemonFrame::HelloAck {
            protocol_version: ipc::WIRE_PROTOCOL_VERSION,
            snapshot_schema_version: CACHE_SCHEMA_VERSION,
        }
    );
    match subscriber.read_frame() {
        ipc::DaemonFrame::Snapshot { seq, snapshot } => {
            assert_eq!(seq, 1, "bootstrap snapshot should carry the initial seq");
            assert_eq!(snapshot, expected_snapshot);
        }
        other => panic!("expected bootstrap snapshot frame, got {other:?}"),
    }
    subscriber
}

fn hello_ack_frame() -> ipc::DaemonFrame {
    ipc::DaemonFrame::HelloAck {
        protocol_version: ipc::WIRE_PROTOCOL_VERSION,
        snapshot_schema_version: CACHE_SCHEMA_VERSION,
    }
}

fn serve_snapshot_responses(
    socket_path: &Path,
    responses: Vec<Vec<ipc::DaemonFrame>>,
) -> JoinHandle<usize> {
    let listener = UnixListener::bind(socket_path).expect("snapshot listener should bind");
    std::thread::spawn(move || {
        let mut accepted = 0;
        for response in responses {
            let (mut stream, _) = listener.accept().expect("snapshot client should connect");
            let reader_stream = stream.try_clone().expect("server stream should clone");
            let mut reader = BufReader::new(reader_stream);
            assert_eq!(
                ipc::read_client_frame(&mut reader)
                    .expect("client hello should decode")
                    .expect("client should send hello"),
                socket_hello(ipc::ClientMode::Snapshot)
            );
            for frame in response {
                if let Err(error) =
                    stream.write_all(&ipc::encode_frame(&frame).expect("frame should encode"))
                {
                    assert_eq!(
                        error.kind(),
                        std::io::ErrorKind::BrokenPipe,
                        "daemon response should write or observe client disconnect"
                    );
                    break;
                }
            }
            accepted += 1;
        }
        accepted
    })
}

fn serve_lifecycle_responses(
    socket_path: &Path,
    responses: Vec<ipc::DaemonFrame>,
) -> JoinHandle<usize> {
    let listener = UnixListener::bind(socket_path).expect("lifecycle listener should bind");
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("lifecycle client should connect");
        let reader_stream = stream.try_clone().expect("server stream should clone");
        let mut reader = BufReader::new(reader_stream);
        assert_eq!(
            ipc::read_client_frame(&mut reader)
                .expect("client hello should decode")
                .expect("client should send hello"),
            socket_hello(ipc::ClientMode::LifecycleStatus)
        );
        for frame in responses {
            if let Err(error) =
                stream.write_all(&ipc::encode_frame(&frame).expect("frame should encode"))
            {
                assert_eq!(
                    error.kind(),
                    std::io::ErrorKind::BrokenPipe,
                    "daemon response should write or observe client disconnect"
                );
                break;
            }
        }
        1
    })
}

fn wait_for_subscriber_count(state: &daemon::DaemonSocketState, expected: usize) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if state.subscriber_count() == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for subscriber count {expected}, got {}",
            state.subscriber_count()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

enum FakeInitialSnapshot {
    Ready,
    Oversized,
    Failed(&'static str),
}

enum FakeTmuxStartup {
    Started,
    Failed(&'static str),
}

struct FakeDaemonStartup {
    initial_snapshot: FakeInitialSnapshot,
    tmux_startup: FakeTmuxStartup,
}

impl FakeDaemonStartup {
    fn ready() -> Self {
        Self {
            initial_snapshot: FakeInitialSnapshot::Ready,
            tmux_startup: FakeTmuxStartup::Started,
        }
    }

    fn with_initial_snapshot(initial_snapshot: FakeInitialSnapshot) -> Self {
        Self {
            initial_snapshot,
            ..Self::ready()
        }
    }

    fn with_tmux_startup(tmux_startup: FakeTmuxStartup) -> Self {
        Self {
            tmux_startup,
            ..Self::ready()
        }
    }

}

impl daemon::StartupActions for FakeDaemonStartup {
    fn tmux_version(&self) -> Option<String> {
        Some("test-tmux".to_string())
    }

    fn initial_snapshot(&self, tmux_version: Option<&str>) -> anyhow::Result<SnapshotEnvelope> {
        match self.initial_snapshot {
            FakeInitialSnapshot::Ready => {
                let mut snapshot = empty_socket_snapshot("2026-05-03T00:00:00Z");
                snapshot.source.tmux_version = tmux_version.map(str::to_string);
                Ok(snapshot)
            }
            FakeInitialSnapshot::Oversized => {
                let mut snapshot = empty_socket_snapshot("2026-05-03T00:00:00Z");
                snapshot.source.tmux_version = tmux_version.map(str::to_string);
                snapshot.generated_at = "x".repeat(ipc::DAEMON_FRAME_MAX_BYTES);
                Ok(snapshot)
            }
            FakeInitialSnapshot::Failed(message) => anyhow::bail!("{message}"),
        }
    }

    fn start_tmux_control_mode_client(
        &self,
    ) -> anyhow::Result<daemon::StartedTmuxControlModeClient> {
        match self.tmux_startup {
            FakeTmuxStartup::Started => {
                Ok(daemon::StartedTmuxControlModeClient::test_started_without_process())
            }
            FakeTmuxStartup::Failed(message) => anyhow::bail!("{message}"),
        }
    }

}

fn wait_for_socket_connection(socket_path: &Path) -> UnixStream {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match UnixStream::connect(socket_path) {
            Ok(stream) => return stream,
            Err(error) if Instant::now() < deadline => {
                if !matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) {
                    panic!("unexpected socket connection error: {error}");
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("daemon socket did not become observable: {error}"),
        }
    }
}

fn observe_startup_failed_frames(
    startup: FakeDaemonStartup,
) -> (Vec<ipc::DaemonFrame>, String) {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let socket_path = tempdir.path().join("agentscan.sock");
    let run_socket_path = socket_path.clone();
    let handle =
        std::thread::spawn(move || daemon::test_daemon_run_with_startup(&run_socket_path, startup));

    let deadline = Instant::now() + Duration::from_secs(2);
    let frames = loop {
        let mut client = wait_for_socket_connection(&socket_path);
        client
            .write_all(
                &ipc::encode_frame(&socket_hello(ipc::ClientMode::Snapshot))
                    .expect("hello should encode"),
            )
            .expect("hello should write");
        let frames = read_all_daemon_frames(client);
        if matches!(
            frames.as_slice(),
            [
                ipc::DaemonFrame::HelloAck { .. },
                ipc::DaemonFrame::Unavailable {
                    reason: ipc::UnavailableReason::StartupFailed,
                    ..
                },
            ]
        ) {
            break frames;
        }
        if Instant::now() >= deadline {
            panic!("daemon socket did not expose startup_failed; last frames: {frames:?}");
        }
        std::thread::sleep(Duration::from_millis(5));
    };

    let error = handle
        .join()
        .expect("daemon startup thread should join")
        .expect_err("startup should fail");

    (frames, format!("{error:#}"))
}

fn assert_startup_failed_contains(
    startup: FakeDaemonStartup,
    expected_reason: &str,
) -> String {
    let (frames, error) = observe_startup_failed_frames(startup);

    match frames.as_slice() {
        [
            ipc::DaemonFrame::HelloAck { .. },
            ipc::DaemonFrame::Unavailable {
                reason: ipc::UnavailableReason::StartupFailed,
                message,
            },
        ] => {
            assert!(
                message.contains(expected_reason),
                "expected startup_failed message to contain {expected_reason:?}, got {message:?}"
            );
            assert!(
                message.contains("no usable socket snapshot was published"),
                "expected startup_failed message to include publication guidance, got {message:?}"
            );
        }
        other => panic!("expected startup_failed frames, got {other:?}"),
    }

    assert!(
        error.contains(expected_reason),
        "expected daemon error to contain {expected_reason:?}, got {error:?}"
    );
    assert!(
        error.contains("no usable socket snapshot was published"),
        "expected daemon error to include publication guidance, got {error:?}"
    );

    error
}

#[test]
fn daemon_socket_snapshot_client_receives_ack_snapshot_and_eof() {
    let state = daemon::DaemonSocketState::new();
    let snapshot = empty_socket_snapshot("2026-05-03T00:00:00Z");
    state
        .publish_initial_snapshot(snapshot.clone())
        .expect("snapshot should publish");

    let frames = exchange_daemon_frames(state, socket_hello(ipc::ClientMode::Snapshot));

    assert_eq!(
        frames,
        vec![
            ipc::DaemonFrame::HelloAck {
                protocol_version: ipc::WIRE_PROTOCOL_VERSION,
                snapshot_schema_version: CACHE_SCHEMA_VERSION,
            },
            ipc::DaemonFrame::Snapshot { seq: 1, snapshot },
        ]
    );
}

#[test]
fn daemon_socket_snapshot_client_receives_not_ready_during_initialization() {
    let state = daemon::DaemonSocketState::new();

    let frames = exchange_daemon_frames(state, socket_hello(ipc::ClientMode::Snapshot));

    assert!(matches!(
        frames.as_slice(),
        [
            ipc::DaemonFrame::HelloAck { .. },
            ipc::DaemonFrame::Unavailable {
                reason: ipc::UnavailableReason::DaemonNotReady,
                ..
            },
        ]
    ));
}

#[test]
fn daemon_socket_snapshot_client_receives_startup_failed_after_failure() {
    let state = daemon::DaemonSocketState::new();
    state.mark_startup_failed("tmux attach failed".to_string());

    let frames = exchange_daemon_frames(state, socket_hello(ipc::ClientMode::Snapshot));

    assert!(matches!(
        frames.as_slice(),
        [
            ipc::DaemonFrame::HelloAck { .. },
            ipc::DaemonFrame::Unavailable {
                reason: ipc::UnavailableReason::StartupFailed,
                ..
            },
        ]
    ));
}

#[test]
fn daemon_socket_closing_state_wins_over_not_ready() {
    let state = daemon::DaemonSocketState::new();
    state.mark_closing();

    let frames = exchange_daemon_frames(state, socket_hello(ipc::ClientMode::Snapshot));

    assert!(matches!(
        frames.as_slice(),
        [
            ipc::DaemonFrame::HelloAck { .. },
            ipc::DaemonFrame::Unavailable {
                reason: ipc::UnavailableReason::ServerClosing,
                ..
            },
        ]
    ));
}

#[test]
fn daemon_socket_lifecycle_status_reports_ready_identity_and_counts() {
    let state = daemon::DaemonSocketState::new();
    let snapshot = empty_socket_snapshot("2026-05-03T00:00:00Z");
    state
        .publish_initial_snapshot(snapshot)
        .expect("snapshot should publish");

    let frames = exchange_daemon_frames(state, socket_hello(ipc::ClientMode::LifecycleStatus));

    match frames.as_slice() {
        [
            ipc::DaemonFrame::HelloAck { .. },
            ipc::DaemonFrame::LifecycleStatus { status },
        ] => {
            assert_eq!(status.state, ipc::LifecycleDaemonState::Ready);
            assert_eq!(status.identity.protocol_version, ipc::WIRE_PROTOCOL_VERSION);
            assert_eq!(status.identity.snapshot_schema_version, CACHE_SCHEMA_VERSION);
            assert_eq!(status.subscriber_count, 0);
            assert_eq!(
                status.latest_snapshot_generated_at.as_deref(),
                Some("2026-05-03T00:00:00Z")
            );
            assert_eq!(status.latest_snapshot_pane_count, Some(0));
            assert_eq!(
                status.latest_snapshot_update_source.as_deref(),
                Some("initial_snapshot")
            );
            assert_eq!(status.latest_snapshot_update_detail, None);
            assert_eq!(status.latest_snapshot_update_duration_ms, None);
            assert_eq!(status.control_mode_broker, None);
            assert_eq!(status.runtime_telemetry, None);
            assert_eq!(
                status.latest_snapshot_observability,
                Some(ipc::SnapshotObservabilityFrame::default())
            );
            assert!(status.recent_events.is_empty());
            assert_eq!(status.unavailable_reason, None);
        }
        other => panic!("expected lifecycle status frames, got {other:?}"),
    }
}

#[test]
fn daemon_socket_observability_event_ring_is_bounded() {
    let state = daemon::DaemonSocketState::new();
    for index in 0..300 {
        state.test_record_observability_event(ipc::DaemonObservabilityEventFrame {
            at: format!("2026-05-03T00:00:{:02}Z", index % 60),
            source: "control_event".to_string(),
            detail: Some(format!("pane:%{index}")),
            refresh: "targeted_pane".to_string(),
            control_sources: Vec::new(),
            control_lines: Vec::new(),
            changed: index % 2 == 0,
            published: index % 2 == 0,
            duration_ms: Some(index),
            diff: None,
        });
    }

    assert_eq!(state.test_recent_event_count(), 256);
}

#[test]
fn daemon_socket_state_recovers_from_poisoned_lock() {
    let state = daemon::DaemonSocketState::new();
    state
        .publish_initial_snapshot(empty_socket_snapshot("2026-05-03T00:00:00Z"))
        .expect("snapshot should publish");

    // Simulate a handler thread panicking while holding the state mutex.
    state.test_poison_lock();

    // The daemon must keep serving: a subsequent state operation and a full
    // snapshot exchange still succeed instead of panicking on the poisoned lock.
    assert_eq!(state.test_recent_event_count(), 0);
    let frames = exchange_daemon_frames(state, socket_hello(ipc::ClientMode::Snapshot));
    assert!(
        frames
            .iter()
            .any(|frame| matches!(frame, ipc::DaemonFrame::Snapshot { .. })),
        "expected a snapshot frame after lock poisoning, got: {frames:?}"
    );
}

#[test]
fn daemon_socket_client_event_is_enqueued() {
    let state = daemon::DaemonSocketState::new();
    state
        .publish_initial_snapshot(empty_socket_snapshot("2026-05-03T00:00:00Z"))
        .expect("snapshot should publish");
    let receiver = state.test_install_client_event_sender();
    let event = ipc::ClientEventFrame::PaneFocus {
        pane_id: "%42".to_string(),
    };

    let frames = exchange_daemon_frames(state.clone(), socket_client_event(event.clone()));

    assert_eq!(frames, vec![hello_ack_frame()]);
    assert_eq!(daemon::test_recv_client_event(&receiver), Some(event));
}

#[test]
fn daemon_socket_client_event_reports_not_ready_without_runtime_queue() {
    let state = daemon::DaemonSocketState::new();

    let frames = exchange_daemon_frames(
        state,
        socket_client_event(ipc::ClientEventFrame::PaneFocus {
            pane_id: "%42".to_string(),
        }),
    );

    assert!(matches!(
        frames.as_slice(),
        [
            ipc::DaemonFrame::HelloAck { .. },
            ipc::DaemonFrame::Unavailable {
                reason: ipc::UnavailableReason::DaemonNotReady,
                ..
            },
        ]
    ));
}

#[test]
fn daemon_lifecycle_query_rejects_incompatible_hello_ack() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let socket_path = tempdir.path().join("agentscan.sock");
    let status = ipc::LifecycleStatusFrame {
        state: ipc::LifecycleDaemonState::Ready,
        identity: ipc::DaemonIdentityFrame {
            pid: 42,
            daemon_start_time: "2026-05-03T00:00:00Z".to_string(),
            executable: "/bin/agentscan".to_string(),
            executable_canonical: None,
            socket_path: socket_path.display().to_string(),
            protocol_version: ipc::WIRE_PROTOCOL_VERSION + 1,
            snapshot_schema_version: CACHE_SCHEMA_VERSION,
        },
        subscriber_count: 0,
        latest_snapshot_generated_at: None,
        latest_snapshot_pane_count: None,
        latest_snapshot_update_source: None,
        latest_snapshot_update_detail: None,
        latest_snapshot_update_duration_ms: None,
        control_mode_broker: Some(ipc::ControlModeBrokerStatusFrame {
            mode: ipc::ControlModeBrokerMode::Fallback,
            disabled_reason: Some("test fallback".to_string()),
            reconnect_count: 2,
            fallback_count: Some(3),
            subscriber_count: Some(2),
            primary_session_id: None,
            subscriber_coverage_complete: None,
            desired_subscriber_count: None,
            active_subscriber_count: None,
            missing_subscriber_session_ids: None,
            dead_subscriber_count: None,
            subscribers: None,
            last_subscriber_reconcile_at: None,
            next_subscriber_monitor_in_ms: None,
            next_reconcile_in_ms: None,
        }),
        runtime_telemetry: Some(ipc::RuntimeTelemetryFrame {
            control_event_refresh_count: 4,
            control_event_batch_count: 5,
            control_event_line_count: 6,
            control_event_output_line_count: 0,
            control_event_output_byte_count: 0,
            control_event_pane_count: 7,
            control_event_title_count: 8,
            control_event_window_count: 9,
            control_event_session_count: 10,
            control_event_resnapshot_count: 11,
            control_event_ignored_count: 12,
            reconcile_attempt_count: 5,
            reconcile_noop_count: 6,
            reconcile_changed_snapshot_count: 7,
            targeted_title_update_count: 8,
            targeted_pane_refresh_count: 9,
            targeted_scope_refresh_count: 10,
            full_snapshot_refresh_count: 11,
            targeted_refresh_fallback_to_full_count: 8,
            subscriber_monitor_count: Some(0),
            subscriber_start_count: Some(0),
            subscriber_reattach_count: Some(0),
            subscriber_attach_failure_count: Some(0),
            subscriber_exit_count: Some(0),
            broker_fallback_count: 3,
            pane_output_capture_attempt_count: 0,
            pane_output_capture_hit_count: 0,
            pane_output_capture_error_count: 0,
        }),
        latest_snapshot_observability: None,
        recent_events: Vec::new(),
        unavailable_reason: None,
        message: None,
    };
    let handle = serve_lifecycle_responses(
        &socket_path,
        vec![
            ipc::DaemonFrame::HelloAck {
                protocol_version: ipc::WIRE_PROTOCOL_VERSION + 1,
                snapshot_schema_version: CACHE_SCHEMA_VERSION,
            },
            ipc::DaemonFrame::LifecycleStatus {
                status: Box::new(status),
            },
        ],
    );

    let error = daemon::daemon_status_with_socket_path(&socket_path, OutputFormat::Text, false)
        .expect_err("incompatible lifecycle ack should fail");

    assert!(error.to_string().contains("incompatible lifecycle handshake"));
    assert_eq!(handle.join().expect("server should join"), 1);
}

#[test]
fn daemon_auto_start_translates_tmux_env_socket_before_removing_tmux() {
    let read_env = |name: &str| match name {
        "TMUX" => Some(OsString::from("/tmp/custom-tmux.sock,123,0")),
        _ => None,
    };
    assert_eq!(
        daemon::test_daemon_start_tmux_envs_from(read_env),
        vec![(
            OsString::from(TMUX_SOCKET_ENV_VAR),
            OsString::from("/tmp/custom-tmux.sock")
        )]
    );
    assert_eq!(
        daemon::test_daemon_start_env_removes_from(read_env),
        vec![OsString::from("TMUX")]
    );
}

#[test]
fn daemon_auto_start_preserves_explicit_agentscan_tmux_socket() {
    let read_env = |name: &str| match name {
        TMUX_SOCKET_ENV_VAR => Some(OsString::from("/tmp/explicit.sock")),
        "TMUX" => Some(OsString::from("/tmp/custom-tmux.sock,123,0")),
        _ => None,
    };
    assert!(daemon::test_daemon_start_tmux_envs_from(read_env).is_empty());
    assert_eq!(
        daemon::test_daemon_start_env_removes_from(read_env),
        vec![OsString::from("TMUX")]
    );
}

#[test]
fn implicit_macos_auto_start_blocks_untrusted_executable() {
    let error = daemon::test_implicit_consumer_macos_auto_start_preflight(
        Some("codesign reports an ad-hoc executable"),
    )
    .expect_err("implicit auto-start should block untrusted macOS executables");

    assert!(matches!(
        error,
        daemon::DaemonSnapshotError::AutoStartDisabled { .. }
    ));
    let message = error.to_string();
    assert!(message.contains("macOS executable trust preflight rejected"));
    assert!(message.contains("codesign reports an ad-hoc executable"));
    assert!(message.contains("agentscan daemon run"));
}

#[test]
fn tui_macos_auto_start_blocks_untrusted_executable() {
    let error = daemon::test_tui_macos_auto_start_preflight(
        Some("codesign reports an ad-hoc executable"),
    )
    .expect_err("TUI auto-start should block untrusted macOS executables");

    assert!(matches!(
        error,
        daemon::DaemonSnapshotError::AutoStartDisabled { .. }
    ));
    assert!(
        error
            .to_string()
            .contains("macOS executable trust preflight rejected")
    );
    assert!(
        error
            .to_string()
            .contains("codesign reports an ad-hoc executable")
    );
    assert!(error.to_string().contains("agentscan daemon run"));
}

#[test]
fn explicit_macos_daemon_start_blocks_untrusted_executable() {
    daemon::test_explicit_macos_daemon_start_preflight(
        Some("codesign reports an ad-hoc executable"),
    )
    .expect_err("detached explicit daemon start should block untrusted macOS executables");
}

#[test]
fn daemon_restart_skips_stop_when_start_preflight_fails() {
    let stopped = std::cell::Cell::new(false);
    let started = std::cell::Cell::new(false);

    let error = daemon::test_daemon_restart_with_steps(
        || {
            Err(daemon::DaemonSnapshotError::AutoStartDisabled {
                reason: "preflight blocked start".to_string(),
            })
        },
        || {
            stopped.set(true);
            Ok(())
        },
        || {
            started.set(true);
            Ok(())
        },
    )
    .expect_err("restart should fail before stopping when preflight rejects start");

    assert!(!stopped.get(), "restart must preserve the running daemon");
    assert!(!started.get(), "restart must not attempt start after preflight failure");
    assert!(error.to_string().contains("preflight blocked start"));
}

#[test]
fn macos_preflight_assesses_all_detached_starts() {
    assert!(daemon::test_macos_start_requires_trust_preflight(
        true, false
    ));
    assert!(daemon::test_macos_start_requires_trust_preflight(
        false, true
    ));
    assert!(daemon::test_macos_start_requires_trust_preflight(
        false, false
    ));
}

#[test]
fn implicit_macos_auto_start_allows_trusted_executable() {
    daemon::test_implicit_consumer_macos_auto_start_preflight(None)
        .expect("implicit auto-start should allow trusted macOS executables");
}

#[test]
fn tui_macos_auto_start_allows_trusted_executable() {
    daemon::test_tui_macos_auto_start_preflight(None)
        .expect("TUI auto-start should allow trusted macOS executables");
}

#[test]
fn macos_codesign_assessment_allows_valid_signed_cli_output() {
    let display_text = "\
Executable=/usr/bin/git
Identifier=com.apple.git
Format=Mach-O universal
CodeDirectory v=20400 size=123 flags=0x0(none) hashes=1+0 location=embedded
Authority=Software Signing
Authority=Apple Code Signing Certification Authority
Authority=Apple Root CA
TeamIdentifier=not set
";

    daemon::test_macos_executable_assessment_for_outputs(
        true,
        display_text,
        true,
        "",
    )
        .expect("valid non-ad-hoc CLI signatures should be trusted");
}

#[test]
fn macos_codesign_assessment_blocks_adhoc_output_before_verify() {
    let display_text = "\
Executable=/tmp/agentscan
CodeDirectory v=20400 size=123 flags=0x20002(adhoc,linker-signed) hashes=1+0 location=embedded
Signature=adhoc
TeamIdentifier=not set
";

    let reason = daemon::test_macos_executable_assessment_for_outputs(
        true,
        display_text,
        true,
        "",
    )
    .expect_err("ad-hoc signatures should be rejected");

    assert!(reason.contains("ad-hoc executable"));
}

#[test]
fn macos_codesign_assessment_blocks_invalid_signed_output() {
    let display_text = "\
Executable=/tmp/agentscan
Identifier=com.example.agentscan
Authority=Developer ID Application: Example
TeamIdentifier=ABCDE12345
";

    let reason = daemon::test_macos_executable_assessment_for_outputs(
        true,
        display_text,
        false,
        "/tmp/agentscan: invalid signature",
    )
    .expect_err("invalid signatures should be rejected");

    assert!(reason.contains("codesign verification failed"));
}

#[test]
fn macos_executable_assessment_trusts_valid_signed_output_without_spctl() {
    let display_text = "\
Executable=/tmp/agentscan
Identifier=com.example.agentscan
Authority=Developer ID Application: Example
TeamIdentifier=ABCDE12345
";

    daemon::test_macos_executable_assessment_for_outputs(true, display_text, true, "")
        .expect("valid signed output should not require Gatekeeper assessment");
}

#[test]
fn daemon_socket_subscribe_client_receives_bootstrap_and_update() {
    let state = daemon::DaemonSocketState::new();
    let initial = empty_socket_snapshot("2026-05-03T00:00:00Z");
    state
        .publish_initial_snapshot(initial.clone())
        .expect("snapshot should publish");

    let mut subscriber = open_ready_subscriber(state.clone(), initial);
    wait_for_subscriber_count(&state, 1);

    let updated = empty_socket_snapshot("2026-05-03T00:00:01Z");
    assert!(state.publish_later_snapshot(updated.clone()));

    // Post-bootstrap publishes broadcast an incremental diff, not a full frame:
    // the two empty snapshots differ only in the envelope-level `generated_at`, so
    // the diff carries no pane changes but advances the seq to 2.
    match subscriber.read_frame() {
        ipc::DaemonFrame::SnapshotDiff {
            seq,
            schema_version,
            generated_at,
            source,
            changed_panes,
            removed_pane_ids,
        } => {
            assert_eq!(seq, 2);
            assert_eq!(schema_version, updated.schema_version);
            assert_eq!(generated_at, updated.generated_at);
            assert_eq!(source, updated.source);
            assert!(changed_panes.is_empty());
            assert!(removed_pane_ids.is_empty());
        }
        other => panic!("expected snapshot diff frame, got {other:?}"),
    }
}

#[test]
fn daemon_socket_protocol_mismatch_shutdown_skips_ack() {
    let state = daemon::DaemonSocketState::new();
    let hello = ipc::ClientFrame::Hello {
        protocol_version: ipc::WIRE_PROTOCOL_VERSION + 1,
        snapshot_schema_version: CACHE_SCHEMA_VERSION,
        mode: ipc::ClientMode::Snapshot,
    };

    let frames = exchange_daemon_frames(state, hello);

    assert!(matches!(
        frames.as_slice(),
        [ipc::DaemonFrame::Shutdown {
            reason: ipc::ShutdownReason::ProtocolMismatch,
            ..
        }]
    ));
}

#[test]
fn daemon_socket_oversized_initial_snapshot_fails_publication() {
    let state = daemon::DaemonSocketState::new();
    let mut snapshot = empty_socket_snapshot("2026-05-03T00:00:00Z");
    snapshot.generated_at = "x".repeat(ipc::DAEMON_FRAME_MAX_BYTES);

    let error = state
        .publish_initial_snapshot(snapshot)
        .expect_err("oversized snapshot should fail");

    assert!(
        error.to_string().contains("exceeded socket frame limit"),
        "expected frame limit error, got {error:#}"
    );
}

#[test]
fn daemon_socket_oversized_later_snapshot_preserves_last_good_frame() {
    let state = daemon::DaemonSocketState::new();
    let initial = empty_socket_snapshot("2026-05-03T00:00:00Z");
    state
        .publish_initial_snapshot(initial.clone())
        .expect("initial snapshot should publish");

    let mut oversized = empty_socket_snapshot("2026-05-03T00:00:01Z");
    oversized.generated_at = "x".repeat(ipc::DAEMON_FRAME_MAX_BYTES);
    assert!(!state.publish_later_snapshot(oversized));

    let frames = exchange_daemon_frames(state, socket_hello(ipc::ClientMode::Snapshot));

    assert_eq!(
        frames,
        vec![
            ipc::DaemonFrame::HelloAck {
                protocol_version: ipc::WIRE_PROTOCOL_VERSION,
                snapshot_schema_version: CACHE_SCHEMA_VERSION,
            },
            ipc::DaemonFrame::Snapshot { seq: 1, snapshot: initial },
        ]
    );
}

// A pane whose encoded size is dominated by `title_len`, for tests that steer
// a snapshot toward the wire frame limit.
fn socket_frame_limit_pane(pane_id: &str, pane_index: u32, title_len: usize) -> PaneRecord {
    PaneRecord {
        pane_id: pane_id.to_string(),
        location: super::PaneLocation {
            session_name: "bulk".to_string(),
            window_index: 0,
            pane_index,
            window_name: "win".to_string(),
        },
        tmux: super::TmuxPaneMetadata {
            pane_pid: 100,
            pane_tty: "/dev/ttys000".to_string(),
            pane_current_path: "/tmp".to_string(),
            pane_current_command: "node".to_string(),
            pane_title_raw: "x".repeat(title_len),
            session_id: Some("$1".to_string()),
            window_id: Some("@1".to_string()),
            pane_active: false,
            window_active: false,
        },
        display: super::DisplayMetadata {
            label: pane_id.to_string(),
            activity_label: None,
        },
        provider: None,
        status: super::PaneStatus::not_checked(),
        classification: super::PaneClassification {
            matched_by: None,
            confidence: None,
            reasons: Vec::new(),
        },
        agent_metadata: super::AgentMetadata::default(),
        diagnostics: super::PaneDiagnostics {
            cache_origin: "daemon_snapshot".to_string(),
            proc_fallback: super::ProcFallbackDiagnostics::default(),
        },
    }
}

#[test]
fn daemon_socket_rejects_diff_that_grows_snapshot_past_frame_limit() {
    let state = daemon::DaemonSocketState::new();
    // Bootstrap just under the wire limit so it publishes and serves fine.
    let mut initial = empty_socket_snapshot("2026-05-03T00:00:00Z");
    initial
        .panes
        .push(socket_frame_limit_pane("%1", 0, ipc::DAEMON_FRAME_MAX_BYTES - 64 * 1024));
    state
        .publish_initial_snapshot(initial.clone())
        .expect("near-limit initial snapshot should publish");

    // One small added pane: the diff frame easily fits the wire limit, but the
    // committed snapshot's FULL frame would not, leaving every later bootstrap
    // and one-shot query unservable. The publish must be rejected instead.
    let mut grown = initial.clone();
    grown.generated_at = "2026-05-03T00:00:01Z".to_string();
    grown.source.daemon_generated_at = Some("2026-05-03T00:00:01Z".to_string());
    grown.panes.push(socket_frame_limit_pane("%2", 1, 128 * 1024));
    assert!(!state.publish_later_snapshot(grown));

    // The last good snapshot stays authoritative and bootstrap-servable.
    let frames = exchange_daemon_frames(state, socket_hello(ipc::ClientMode::Snapshot));
    assert_eq!(
        frames,
        vec![
            ipc::DaemonFrame::HelloAck {
                protocol_version: ipc::WIRE_PROTOCOL_VERSION,
                snapshot_schema_version: CACHE_SCHEMA_VERSION,
            },
            ipc::DaemonFrame::Snapshot {
                seq: 1,
                snapshot: initial
            },
        ]
    );
}

// A diff frame carries changed panes plus removed pane ids, so it can exceed
// the wire limit even when the new snapshot's full frame fits (a near-limit
// replacement that also removes many panes). The publish must fall back to a
// full-frame broadcast instead of failing — the daemon's runtime state has
// already adopted the new snapshot, so a failed publish strands clients on the
// old one forever.
#[test]
fn daemon_socket_publishes_full_frame_when_diff_alone_exceeds_frame_limit() {
    let state = daemon::DaemonSocketState::new();
    // Bootstrap: many panes whose ids alone total ~2 MiB, so the ids reappear
    // in the next diff's `removed_pane_ids`.
    let mut initial = empty_socket_snapshot("2026-05-03T00:00:00Z");
    for index in 0..40 {
        let long_id = format!("%prev-{index}-{}", "x".repeat(50 * 1024));
        let mut pane = socket_frame_limit_pane(&long_id, index, 8);
        pane.display.label = format!("p{index}");
        initial.panes.push(pane);
    }
    state
        .publish_initial_snapshot(initial)
        .expect("bootstrap snapshot should publish");

    // Replacement: one large pane (~3.5 MiB full frame, fits) while removing
    // every bootstrap pane (~2 MiB of removed ids): the diff frame (~5.5 MiB)
    // exceeds the limit even though the full frame does not.
    let mut replacement = empty_socket_snapshot("2026-05-03T00:00:01Z");
    replacement.source.daemon_generated_at = Some("2026-05-03T00:00:01Z".to_string());
    replacement.panes.push(socket_frame_limit_pane(
        "%new",
        0,
        3 * 1024 * 1024 + 512 * 1024,
    ));
    assert!(state.publish_later_snapshot(replacement.clone()));

    // A fresh snapshot query serves the replacement, proving the store adopted it.
    let frames = exchange_daemon_frames(state, socket_hello(ipc::ClientMode::Snapshot));
    assert_eq!(
        frames,
        vec![
            ipc::DaemonFrame::HelloAck {
                protocol_version: ipc::WIRE_PROTOCOL_VERSION,
                snapshot_schema_version: CACHE_SCHEMA_VERSION,
            },
            ipc::DaemonFrame::Snapshot {
                seq: 2,
                snapshot: replacement
            },
        ]
    );
}

// `snapshot_wire_diff` omits panes whose only change is the volatile
// `diagnostics.cache_origin`, but full frames still serialize that field: the
// store's size bound must count the omitted growth or a cache_origin-only
// publish could silently commit a snapshot whose full frame no longer encodes.
#[test]
fn daemon_socket_rejects_omitted_pane_growth_past_frame_limit() {
    let state = daemon::DaemonSocketState::new();
    let mut initial = empty_socket_snapshot("2026-05-03T00:00:00Z");
    initial
        .panes
        .push(socket_frame_limit_pane("%1", 0, ipc::DAEMON_FRAME_MAX_BYTES - 64 * 1024));
    state
        .publish_initial_snapshot(initial.clone())
        .expect("near-limit initial snapshot should publish");

    // The pane stays materially equal (only cache_origin changes), so the diff
    // frame is tiny — but the committed FULL frame would exceed the wire limit.
    let mut grown = initial.clone();
    grown.generated_at = "2026-05-03T00:00:01Z".to_string();
    grown.source.daemon_generated_at = Some("2026-05-03T00:00:01Z".to_string());
    grown.panes[0].diagnostics.cache_origin = "x".repeat(128 * 1024);
    assert!(!state.publish_later_snapshot(grown));

    // The last good snapshot stays authoritative and bootstrap-servable.
    let frames = exchange_daemon_frames(state, socket_hello(ipc::ClientMode::Snapshot));
    assert_eq!(
        frames,
        vec![
            ipc::DaemonFrame::HelloAck {
                protocol_version: ipc::WIRE_PROTOCOL_VERSION,
                snapshot_schema_version: CACHE_SCHEMA_VERSION,
            },
            ipc::DaemonFrame::Snapshot {
                seq: 1,
                snapshot: initial
            },
        ]
    );
}

#[test]
fn daemon_socket_subscriber_mailbox_single_enqueue_delivers_primary() {
    let mailbox = daemon::SubscriberMailbox::new();
    let snapshot = empty_socket_snapshot("2026-05-03T00:00:01Z");
    // A diff primary that the mailbox delivers verbatim when nothing is pending.
    let diff_primary = encoded_snapshot_frame(2, &snapshot);
    mailbox.enqueue(diff_broadcast(diff_primary.clone(), &snapshot, 2));

    assert_eq!(
        mailbox
            .try_take_pending()
            .expect("mailbox should have pending frame")
            .as_ref(),
        diff_primary.as_ref()
    );
    mailbox.close();
    assert!(mailbox.is_closed());
}

#[test]
fn daemon_socket_subscriber_mailbox_coalesce_upgrades_to_full_snapshot() {
    let mailbox = daemon::SubscriberMailbox::new();
    let first = empty_socket_snapshot("2026-05-03T00:00:01Z");
    let second = empty_socket_snapshot("2026-05-03T00:00:02Z");

    // A slow subscriber that never drained the first diff: coalescing the second
    // enqueue over it must NOT silently drop a diff (that would diverge). Instead
    // the mailbox replaces the whole pending slot with the second broadcast's
    // absolute full snapshot, so the subscriber re-syncs regardless.
    mailbox.enqueue(diff_broadcast(encoded_snapshot_frame(2, &first), &first, 2));
    mailbox.enqueue(diff_broadcast(encoded_snapshot_frame(3, &second), &second, 3));

    let pending = mailbox
        .try_take_pending()
        .expect("mailbox should have pending frame");
    let frame = ipc::decode_daemon_frame(&pending).expect("pending frame should decode");
    match frame {
        ipc::DaemonFrame::Snapshot { seq, snapshot } => {
            assert_eq!(seq, 3, "coalesced frame should be the latest full snapshot");
            assert_eq!(snapshot, second);
        }
        other => panic!("expected coalesced full snapshot frame, got {other:?}"),
    }
}

#[test]
fn daemon_socket_subscriber_limit_returns_unavailable_and_recovers_capacity() {
    let state = daemon::DaemonSocketState::new();
    let initial = empty_socket_snapshot("2026-05-03T00:00:00Z");
    state
        .publish_initial_snapshot(initial.clone())
        .expect("snapshot should publish");

    let mut subscriber_ids = Vec::new();
    for _ in 0..daemon::MAX_SUBSCRIBERS {
        subscriber_ids.push(
            state
                .test_register_subscriber_for_capacity()
                .expect("subscriber should register"),
        );
    }
    wait_for_subscriber_count(&state, daemon::MAX_SUBSCRIBERS);

    let frames = exchange_daemon_frames(state.clone(), socket_hello(ipc::ClientMode::Subscribe));
    assert!(matches!(
        frames.as_slice(),
        [
            ipc::DaemonFrame::HelloAck { .. },
            ipc::DaemonFrame::Unavailable {
                reason: ipc::UnavailableReason::SubscriberLimitReached,
                ..
            },
        ]
    ));
    wait_for_subscriber_count(&state, daemon::MAX_SUBSCRIBERS);

    assert!(state.test_retire_subscriber(subscriber_ids[0]));
    wait_for_subscriber_count(&state, daemon::MAX_SUBSCRIBERS - 1);

    let _replacement = open_ready_subscriber(state.clone(), initial);
    wait_for_subscriber_count(&state, daemon::MAX_SUBSCRIBERS);
}

#[test]
fn daemon_socket_pending_handshake_limit_returns_server_busy() {
    let state = daemon::DaemonSocketState::new();
    let mut guards = Vec::new();
    for _ in 0..daemon::MAX_PENDING_HANDSHAKES {
        guards.push(
            state
                .try_acquire_pending_handshake()
                .expect("pending handshake should reserve"),
        );
    }
    assert!(state.try_acquire_pending_handshake().is_none());

    let (client, server) = UnixStream::pair().expect("socket pair should be created");
    daemon::refuse_server_busy(server);
    let mut reader = BufReader::new(client.try_clone().expect("client should clone"));
    let frame = ipc::read_daemon_frame(&mut reader)
        .expect("shutdown frame should decode")
        .expect("shutdown frame should be present");
    assert!(matches!(
        frame,
        ipc::DaemonFrame::Shutdown {
            reason: ipc::ShutdownReason::ServerBusy,
            ..
        }
    ));

    drop(guards);
    assert_eq!(state.pending_handshake_count(), 0);
    let _ = client.shutdown(std::net::Shutdown::Both);
}

#[test]
fn daemon_socket_subscriber_protocol_violation_retires_and_recovers_capacity() {
    let state = daemon::DaemonSocketState::new();
    let initial = empty_socket_snapshot("2026-05-03T00:00:00Z");
    state
        .publish_initial_snapshot(initial.clone())
        .expect("snapshot should publish");
    let mut subscriber = open_ready_subscriber(state.clone(), initial.clone());
    wait_for_subscriber_count(&state, 1);

    subscriber
        .writer
        .write_all(b"unexpected post-handshake bytes\n")
        .expect("protocol violation should write");
    subscriber.join();
    wait_for_subscriber_count(&state, 0);

    let _replacement = open_ready_subscriber(state.clone(), initial);
    wait_for_subscriber_count(&state, 1);
}

#[test]
fn daemon_socket_subscriber_eof_retires_and_recovers_capacity() {
    let state = daemon::DaemonSocketState::new();
    let initial = empty_socket_snapshot("2026-05-03T00:00:00Z");
    state
        .publish_initial_snapshot(initial.clone())
        .expect("snapshot should publish");
    let mut subscriber = open_ready_subscriber(state.clone(), initial.clone());
    wait_for_subscriber_count(&state, 1);

    subscriber
        .writer
        .shutdown(std::net::Shutdown::Write)
        .expect("subscriber EOF should close write side");
    subscriber.join();
    wait_for_subscriber_count(&state, 0);

    let _replacement = open_ready_subscriber(state.clone(), initial);
    wait_for_subscriber_count(&state, 1);
}

#[test]
fn daemon_socket_oversized_update_is_skipped_for_subscribers() {
    let state = daemon::DaemonSocketState::new();
    let initial = empty_socket_snapshot("2026-05-03T00:00:00Z");
    state
        .publish_initial_snapshot(initial.clone())
        .expect("snapshot should publish");
    let mut subscriber = open_ready_subscriber(state.clone(), initial);

    let mut oversized = empty_socket_snapshot("2026-05-03T00:00:01Z");
    oversized.generated_at = "x".repeat(ipc::DAEMON_FRAME_MAX_BYTES);
    assert!(!state.publish_later_snapshot(oversized));

    let next_good = empty_socket_snapshot("2026-05-03T00:00:02Z");
    assert!(state.publish_later_snapshot(next_good.clone()));
    // The oversized publish was skipped (no frame enqueued and seq unchanged), so
    // the next good publish is the subscriber's first post-bootstrap frame: a diff
    // at seq 2 carrying the new envelope fields.
    match subscriber.read_frame() {
        ipc::DaemonFrame::SnapshotDiff {
            seq,
            generated_at,
            changed_panes,
            removed_pane_ids,
            ..
        } => {
            assert_eq!(seq, 2);
            assert_eq!(generated_at, next_good.generated_at);
            assert!(changed_panes.is_empty());
            assert!(removed_pane_ids.is_empty());
        }
        other => panic!("expected snapshot diff frame, got {other:?}"),
    }
}

#[test]
fn daemon_socket_closing_retires_subscribers_and_rejects_new_subscribers() {
    let state = daemon::DaemonSocketState::new();
    let initial = empty_socket_snapshot("2026-05-03T00:00:00Z");
    state
        .publish_initial_snapshot(initial.clone())
        .expect("snapshot should publish");
    let mut subscriber = open_ready_subscriber(state.clone(), initial);
    wait_for_subscriber_count(&state, 1);

    state.mark_closing();
    state.mark_closing();
    assert!(matches!(
        subscriber.read_frame(),
        ipc::DaemonFrame::Unavailable {
            reason: ipc::UnavailableReason::ServerClosing,
            ..
        }
    ));
    wait_for_subscriber_count(&state, 0);
    subscriber.join();

    let frames = exchange_daemon_frames(state, socket_hello(ipc::ClientMode::Subscribe));
    assert!(matches!(
        frames.as_slice(),
        [
            ipc::DaemonFrame::HelloAck { .. },
            ipc::DaemonFrame::Unavailable {
                reason: ipc::UnavailableReason::ServerClosing,
                ..
            },
        ]
    ));
}

#[test]
fn daemon_socket_initial_snapshot_failure_is_observable_as_startup_failed() {
    assert_startup_failed_contains(
        FakeDaemonStartup::with_initial_snapshot(FakeInitialSnapshot::Failed(
            "initial tmux snapshot failed",
        )),
        "initial tmux snapshot failed",
    );
}

#[test]
fn daemon_socket_oversized_initial_snapshot_fails_startup_with_guidance() {
    let error = assert_startup_failed_contains(
        FakeDaemonStartup::with_initial_snapshot(FakeInitialSnapshot::Oversized),
        "encoded snapshot frame was",
    );

    assert!(
        error.contains("exceeding daemon frame limit"),
        "expected frame limit in startup error, got {error:?}"
    );
    assert!(
        error.contains("initial snapshot failed before daemon socket readiness"),
        "expected initial snapshot context in startup error, got {error:?}"
    );
}

#[test]
fn daemon_socket_tmux_attach_failure_is_observable_as_startup_failed() {
    assert_startup_failed_contains(
        FakeDaemonStartup::with_tmux_startup(FakeTmuxStartup::Failed("tmux attach failed")),
        "tmux attach failed",
    );
}

#[test]
fn daemon_socket_subscription_setup_failure_is_observable_as_startup_failed() {
    assert_startup_failed_contains(
        FakeDaemonStartup::with_tmux_startup(FakeTmuxStartup::Failed(
            "tmux rejected daemon subscription setup",
        )),
        "tmux rejected daemon subscription setup",
    );
}

#[test]
fn daemon_socket_subscription_setup_waits_past_attach_response() {
    let error = daemon::test_wait_for_attach_then_subscription_transcript(&[
        "%begin 1777830000 100 0",
        "%end 1777830000 100 0",
        "%begin 1777830000 101 0",
        "%error 1777830000 101 0 no such format",
    ])
    .expect_err("subscription response error after attach success should fail startup");
    let message = format!("{error:#}");

    assert!(
        message.contains("daemon subscription setup"),
        "expected subscription setup context, got {message:?}"
    );
    assert!(
        message.contains("no such format"),
        "expected tmux error detail, got {message:?}"
    );
}

#[test]
fn daemon_socket_startup_failure_removes_owned_socket_path() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let socket_path = tempdir.path().join("agentscan.sock");
    let run_socket_path = socket_path.clone();
    let handle = std::thread::spawn(move || {
        daemon::test_daemon_run_with_startup(
            &run_socket_path,
            FakeDaemonStartup::with_initial_snapshot(FakeInitialSnapshot::Failed(
                "initial tmux snapshot failed",
            )),
        )
    });

    let mut client = wait_for_socket_connection(&socket_path);
    client
        .write_all(
            &ipc::encode_frame(&socket_hello(ipc::ClientMode::Snapshot))
                .expect("hello should encode"),
        )
        .expect("hello should write");
    let _ = read_all_daemon_frames(client);

    handle
        .join()
        .expect("daemon startup thread should join")
        .expect_err("startup should fail");
    assert!(
        !socket_path.exists(),
        "owned daemon socket path should be removed on startup failure"
    );
}

#[test]
fn daemon_socket_client_handler_works_over_real_unix_socket_path() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let socket_path = tempdir.path().join("agentscan.sock");
    let listener = UnixListener::bind(&socket_path).expect("listener should bind temp socket path");
    assert!(socket_path.exists());

    let state = daemon::DaemonSocketState::new();
    let snapshot = empty_socket_snapshot("2026-05-03T00:00:00Z");
    state
        .publish_initial_snapshot(snapshot.clone())
        .expect("snapshot should publish");
    let server_state = state.clone();
    let handle = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("client should connect");
        daemon::handle_daemon_socket_client(stream, &server_state).expect("client should handle");
    });

    let mut client = UnixStream::connect(&socket_path).expect("client should connect to path");
    client
        .write_all(
            &ipc::encode_frame(&socket_hello(ipc::ClientMode::Snapshot))
                .expect("hello should encode"),
        )
        .expect("hello should write");
    let frames = read_all_daemon_frames(client);

    handle.join().expect("server should join");
    assert_eq!(
        frames,
        vec![
            ipc::DaemonFrame::HelloAck {
                protocol_version: ipc::WIRE_PROTOCOL_VERSION,
                snapshot_schema_version: CACHE_SCHEMA_VERSION,
            },
            ipc::DaemonFrame::Snapshot { seq: 1, snapshot },
        ]
    );
}

#[test]
fn daemon_socket_client_handler_waits_on_nonblocking_accepted_stream() {
    let state = daemon::DaemonSocketState::new();
    let snapshot = empty_socket_snapshot("2026-05-03T00:00:00Z");
    state
        .publish_initial_snapshot(snapshot.clone())
        .expect("snapshot should publish");

    let (mut client, server) = UnixStream::pair().expect("socket pair should be created");
    server
        .set_nonblocking(true)
        .expect("server stream should be configurable as nonblocking");
    let handle = std::thread::spawn(move || {
        daemon::handle_daemon_socket_client(server, &state).expect("client should handle");
    });

    std::thread::sleep(Duration::from_millis(25));
    client
        .write_all(
            &ipc::encode_frame(&socket_hello(ipc::ClientMode::Snapshot))
                .expect("hello should encode"),
        )
        .expect("hello should write after delayed client startup");
    let frames = read_all_daemon_frames(client);

    handle.join().expect("server should join");
    assert_eq!(
        frames,
        vec![
            ipc::DaemonFrame::HelloAck {
                protocol_version: ipc::WIRE_PROTOCOL_VERSION,
                snapshot_schema_version: CACHE_SCHEMA_VERSION,
            },
            ipc::DaemonFrame::Snapshot { seq: 1, snapshot },
        ]
    );
}

#[test]
fn daemon_snapshot_helper_reads_ready_socket() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let socket_path = tempdir.path().join("agentscan.sock");
    let snapshot = empty_socket_snapshot("2026-05-03T00:00:00Z");
    let handle = serve_snapshot_responses(
        &socket_path,
        vec![vec![
            hello_ack_frame(),
            ipc::DaemonFrame::Snapshot {
                seq: 1,
                snapshot: snapshot.clone(),
            },
        ]],
    );

    let actual =
        daemon::snapshot_via_socket_path(&socket_path, daemon::AutoStartPolicy::disabled_for_tests())
            .expect("snapshot helper should read ready daemon");

    assert_eq!(actual, snapshot);
    assert_eq!(handle.join().expect("server should join"), 1);
}

#[test]
fn daemon_snapshot_helper_retries_not_ready_then_succeeds() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let socket_path = tempdir.path().join("agentscan.sock");
    let snapshot = empty_socket_snapshot("2026-05-03T00:00:01Z");
    let handle = serve_snapshot_responses(
        &socket_path,
        vec![
            vec![
                hello_ack_frame(),
                ipc::DaemonFrame::Unavailable {
                    reason: ipc::UnavailableReason::DaemonNotReady,
                    message: "initializing".to_string(),
                },
            ],
            vec![
                hello_ack_frame(),
                ipc::DaemonFrame::Snapshot {
                    seq: 1,
                    snapshot: snapshot.clone(),
                },
            ],
        ],
    );

    let actual =
        daemon::snapshot_via_socket_path(&socket_path, daemon::AutoStartPolicy::disabled_for_tests())
            .expect("snapshot helper should retry not-ready daemon");

    assert_eq!(actual, snapshot);
    assert_eq!(handle.join().expect("server should join"), 2);
}

#[test]
fn daemon_snapshot_helper_treats_startup_failed_as_terminal() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let socket_path = tempdir.path().join("agentscan.sock");
    let handle = serve_snapshot_responses(
        &socket_path,
        vec![vec![
            hello_ack_frame(),
            ipc::DaemonFrame::Unavailable {
                reason: ipc::UnavailableReason::StartupFailed,
                message: "tmux server is unavailable".to_string(),
            },
        ]],
    );

    let error =
        daemon::snapshot_via_socket_path(&socket_path, daemon::AutoStartPolicy::enabled_for_tests())
            .expect_err("startup_failed should be terminal");

    assert!(matches!(
        error,
        daemon::DaemonSnapshotError::StartupFailed { .. }
    ));
    assert!(error.to_string().contains("tmux server is unavailable"));
    assert_eq!(handle.join().expect("server should join"), 1);
}

#[test]
fn daemon_snapshot_helper_treats_server_closing_as_terminal() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let socket_path = tempdir.path().join("agentscan.sock");
    let handle = serve_snapshot_responses(
        &socket_path,
        vec![vec![
            hello_ack_frame(),
            ipc::DaemonFrame::Unavailable {
                reason: ipc::UnavailableReason::ServerClosing,
                message: "daemon is shutting down".to_string(),
            },
        ]],
    );

    let error =
        daemon::snapshot_via_socket_path(&socket_path, daemon::AutoStartPolicy::enabled_for_tests())
            .expect_err("server_closing should be terminal");

    assert_eq!(
        error,
        daemon::DaemonSnapshotError::ServerClosing {
            message: "daemon is shutting down".to_string()
        }
    );
    assert_eq!(handle.join().expect("server should join"), 1);
}

#[test]
fn daemon_snapshot_helper_reports_incompatible_daemon_guidance() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let socket_path = tempdir.path().join("agentscan.sock");
    let handle = serve_snapshot_responses(
        &socket_path,
        vec![vec![ipc::DaemonFrame::Shutdown {
            reason: ipc::ShutdownReason::ProtocolMismatch,
            message: "protocol 0 is unsupported".to_string(),
        }]],
    );

    let error =
        daemon::snapshot_via_socket_path(&socket_path, daemon::AutoStartPolicy::enabled_for_tests())
            .expect_err("protocol mismatch should be incompatible");

    assert!(matches!(
        error,
        daemon::DaemonSnapshotError::Incompatible { .. }
    ));
    assert!(
        error
            .to_string()
            .contains("stop the incompatible daemon manually")
    );
    assert_eq!(handle.join().expect("server should join"), 1);
}

#[test]
fn daemon_snapshot_helper_rejects_incompatible_hello_ack() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let socket_path = tempdir.path().join("agentscan.sock");
    let snapshot = empty_socket_snapshot("2026-05-03T00:00:00Z");
    let handle = serve_snapshot_responses(
        &socket_path,
        vec![vec![
            ipc::DaemonFrame::HelloAck {
                protocol_version: ipc::WIRE_PROTOCOL_VERSION + 1,
                snapshot_schema_version: CACHE_SCHEMA_VERSION,
            },
            ipc::DaemonFrame::Snapshot { seq: 1, snapshot },
        ]],
    );

    let error =
        daemon::snapshot_via_socket_path(&socket_path, daemon::AutoStartPolicy::enabled_for_tests())
            .expect_err("incompatible hello ack should fail");

    assert!(matches!(
        error,
        daemon::DaemonSnapshotError::Incompatible { .. }
    ));
    assert!(error.to_string().contains("incompatible snapshot handshake"));
    assert_eq!(handle.join().expect("server should join"), 1);
}

#[test]
fn daemon_snapshot_helper_rejects_invalid_snapshot_schema() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let socket_path = tempdir.path().join("agentscan.sock");
    let mut snapshot = empty_socket_snapshot("2026-05-03T00:00:00Z");
    snapshot.schema_version = CACHE_SCHEMA_VERSION + 1;
    let handle = serve_snapshot_responses(
        &socket_path,
        vec![vec![
            hello_ack_frame(),
            ipc::DaemonFrame::Snapshot { seq: 1, snapshot },
        ]],
    );

    let error =
        daemon::snapshot_via_socket_path(&socket_path, daemon::AutoStartPolicy::enabled_for_tests())
            .expect_err("invalid snapshot schema should fail");

    assert!(matches!(
        error,
        daemon::DaemonSnapshotError::Incompatible { .. }
    ));
    assert!(error.to_string().contains("invalid snapshot"));
    assert!(error.to_string().contains("unsupported snapshot schema version"));
    assert_eq!(handle.join().expect("server should join"), 1);
}

#[test]
fn daemon_snapshot_helper_reports_disabled_auto_start_when_socket_is_missing() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let socket_path = tempdir.path().join("missing.sock");

    let error =
        daemon::snapshot_via_socket_path(&socket_path, daemon::AutoStartPolicy::disabled_for_tests())
            .expect_err("missing socket with opt-out should not auto-start");

    assert_eq!(
        error,
        daemon::DaemonSnapshotError::AutoStartDisabled {
            reason: "socket is missing".to_string()
        }
    );
}

#[cfg(target_os = "macos")]
#[test]
fn daemon_snapshot_helper_blocks_untrusted_implicit_auto_start_on_macos() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let socket_path = tempdir.path().join("missing.sock");

    let error = daemon::snapshot_via_socket_path_with_start_command(
        &socket_path,
        Path::new("/tmp/agentscan-untrusted"),
        &[],
        &[],
    )
    .expect_err("missing socket should block untrusted macOS executable");

    assert!(matches!(
        error,
        daemon::DaemonSnapshotError::AutoStartDisabled { .. }
    ));
    assert!(error.to_string().contains("macOS executable trust preflight rejected"));
    assert!(error.to_string().contains("codesign inspection failed"));
}

#[cfg(target_os = "macos")]
#[test]
fn daemon_snapshot_helper_removes_stale_socket_before_untrusted_macos_refusal() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let socket_path = tempdir.path().join("stale.sock");
    {
        let _listener = UnixListener::bind(&socket_path).expect("stale socket should bind");
    }
    assert!(socket_path.exists());
    let deadline = Instant::now() + Duration::from_secs(1);
    while UnixStream::connect(&socket_path).is_ok() {
        assert!(
            Instant::now() < deadline,
            "closed test listener should stop accepting connections"
        );
        std::thread::sleep(Duration::from_millis(5));
    }

    let error = daemon::snapshot_via_socket_path_with_start_command(
        &socket_path,
        Path::new("/tmp/agentscan-untrusted"),
        &[],
        &[],
    )
    .expect_err("stale socket should block untrusted macOS executable");

    assert!(matches!(
        error,
        daemon::DaemonSnapshotError::AutoStartDisabled { .. }
    ));
    assert!(
        !socket_path.exists(),
        "macOS refusal should remove refused stale sockets so `agentscan daemon run` can bind"
    );
}

#[test]
fn transient_accept_errors_are_retried_not_fatal() {
    // Resource pressure must be retried so a momentary fd exhaustion does not leave
    // the daemon permanently deaf.
    for errno in [libc::EMFILE, libc::ENFILE, libc::ENOBUFS, libc::ENOMEM] {
        let error = std::io::Error::from_raw_os_error(errno);
        assert!(
            daemon::is_transient_accept_error(&error),
            "errno {errno} should be treated as transient"
        );
    }
    assert!(
        daemon::is_transient_accept_error(&std::io::Error::from(
            std::io::ErrorKind::ConnectionAborted
        )),
        "a peer that aborted before accept should be transient"
    );
}

#[test]
fn broken_listener_accept_errors_are_fatal() {
    // A structurally broken listener cannot recover; it must escalate to shutdown.
    for errno in [libc::EBADF, libc::EINVAL, libc::ENOTSOCK] {
        let error = std::io::Error::from_raw_os_error(errno);
        assert!(
            !daemon::is_transient_accept_error(&error),
            "errno {errno} should be treated as fatal"
        );
    }
}
