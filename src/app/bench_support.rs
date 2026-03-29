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

pub fn popup_entry_count(records: &BenchPaneRecords) -> usize {
    cache::popup_entries(&records.0).len()
}

pub fn deserialize_snapshot_pane_count(input: &str) -> Result<usize> {
    let snapshot: SnapshotEnvelope =
        serde_json::from_str(input).context("cache fixture should deserialize")?;
    Ok(snapshot.panes.len())
}
