use std::collections::BTreeMap;

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

pub fn popup_rendered_row_count(records: &BenchPaneRecords) -> usize {
    let mut key_targets = BTreeMap::new();
    popup_ui::synchronize_key_targets(&mut key_targets, &records.0);
    popup_ui::render_rows(&records.0, &key_targets).len()
}

pub fn deserialize_snapshot_pane_count(input: &str) -> Result<usize> {
    let snapshot: SnapshotEnvelope =
        serde_json::from_str(input).context("cache fixture should deserialize")?;
    Ok(snapshot.panes.len())
}
