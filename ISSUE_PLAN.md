# AUR-178 Issue Plan: Rewrite Cache-Dependent Integration Harnesses

## Scope

Replace remaining cache-as-transport test dependencies with daemon/socket-aware
fixtures so later cache removal does not erase daemon, command, or TUI coverage.

This issue targets tests and harnesses, not product behavior:

- daemon readiness checks that currently wait for cache writes
- daemon pane-state assertions that currently inspect the cache file
- one-shot/TUI tests that poison or seed cache only to prove socket behavior
- reusable fixtures for fake socket servers and real daemon harnesses

## Non-Goals

- Do not remove `agentscan cache validate`, cache diagnostics, or cache-specific
  unit/integration tests that still intentionally cover the cache surface.
- Do not change snapshot schema or daemon wire protocol.
- Do not remove daemon cache writes from product code; cache transport removal is
  later milestone work.
- Do not reduce real tmux end-to-end coverage for daemon lifecycle, command
  behavior, focus, or TUI workflows.
- Do not push changes; the milestone remains local until complete.

## Implementation Outline

1. Classify current cache dependencies.
   - Keep cache-specific coverage in `tests/cache_validate_integration.rs`,
     `tests/cache_show_integration.rs`, and cache unit tests.
   - Treat `tests/daemon_integration.rs` waits/assertions on daemon cache files
     as migration artifacts unless the test is explicitly about cache
     diagnostics or cache refresh semantics.
   - Include `tests/daemon_status_integration.rs` in the audit even though it is
     already cache-independent, so daemon status coverage does not regress.
   - Identify all uses of `wait_for_cache`, `wait_for_cache_file`,
     `wait_for_pane`, `pane_from_cache`, direct `harness.cache_path` fixture
     writes, and `CACHE_SNAPSHOT_FIXTURE` in daemon-backed command/TUI tests.
   - Define and enforce a post-conversion allowlist:
     - allowed: cache path/schema/diagnostic unit tests
     - allowed: `tests/cache_validate_integration.rs`
     - allowed: `tests/cache_show_integration.rs` while removal behavior is
       still asserted
     - allowed: metadata helper cache-refresh tests while metadata helpers still
       intentionally update the migration cache
     - allowed: `cache_validate_refresh_preserves_last_daemon_refresh_semantics`
     - allowed: `scan_refresh_preserves_existing_daemon_cache`
     - allowed: poisoned/missing cache guard tests only when the expected data is
       served by socket/daemon state
     - not allowed: daemon readiness, daemon pane state, one-shot command
       routing, or TUI success predicates that depend on cache writes

2. Add reusable socket snapshot helpers to the integration harness.
   - Add a helper that reads one daemon socket snapshot using
     `ClientMode::Snapshot`, sends `ipc::WIRE_PROTOCOL_VERSION` and
     `CACHE_SCHEMA_VERSION` directly, validates `hello_ack`, validates the
     snapshot, and returns a typed `SnapshotEnvelope`.
   - Add `wait_for_daemon_snapshot(&mut daemon, predicate)` and
     `wait_for_daemon_pane(...)` helpers that poll the socket rather than the
     cache file.
   - Timeout errors should include daemon stdout/stderr, socket path, last
     socket frame/error, and a summary of the last snapshot.
   - Keep `AGENTSCAN_SOCKET_PATH` isolation through the existing harness socket
     path.
   - Use socket helpers for daemon readiness instead of waiting for cache writes
     whenever the test is not cache-specific.
   - For repeated live update tests, prefer a subscription-based helper or
     latest-snapshot wait when that reduces connection churn; fresh one-shot
     snapshot polling is acceptable for low-frequency readiness checks.

3. Reuse or extract fake socket server fixtures for one-shot command tests.
   - Consolidate the current fake snapshot daemon logic used by one-shot tests
     into a reusable helper with explicit socket path, frame sequence, and
     assertion hooks.
   - Fake socket fixtures must have bounded accept/read timeouts, capture client
     requests, join on drop, and fail on unexpected missing or extra connections
     unless the test explicitly expects retries/reconnects.
   - Prefer fake socket servers for command routing tests that only need to prove
     a CLI command reads the socket and ignores poisoned cache.
   - Move unit-ish protocol/lifecycle cases into in-process or fake-socket tests
     when they do not need tmux subprocess behavior.
   - Keep real daemon/tmux subprocess tests where end-to-end daemon discovery,
     auto-start, focus, display-popup, or subscription behavior matters.

4. Convert daemon-backed integration tests away from cache transport.
   - Replace readiness checks such as `wait_for_cache(... pane exists ...)` with
     socket snapshot waits.
   - Replace daemon update assertions that inspect cache JSON with socket
     snapshot assertions where the intended behavior is daemon state updates.
   - Absence checks must first observe the pane/window/session present after
     daemon startup, then observe absence after the tmux mutation. Prefer
     comparing a newer `generated_at` or otherwise proving a post-mutation
     snapshot was observed.
   - Preserve dedicated cache tests:
     - `cache validate` diagnostics and max-age behavior
     - metadata helper cache refresh behavior while those helpers still write the
       migration cache
     - `scan_refresh_preserves_existing_daemon_cache`
     - cache path and schema validation unit tests
   - Rename helper/test names that say "cache" when the assertion becomes
     socket-state based.

