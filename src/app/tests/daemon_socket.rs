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

fn encoded_snapshot_frame(snapshot: &SnapshotEnvelope) -> daemon::EncodedDaemonFrame {
    let encoded = ipc::encode_frame(&ipc::DaemonFrame::Snapshot {
        snapshot: snapshot.clone(),
    })
    .expect("snapshot frame should encode");
    std::sync::Arc::<[u8]>::from(encoded)
}

fn socket_hello(mode: ipc::ClientMode) -> ipc::ClientFrame {
    ipc::ClientFrame::Hello {
        protocol_version: ipc::WIRE_PROTOCOL_VERSION,
        snapshot_schema_version: CACHE_SCHEMA_VERSION,
        mode,
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
    assert_eq!(
        subscriber.read_frame(),
        ipc::DaemonFrame::Snapshot {
            snapshot: expected_snapshot
        }
    );
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
                stream
                    .write_all(&ipc::encode_frame(&frame).expect("frame should encode"))
                    .expect("daemon response should write");
            }
            accepted += 1;
        }
        accepted
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
    cache_publication_error: Option<&'static str>,
}

impl FakeDaemonStartup {
    fn ready() -> Self {
        Self {
            initial_snapshot: FakeInitialSnapshot::Ready,
            tmux_startup: FakeTmuxStartup::Started,
            cache_publication_error: None,
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

    fn with_cache_publication_error(message: &'static str) -> Self {
        Self {
            cache_publication_error: Some(message),
            ..Self::ready()
        }
    }
}

impl daemon::StartupActions for FakeDaemonStartup {
    fn initial_snapshot(&self) -> anyhow::Result<SnapshotEnvelope> {
        match self.initial_snapshot {
            FakeInitialSnapshot::Ready => Ok(empty_socket_snapshot("2026-05-03T00:00:00Z")),
            FakeInitialSnapshot::Oversized => {
                let mut snapshot = empty_socket_snapshot("2026-05-03T00:00:00Z");
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

    fn publish_initial_cache_snapshot(&self, _snapshot: &SnapshotEnvelope) -> anyhow::Result<()> {
        match self.cache_publication_error {
            Some(message) => anyhow::bail!("{message}"),
            None => Ok(()),
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
            ipc::DaemonFrame::Snapshot { snapshot },
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
            assert_eq!(status.unavailable_reason, None);
        }
        other => panic!("expected lifecycle status frames, got {other:?}"),
    }
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
    state.publish_later_snapshot(updated.clone());

    assert_eq!(
        subscriber.read_frame(),
        ipc::DaemonFrame::Snapshot { snapshot: updated }
    );
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
    state.publish_later_snapshot(oversized);

    let frames = exchange_daemon_frames(state, socket_hello(ipc::ClientMode::Snapshot));

    assert_eq!(
        frames,
        vec![
            ipc::DaemonFrame::HelloAck {
                protocol_version: ipc::WIRE_PROTOCOL_VERSION,
                snapshot_schema_version: CACHE_SCHEMA_VERSION,
            },
            ipc::DaemonFrame::Snapshot { snapshot: initial },
        ]
    );
}

#[test]
fn daemon_socket_subscriber_mailbox_is_latest_wins() {
    let mailbox = daemon::SubscriberMailbox::new();
    let older = encoded_snapshot_frame(&empty_socket_snapshot("2026-05-03T00:00:00Z"));
    let newer = encoded_snapshot_frame(&empty_socket_snapshot("2026-05-03T00:00:01Z"));

    mailbox.enqueue(older);
    mailbox.enqueue(newer.clone());

    assert_eq!(
        mailbox
            .try_take_pending()
            .expect("mailbox should have pending frame")
            .as_ref(),
        newer.as_ref()
    );
    mailbox.close();
    assert!(mailbox.is_closed());
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
    state.publish_later_snapshot(oversized);

    let next_good = empty_socket_snapshot("2026-05-03T00:00:02Z");
    state.publish_later_snapshot(next_good.clone());
    assert_eq!(
        subscriber.read_frame(),
        ipc::DaemonFrame::Snapshot {
            snapshot: next_good
        }
    );
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
fn daemon_socket_initial_cache_publication_failure_blocks_socket_readiness() {
    assert_startup_failed_contains(
        FakeDaemonStartup::with_cache_publication_error("cache write failed"),
        "cache write failed",
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
            ipc::DaemonFrame::Snapshot { snapshot },
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
