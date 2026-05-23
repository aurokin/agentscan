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
                snapshot: snapshot.clone(),
            },
            serde_json::json!({
                "type": "snapshot",
                "snapshot": snapshot,
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
