use agentscan::app::bench_support as app_bench;
use criterion::{Criterion, criterion_group, criterion_main};

const TMUX_SNAPSHOT_FIXTURE: &str = include_str!("../tests/fixtures/tmux_snapshot_titles.txt");
const CACHE_SNAPSHOT_FIXTURE: &str = include_str!("../tests/fixtures/cache_snapshot_v1.json");

fn bench_parse_pane_rows(c: &mut Criterion) {
    c.bench_function("parse_pane_rows/fixture_snapshot", |b| {
        b.iter(|| {
            app_bench::parse_pane_rows(TMUX_SNAPSHOT_FIXTURE)
                .expect("fixture snapshot should parse")
        })
    });
}

fn bench_pane_from_row(c: &mut Criterion) {
    let rows =
        app_bench::parse_pane_rows(TMUX_SNAPSHOT_FIXTURE).expect("fixture snapshot should parse");

    c.bench_function("pane_from_row/fixture_snapshot", |b| {
        b.iter(|| app_bench::pane_records_from_rows(rows.clone()))
    });
}

fn bench_cache_deserialize(c: &mut Criterion) {
    c.bench_function("cache_deserialize/current_schema", |b| {
        b.iter(|| {
            app_bench::deserialize_snapshot_pane_count(CACHE_SNAPSHOT_FIXTURE)
                .expect("cache fixture should deserialize")
        })
    });
}

fn bench_popup_entries(c: &mut Criterion) {
    let panes =
        app_bench::parse_pane_rows(TMUX_SNAPSHOT_FIXTURE).expect("fixture snapshot should parse");
    let panes = app_bench::pane_records_from_rows(panes);

    c.bench_function("popup_entries/fixture_snapshot", |b| {
        b.iter(|| app_bench::popup_entry_count(&panes))
    });
}

criterion_group!(
    benches,
    bench_parse_pane_rows,
    bench_pane_from_row,
    bench_cache_deserialize,
    bench_popup_entries
);
criterion_main!(benches);
