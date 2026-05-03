# AUR-176 Issue Plan: Migrate One-Shot Commands to Socket Snapshots

## Scope

Move the one-shot command surfaces to daemon socket snapshots by default:

- bare `agentscan`
- `agentscan list`
- `agentscan inspect <pane_id>`
- `agentscan focus <pane_id>`
- new `agentscan snapshot`

Preserve direct tmux bypass with `--refresh`, keep `agentscan scan` daemon-free, and remove the legacy `agentscan cache show` structured snapshot route.

## Non-Goals

- Do not migrate `agentscan tui`; TUI daemon subscription belongs to AUR-177.
- Do not remove all cache internals or cache diagnostics; cache cleanup is later migration work.
- Do not change daemon socket wire format or snapshot schema.
- Do not add provider detection heuristics.
- Do not push changes; the milestone still stays local until complete.

## Implementation Outline

1. Add the `snapshot` command.
   - Add `Commands::Snapshot(SnapshotArgs)`.
   - `SnapshotArgs` should include `RefreshArgs`, `AutoStartArgs`, and `OutputFormat`.
   - Merge root `--refresh`, `--format`, and `--no-auto-start` into explicit `snapshot`.
   - Reject root `--all` for `snapshot`; raw snapshot envelopes are unfiltered.
   - Emit the raw `SnapshotEnvelope`; JSON should preserve the same envelope shape currently exposed by `cache show --format json`.
   - Do not reuse cache-branded text output for `snapshot`; add a neutral snapshot summary text helper if text mode is retained.
   - Treat `snapshot --format json` as the stable structured contract; text output is human-facing only.

2. Centralize one-shot snapshot loading.
   - Add a helper in command code such as:
     - `snapshot_for_consumer(refresh: RefreshArgs, auto_start: AutoStartArgs) -> Result<SnapshotEnvelope>`
   - If `refresh` is true, bypass the daemon and use direct tmux scanning via a clearly named helper that only calls `scanner::snapshot_from_tmux()`.
   - Migrated one-shot `--refresh` paths must not call `cache::refresh_cache_from_tmux()`, `cache::load_snapshot(true)`, or otherwise write/update the cache.
   - If `refresh` is false, call `daemon::snapshot_via_socket(AutoStartPolicy::from_args(auto_start))`.
   - Convert `DaemonSnapshotError` through its existing `Display`/`anyhow` path so command errors preserve opt-out, incompatible, startup failed, child exit, timeout, busy, and closing distinctions.
   - Keep filtering in `list` only; `inspect`, `focus`, and `snapshot` should operate on the full envelope they load.

3. Migrate default one-shot behavior.
   - Bare `agentscan` and `agentscan list` should use daemon snapshots by default and preserve text/json output.
   - `inspect` should use daemon snapshots by default and keep JSON/text pane output.
   - `focus` should use daemon snapshots by default to validate pane existence before focusing; with `--refresh`, validate against a fresh direct tmux scan.
   - Keep pane-not-found wording honest:
     - daemon/default path: "daemon snapshot"
     - refresh path: "fresh tmux snapshot"
   - Keep `--no-auto-start` effective on daemon-backed default paths and irrelevant on `--refresh` direct paths.

4. Keep `scan` direct and cache-free.
   - `command_scan` should call `scanner::snapshot_from_tmux()` regardless of `--refresh`.
   - Keep root arg merging and output behavior for compatibility.
   - Add tests proving `scan` does not use the daemon socket, does not auto-start, and does not write/update the cache.

5. Replace legacy `cache show` routing.
   - Remove `CacheCommands::Show` and `CacheShowArgs`.
   - Update TUI/root-format guidance that points users to `cache show --format json`; it should point to `snapshot --format json`.
   - Keep `cache path` and `cache validate` unless implementation requires otherwise.
   - Existing cache-show integration tests should migrate to `snapshot` or be removed if they only cover legacy routing.
   - Leave `cache validate --refresh` cache-specific unless implementation evidence shows it should change; this issue changes one-shot consumer refresh semantics, not cache maintenance semantics.

