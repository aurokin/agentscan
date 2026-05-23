use super::*;

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum LiveClientEvent {
    Connecting { message: String },
    Snapshot { snapshot: SnapshotEnvelope },
    Offline { message: String, retrying: bool },
    Shutdown { message: String },
    Fatal { message: String },
}
