# AUR-179 Issue Plan: Remove Cache Transport And Obsolete Cache Surfaces

## Scope

Remove the persisted cache file as a live transport and delete the user-facing
cache command surface now that daemon-backed consumers and integration tests use
the daemon socket.

This issue targets product behavior, tests, and narrow migration docs:

- daemon startup and update paths that still write cache files
- `agentscan cache path` and `agentscan cache validate`
- `AGENTSCAN_CACHE_PATH` as a supported runtime/test harness input
- cache diagnostics and timestamp-preservation behavior that only existed for
  the migration cache
- tests that only prove cache preservation, cache diagnostics, or cache command
  removal behavior

## Non-Goals

- Do not change the daemon socket wire protocol or snapshot JSON schema unless a
  field is proven to be cache-only and safely removable in this slice.
- Do not remove `scan --refresh` / direct tmux recovery behavior for one-shot
  commands.
- Do not weaken daemon socket, TUI, or focus end-to-end coverage.
- Do not rewrite all documentation; AUR-180 owns the final docs/release notes
  reconciliation.
- Do not push changes; the milestone remains local until complete.

## Implementation Outline

1. Remove the CLI cache surface.
   - Delete `Commands::Cache`, `CacheArgs`, `CacheCommands`, and
     `CacheValidateArgs`.
   - Remove `command_cache` and cache-specific output functions.
   - Replace stale cache-command tests with negative coverage proving
     `agentscan cache`, `cache path`, `cache validate`, and the already-removed
     `cache show` are rejected.
   - Keep root-argument rejection tests for supported non-cache commands.

2. Remove persisted cache writes and fallback reads safely.
   - First delete runtime file read/write/path APIs and compile out cache
     transport code while the harness still supplies isolated cache env vars.
   - Remove daemon initial cache publication and later
     `write_snapshot_to_cache` calls.
   - Simplify `StartupActions` so startup readiness depends on socket snapshot
     publication, not cache publication.
   - Remove metadata helper cache refresh side effects from `tmux
     set-metadata` and `tmux clear-metadata`; daemon/socket updates remain the
     live state path.
   - Only after file write paths are gone, remove `AGENTSCAN_CACHE_PATH` from
     harness subprocess environments and TUI command strings.
   - During the transition, keep `XDG_CACHE_HOME` and `HOME` pointed at the
     harness tempdir and add no-cache-file assertions so any missed cache write
     cannot leak to the user's default cache path.
   - Delete tests whose only purpose was cache write preservation, cache
     diagnostics, or metadata helper cache refresh.

3. Move surviving snapshot helpers out of the cache contract.
   - Keep shared helpers for snapshot validation, summary, filtering, sorting,
     timestamping, and daemon provenance.
   - Prefer renaming the internal `cache` module/types to snapshot-oriented
     names where that reduces durable cache vocabulary without causing
     unnecessary churn.
   - Remove persisted-cache-only helpers such as cache path resolution, cache
     file read/write, stale-cache diagnostics, daemon cache status, and last
     daemon refresh preservation.
   - Keep fixture names only when they describe historical fixture files; product
     code should no longer expose cache file paths or cache health.

4. Update tests to prove cache transport is gone.
   - Convert poisoned-cache one-shot tests into socket-only tests that do not set
     `AGENTSCAN_CACHE_PATH`.
   - Keep direct `--refresh` tests, but assert they bypass daemon/socket state
     rather than asserting cache files were preserved.
   - Update daemon integration harness fields and helpers to remove `cache_path`
     except where a deleted test file still needs fixture-local data during the
     same commit.
   - Add a mechanical `rg` audit before completion for `AGENTSCAN_CACHE_PATH`,
     `cache path`, `cache validate`, `write_snapshot_to_cache`,
     `read_snapshot_from_cache`, `wait_for_cache`, `CacheDiagnostics`, and
     `daemon_cache_status`.
   - Include durable-name audit decisions for `CACHE_ENV_VAR`,
     `CACHE_RELATIVE_PATH`, `cache show`, `cache_origin`,
     `daemon_generated_at`, user-visible "cache schema" errors, and broad
     `cache::` product references.

5. Keep docs narrowly consistent for this slice.
   - Update `README.md`, `ROADMAP.md`, and `docs/harness-engineering.md` only
     where they currently claim a remaining cache surface or cache transport.
   - Leave broad release-note and architecture cleanup for AUR-180, but do not
     leave docs saying users can run removed commands.

## Edge Cases

- Daemon startup must still surface initial snapshot and tmux control-mode
  startup failures through the socket startup state.
- Removing cache publication must not make daemon readiness visible before the
  first socket snapshot is encoded and published.
- Metadata helper commands should remain useful for wrappers even when no daemon
  is running; they update tmux metadata only, and daemon/socket readers observe
  it on the next snapshot.
- Metadata helper coverage must include live targeted updates, unrelated daemon
  updates, and at least one full-reconcile path without relying on cached pane
  merges.
- `--refresh` remains a direct tmux bypass for supported one-shot commands and
  must not attempt daemon socket or cache access.
- TUI still has no `--refresh`, no `--format`, and no direct tmux/cache fallback.
- Existing fixture JSON may contain `diagnostics.cache_origin`; avoid snapshot
  schema churn unless all producer/consumer tests are updated deliberately.
- User-visible schema validation errors should say "snapshot schema" instead of
  "cache schema" if the validation helper survives cache removal.

## Test Plan

Focused tests:

- `cargo test --test daemon_integration daemon_ -- --nocapture`
- `cargo test --test daemon_integration one_shot -- --nocapture`
- `cargo test --test daemon_integration tui_ -- --nocapture`
- `cargo test --test daemon_status_integration`
- `cargo test daemon_socket -- --nocapture`
- targeted CLI parsing tests for removed `cache` commands
- a daemon metadata full-reconcile test proving wrapper metadata survives without
  cached-pane merging
- a no-cache-file guard around daemon, one-shot, TUI, and metadata-helper
  integration paths

Regression gates:

- `cargo fmt --all --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_arguments`
- `cargo test`

## Documentation Impact

Make repo docs stop advertising the cache file as a remaining migration surface.
Document that daemon state is served over the socket and direct snapshots use
`scan`/`--refresh`; leave final narrative/release-note polish to AUR-180.

## Plan Review Notes

Plan review required these refinements before implementation:

- Avoid test environment leakage by deleting file transport before removing
  `AGENTSCAN_CACHE_PATH`; during the transition, set `XDG_CACHE_HOME` and `HOME`
  to harness temp paths and assert no cache file is created.
- Keep negative CLI coverage for the removed `cache` command family instead of
  simply deleting all cache-command tests.
- Add a full-reconcile metadata preservation test because cached pane merging is
  being removed.
- Audit durable cache vocabulary explicitly and decide what remains for snapshot
  schema compatibility versus what should be renamed or removed in this slice.
