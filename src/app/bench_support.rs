use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;

use super::*;

#[derive(Clone)]
pub struct BenchPaneRows(Vec<TmuxPaneRow>);

pub struct BenchPaneRecords(Vec<PaneRecord>);

pub fn parse_pane_rows(input: &str) -> Result<BenchPaneRows> {
    tmux::parse_pane_rows(input).map(BenchPaneRows)
}

pub fn pane_records_from_rows(rows: BenchPaneRows) -> BenchPaneRecords {
    BenchPaneRecords(rows.0.into_iter().map(classify::pane_from_row).collect())
}

pub fn tui_rendered_row_count(records: &BenchPaneRecords) -> usize {
    let mut key_targets = BTreeMap::new();
    tui::synchronize_key_targets(&mut key_targets, &records.0);
    tui::render_rows(&records.0, &key_targets).len()
}

pub fn deserialize_snapshot_pane_count(input: &str) -> Result<usize> {
    let snapshot: SnapshotEnvelope =
        serde_json::from_str(input).context("cache fixture should deserialize")?;
    Ok(snapshot.panes.len())
}

#[doc(hidden)]
pub fn daemon_protocol_version_for_tests() -> u32 {
    ipc::WIRE_PROTOCOL_VERSION
}

#[doc(hidden)]
pub fn snapshot_schema_version_for_tests() -> u32 {
    CACHE_SCHEMA_VERSION
}

#[doc(hidden)]
pub fn validate_snapshot_json_for_tests(input: &str) -> Result<()> {
    let snapshot: SnapshotEnvelope =
        serde_json::from_str(input).context("snapshot should deserialize")?;
    snapshot::validate_snapshot(&snapshot).map(|_| ())
}

#[doc(hidden)]
pub fn daemon_snapshot_via_socket_path_for_tests(
    socket_path: &Path,
    executable_path: &Path,
    envs: &[(OsString, OsString)],
    env_removes: &[OsString],
) -> Result<String> {
    let snapshot = daemon::snapshot_via_socket_path_with_start_command(
        socket_path,
        executable_path,
        envs,
        env_removes,
    )
    .map_err(anyhow::Error::new)?;
    serde_json::to_string(&snapshot).context("failed to serialize daemon snapshot")
}
