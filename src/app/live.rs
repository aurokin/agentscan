use super::*;

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum LiveClientEvent {
    Connecting {
        message: String,
    },
    // The subscribe stream carries picker `rows` alongside the `snapshot` so a
    // consumer (the desktop) can render the picker directly from the delivered
    // frame instead of spawning a second `agentscan hotkeys` scan per update. The
    // rows are built on the tmux-owning host — with live focus/client resolution
    // and host-local workspace grouping — which a remote client could not
    // reproduce from the snapshot alone.
    // `snapshot` is boxed to keep this large variant from bloating the event enum
    // (and the channel/Result types that carry it) now that it also holds `rows`.
    // A boxed value serializes identically, so the stream shape is unchanged.
    Snapshot {
        snapshot: Box<SnapshotEnvelope>,
        rows: Vec<picker::PickerRow>,
    },
    Offline {
        message: String,
        retrying: bool,
    },
    Shutdown {
        message: String,
    },
    Fatal {
        message: String,
    },
}