6. Keep documentation narrowly aligned.
   - Update README/ROADMAP lines that still describe the pre-AUR-176 branch as cache-backed for one-shot commands.
   - Update `RefreshArgs` help text so it no longer promises cache rewrites for migrated one-shot commands or `scan`.
   - Do not do the full daemon migration docs pass; AUR-180 owns durable docs/release notes.

## Edge Cases

- Missing daemon socket with auto-start enabled starts the daemon and returns one snapshot.
- Missing daemon socket with `--no-auto-start` or `AGENTSCAN_NO_AUTO_START=1` fails through the structured opt-out error.
- `--refresh --no-auto-start` succeeds without daemon contact because refresh is a direct tmux bypass.
- `--refresh` on migrated one-shot commands and `scan` leaves any existing cache file unchanged.
- `startup_failed` and `server_closing` frames remain terminal and must not start a replacement.
- `scan` must remain daemon-free even if root `--no-auto-start`, daemon socket env, or cache env are set.
- `snapshot --format json` must not filter panes; `list` should keep its existing filtered-by-default behavior.
- Default daemon-backed list/inspect/focus/snapshot should continue to work when the cache is stale, poisoned, or missing, proving no silent cache fallback.
- `focus` still needs tmux focus behavior after snapshot validation; stale daemon snapshots can still race with missing panes, so existing focus error handling remains necessary.
- `cache validate` still reads the cache file; this issue does not make it socket-backed.

## Test Plan

Focused tests:

- `cargo test snapshot`
- `cargo test auto_start`
- `cargo test --test daemon_integration one_shot`
- `cargo test --test daemon_integration snapshot`
- `cargo test --test daemon_integration scan`

Required coverage:

- bare `agentscan` reads daemon socket snapshot by default
- `list` reads daemon socket snapshot by default and preserves text/json output
- `inspect` reads daemon socket snapshot by default
- `focus` validates pane existence with daemon snapshot by default
- `snapshot --format json` emits the raw daemon snapshot envelope
- `list --refresh`, `inspect --refresh`, `focus --refresh`, and `snapshot --refresh` bypass daemon/socket auto-start and use direct tmux
- migrated one-shot `--refresh` paths and `scan --refresh` leave cache contents or mtime unchanged when a cache file already exists
- default daemon-backed list/inspect/focus/snapshot ignore poisoned or stale cache contents when the socket has a valid snapshot
- `--no-auto-start` and `AGENTSCAN_NO_AUTO_START=1` fail clearly when daemon is missing on default daemon-backed one-shot commands
- `scan` never connects to or starts the daemon and does not write/update the cache when `--refresh` is present
- `cache show` is no longer accepted, and guidance points to `snapshot`
- root argument merging remains compatible for list-like commands and rejects unsupported root args for non-daemon families

Regression gates:

- `cargo fmt --all --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_arguments`
- `cargo test`

## Documentation Impact

Update only the short README/ROADMAP/help statements made stale by this issue: default one-shot commands become daemon-backed, `snapshot` replaces `cache show`, refresh is a direct tmux bypass, and `scan --refresh` no longer writes the cache. Full migration docs remain AUR-180.

## Plan Review Notes

Plan review required these refinements before implementation:

- Migrated one-shot `--refresh` paths must direct-scan without cache writes, not use cache-era refresh helpers.
- `snapshot` text output must not inherit cache-branded diagnostics; JSON is the stable raw-envelope contract.
- Update `RefreshArgs` help text because it currently says refresh rewrites the cache.
- Add tests with poisoned/stale cache plus valid daemon socket snapshots to prove default one-shot commands are truly socket-backed.
- Add cache content or mtime assertions for `scan --refresh` and migrated one-shot refresh paths.
