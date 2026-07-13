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

fn bench_control_event_output_firehose(c: &mut Criterion) {
    // Simulate a high-throughput agent pane: a 500-line `%output` burst with no
    // terminal-title escape and no metadata change (the worst-case firehose batch).
    let lines: Vec<String> = (0..500)
        .map(|index| format!("%output %1 streaming token chunk number {index} with some payload"))
        .collect();

    c.bench_function("control_event_batch/output_firehose_500", |b| {
        b.iter(|| app_bench::control_event_batch_volume(&lines))
    });
}

fn bench_snapshots_materially_equal(c: &mut Criterion) {
    // Worst case for the daemon hot path: two materially-equal multi-pane
    // snapshots, forcing a full field-wise traversal on every tick.
    let rows =
        app_bench::parse_pane_rows(TMUX_SNAPSHOT_FIXTURE).expect("fixture snapshot should parse");
    let left = app_bench::snapshot_from_pane_rows(rows.clone());
    let right = app_bench::clone_bench_snapshot(&left);

    c.bench_function("snapshots_materially_equal/fixture_snapshot", |b| {
        b.iter(|| app_bench::snapshots_are_materially_equal(&left, &right))
    });
}

fn bench_encode_snapshot_frame(c: &mut Criterion) {
    let rows =
        app_bench::parse_pane_rows(TMUX_SNAPSHOT_FIXTURE).expect("fixture snapshot should parse");
    let snapshot = app_bench::snapshot_from_pane_rows(rows);

    c.bench_function("encode_snapshot_frame/fixture_snapshot", |b| {
        b.iter(|| {
            app_bench::encode_snapshot_frame_bytes(&snapshot)
                .expect("fixture snapshot should encode")
        })
    });
}

fn bench_tui_render_rows(c: &mut Criterion) {
    let panes =
        app_bench::parse_pane_rows(TMUX_SNAPSHOT_FIXTURE).expect("fixture snapshot should parse");
    let panes = app_bench::pane_records_from_rows(panes);

    c.bench_function("tui_render_rows/fixture_snapshot", |b| {
        b.iter(|| app_bench::tui_rendered_row_count(&panes))
    });
}

criterion_group!(
    benches,
    bench_parse_pane_rows,
    bench_pane_from_row,
    bench_cache_deserialize,
    bench_control_event_output_firehose,
    bench_snapshots_materially_equal,
    bench_encode_snapshot_frame,
    bench_tui_render_rows
);
criterion_main!(benches);