5. Prove daemon-backed commands/TUI do not depend on cache writes.
   - Keep or strengthen poisoned-cache tests for list/inspect/focus/snapshot and
     TUI, but serve expected data from fake socket or real daemon state.
   - Add a guard test or harness assertion that socket-backed command/TUI tests
     can pass with a missing or invalid cache file.
   - Avoid using cache mtime/content as the success signal except in tests whose
     purpose is cache behavior.
   - Add a mechanical `rg`/allowlist check as part of implementation review for
     `wait_for_cache`, `wait_for_cache_file`, `wait_for_pane`,
     `pane_from_cache`, `CACHE_SNAPSHOT_FIXTURE`, direct `harness.cache_path`
     writes, and `AGENTSCAN_CACHE_PATH`.

6. Keep docs/contracts aligned.
   - Update `docs/harness-engineering.md` if new helpers define a test contract
     around socket snapshot waits, fake socket servers, or daemon isolation.
   - Avoid broad user-facing docs changes unless test contracts clarify
     migration assumptions.

## Edge Cases

- Socket snapshot wait helpers must treat `DaemonNotReady`, `ServerBusy`, and
  startup races as retryable until the test deadline.
- Helpers must validate `hello_ack` protocol/schema before trusting snapshots.
- Socket helper hello frames must use shared protocol/schema constants directly,
  never values read from cache JSON.
- Absence predicates must not pass on an initial empty/stale snapshot; tests need
  a present-before-absent sequence or equivalent post-mutation proof.
- Tests that start real daemon subprocesses must keep `AGENTSCAN_SOCKET_PATH`,
  `AGENTSCAN_TMUX_SOCKET`, `AGENTSCAN_CACHE_PATH`, `TMUX_TMPDIR`, and `TMUX`
  isolation explicit.
- Auto-started TUI tests should either stop the daemon explicitly or document why
  harness tmux/server teardown is sufficient for the process lifecycle.
- Fake socket helpers must avoid accepting unexpected extra connections unless a
  test explicitly expects retries or reconnects.
- Cache-specific tests should remain clearly named so later cache removal can
  delete/migrate them intentionally instead of hiding cache dependencies.
- Do not convert tests that are intentionally proving cache preservation or
  diagnostics; those should remain cache-based until the cache surface is
  removed.

## Test Plan

Focused tests:

- `cargo test --test daemon_integration daemon_ -- --nocapture`
- `cargo test --test daemon_integration one_shot -- --nocapture`
- `cargo test --test daemon_integration tui_ -- --nocapture`
- `cargo test --test daemon_integration display_popup -- --nocapture`
- `cargo test cache_validate -- --nocapture`
- `cargo test daemon_socket -- --nocapture`

Required coverage:

- Real daemon readiness can be observed through daemon socket snapshots without
  reading the cache file.
- Socket snapshot helpers use protocol/schema constants rather than cache-derived
  schema versions.
- Daemon pane add/remove/title/metadata/session/window update tests assert socket
  snapshot state rather than cache JSON when the cache is not the behavior under
  test.
- Removal tests prove present-before-absent and do not pass on an initial missing
  pane/window/session.
- One-shot daemon-backed commands still ignore poisoned cache contents.
- TUI still bootstraps and rerenders from socket state with poisoned/missing
  cache.
- Cache-specific tests remain isolated and explicit.
- An allowlist `rg` check proves the full suite no longer uses cache writes as a
  prerequisite for non-cache daemon-backed behavior.
- Expected environment-specific skips remain limited to existing display-popup
  tmux-version/key-injection gating and are documented in harness code/docs.

Regression gates:

- `cargo fmt --all --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_arguments`
- `cargo test`

## Documentation Impact

Update `docs/harness-engineering.md` with the new socket-first test helper
contract: daemon-backed behavior should be observed through isolated sockets,
cache helpers are reserved for cache-specific tests, and real tmux subprocess
coverage remains only where end-to-end behavior matters. Include the cache helper
allowlist and expected display-popup skip conditions.

## Plan Review Notes

Plan review required these refinements before implementation:

- Add a mechanical cache-dependency allowlist instead of relying on helper-name
  cleanup.
- Do not derive daemon socket hello schema versions from cache JSON; use shared
  protocol/schema constants.
- Ensure removal/absence tests first observe presence and then observe absence
  after mutation.
- Require fake socket fixtures to use bounded IO, request capture, RAII cleanup,
  and unexpected-connection assertions.
- Keep socket wait timeout diagnostics rich enough to debug flakes.
