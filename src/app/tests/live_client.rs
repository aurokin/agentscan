#[test]
fn live_client_events_serialize_to_stream_contract_shape() {
    let snapshot = empty_socket_snapshot("2026-05-23T17:00:00Z");
    let cases = [
        (
            LiveClientEvent::Connecting {
                message: "connecting".to_string(),
            },
            serde_json::json!({
                "type": "connecting",
                "message": "connecting",
            }),
        ),
        (
            LiveClientEvent::Snapshot {
                snapshot: Box::new(snapshot.clone()),
                rows: Vec::new(),
            },
            serde_json::json!({
                "type": "snapshot",
                "snapshot": snapshot,
                "rows": [],
            }),
        ),
        (
            LiveClientEvent::Offline {
                message: "offline".to_string(),
                retrying: true,
            },
            serde_json::json!({
                "type": "offline",
                "message": "offline",
                "retrying": true,
            }),
        ),
        (
            LiveClientEvent::Shutdown {
                message: "shutdown".to_string(),
            },
            serde_json::json!({
                "type": "shutdown",
                "message": "shutdown",
            }),
        ),
        (
            LiveClientEvent::Fatal {
                message: "fatal".to_string(),
            },
            serde_json::json!({
                "type": "fatal",
                "message": "fatal",
            }),
        ),
    ];

    for (event, expected) in cases {
        assert_eq!(
            serde_json::to_value(event).expect("live client event should serialize"),
            expected
        );
    }
}

#[test]
fn subscription_keepalive_frame_matches_stream_contract_shape() {
    use std::sync::atomic::AtomicBool;

    // The heartbeat frame must be a complete, type-tagged JSON line so consumers that switch on
    // the frame type ignore it (the same `type` convention as every other stream frame).
    let mut buffer: Vec<u8> = Vec::new();
    let cancel = AtomicBool::new(false);
    let ok = daemon::test_write_subscription_keepalive(&mut buffer, &cancel)
        .expect("keepalive write should succeed");
    assert!(ok, "an open writer should keep the stream alive");
    assert_eq!(buffer, b"{\"type\":\"keepalive\"}\n");
    let frame: serde_json::Value =
        serde_json::from_slice(&buffer).expect("keepalive frame should be valid JSON");
    assert_eq!(frame, serde_json::json!({ "type": "keepalive" }));
    assert!(!cancel.load(std::sync::atomic::Ordering::Relaxed));
}

#[test]
fn subscription_keepalive_reports_closed_consumer_on_broken_pipe() {
    use std::sync::atomic::{AtomicBool, Ordering};

    struct BrokenPipeWriter;
    impl std::io::Write for BrokenPipeWriter {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    // A closed consumer (broken pipe) is how the writer learns to exit when no real events flow.
    let cancel = AtomicBool::new(false);
    let ok = daemon::test_write_subscription_keepalive(&mut BrokenPipeWriter, &cancel)
        .expect("broken pipe is reported via the bool, not an error");
    assert!(!ok, "a broken pipe should signal the stream is closed");
    assert!(
        cancel.load(Ordering::Relaxed),
        "a broken pipe should cancel the subscription worker"
    );
}
