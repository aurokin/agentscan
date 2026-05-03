# AUR-172 Issue Plan: Serve One-Shot Snapshots Over The Daemon Socket

## Scope

Implement daemon snapshot-mode serving over the Unix socket foundation from `AUR-171`.

This issue adds:

- daemon-to-client frames for `snapshot` and unavailable/terminal daemon states
- a foreground `daemon run` socket server bound to `ipc::resolve_socket_path()`
- shared daemon socket state for the latest good `SnapshotEnvelope`
- snapshot-mode client handling: hello, `hello_ack`, one snapshot or precise unavailable/shutdown frame, then EOF
- bounded client hello reads and bounded daemon frame writes using the existing frame limits
- startup readiness state: initializing, ready, startup failed, and closing
- tests with synthetic socket clients for ready snapshot, not-ready startup, startup failure, closing-state precedence, and frame-size behavior

## Non-Goals

- Do not migrate `list`, bare `agentscan`, `inspect`, `focus`, `snapshot`, or TUI clients yet.
- Do not add subscribe-mode clients or fan-out yet; `subscribe` may remain rejected or unavailable until `AUR-173`.
- Do not add detached `daemon start`, lifecycle lock/log sidecars, stale socket cleanup, or safe stop; those are `AUR-174`.
- Do not remove cache writes or cache command surfaces; this issue may keep cache writes as migration compatibility while adding socket publication.
- Do not add auto-start or `--no-auto-start`; those are `AUR-175`.

## Implementation Outline

1. Extend `src/app/ipc/mod.rs`.
   - Add `DaemonFrame::Snapshot { snapshot: SnapshotEnvelope }`.
   - Add a distinct unavailable frame for valid clients after `hello_ack`: `daemon_not_ready` is retryable, while `startup_failed`, `server_closing`, and `subscribe_unavailable` are terminal for that connection.
   - Keep `Shutdown` for protocol/schema mismatches and other intentional no-ack shutdowns.
   - Keep protocol/schema mismatch behavior explicit and client-first.
   - Keep daemon frame byte limits enforced before writing snapshots.

2. Add daemon socket state and server helpers, likely in `src/app/daemon.rs` unless the code shape clearly warrants a small submodule.
   - Store latest good snapshot in shared state.
   - Store pre-encoded latest-good snapshot frame bytes beside the snapshot so frame-size checks and last-good behavior are precise.
   - Track startup status separately from snapshot presence so missing initial snapshot can return `daemon_not_ready` only while initialization is still in progress.
   - Check closing state before not-ready.
   - Mark startup failed when initial tmux snapshot, attach, subscription setup, or initial publication fails.
   - Keep lock guards short: read or clone pre-encoded bytes while locked, then drop the lock before socket writes.

3. Wire foreground `daemon_run`.
   - Resolve and bind the socket before startup work so clients can observe initializing and terminal startup state.
   - Use per-tempdir `AGENTSCAN_SOCKET_PATH` in tests and assert the daemon/server uses the requested path.
   - Start an accept loop that handles each snapshot-mode client without blocking the tmux control-mode update loop.
   - Publish the initial snapshot once it exists, then publish later daemon updates only if frame encoding fits `DAEMON_FRAME_MAX_BYTES`.
   - Oversized initial snapshots should fail startup before socket readiness with diagnostic guidance that names the encoded size, frame limit, startup context, and that no usable socket snapshot was published. Oversized later snapshots should preserve the last good socket snapshot and log the skipped update with the encoded size, frame limit, later-update context, and last-good preservation.
   - Treat initial cache write failure as an initial publication failure during the migration window: do not publish socket readiness until the temporary cache-compatible write has succeeded. This keeps existing cache-backed commands coherent until their later socket migration issues remove the cache dependency.
   - If startup fails after the socket is listening, transition shared state to `startup_failed` before returning the foreground daemon error. Use an injectable startup/server harness in tests rather than relying only on fixed sleep timing.
   - Continue cache writes for now so existing command behavior and tests remain intact until later issues, but do not write the initial daemon-marked cache until initial snapshot publication and tmux subscription setup have succeeded.

4. Add synthetic test harnesses.
   - Prefer Unix socket pairs or temporary `UnixListener` paths with synthetic clients.
   - Test protocol behavior without real tmux subprocesses where possible.
   - Keep real tmux integration changes minimal for this issue.

## Edge Cases

- Valid snapshot-mode hello receives `hello_ack`, one `snapshot`, then EOF.
- Protocol/schema mismatch receives a shutdown frame without `hello_ack`.
- Unknown or malformed client frames fail without registering state.
- Subscribe-mode clients receive `hello_ack`, `subscribe_unavailable`, and EOF; they are not registered as subscribers in this issue.
- Closing state wins over `daemon_not_ready`.
- Startup failure is terminal for the daemon process state and should not be reported as transient not-ready.
- Oversized initial snapshot fails startup rather than publishing an unusable daemon.
- Oversized later snapshot keeps the previous good socket snapshot available to clients and emits diagnostic logging that includes encoded byte size, frame limit, initial/later context, and last-good preservation.
- Cache/socket divergence after an oversized later update is acceptable only during the cache-backed migration window; socket clients must still receive the previous good socket snapshot.
- Initial cache write failure is treated as an initial publication failure until one-shot clients migrate away from the cache in later issues.

## Test Plan

Focused tests:

- `cargo test ipc`
- new daemon socket unit tests for ready snapshot, startup not-ready, direct startup failed response, closing-state precedence, subscribe unavailable/non-registration, and frame-size behavior
- startup harness tests for initial snapshot failure, tmux attach failure, subscription setup failure, and initial cache publication failure transitioning to observable `startup_failed`
- one real Unix socket/path test using an isolated `AGENTSCAN_SOCKET_PATH` to verify bind path, EOF, and environment isolation

Regression checks after implementation:

- `cargo fmt --all --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test`

Run the complexity clippy gate if daemon orchestration grows enough to risk it:

- `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_arguments`

## Documentation Impact

No full user-doc rewrite in this issue. Add or adjust only narrow comments/docs if a new internal frame/state contract would otherwise be unclear. Durable docs and release notes remain `AUR-180`.

## Plan Review Notes

The milestone plan review flagged two constraints for this issue:

- Define the daemon-to-client frame contract before relying on socket serving.
- Keep lifecycle ownership out of this issue unless absolutely required; singleton lock/log identity, stale socket cleanup, detached start, status, and stop remain `AUR-174`.

Fresh plan review before implementation flagged these additions:

- Startup failure coverage must prove the named initial snapshot, tmux attach, subscription setup, and initial publication paths, not only direct state mutation.
- Initial oversize diagnostics need explicit size/limit/startup-context guidance.
- Temporary cache compatibility must be coherent: cache write failure blocks socket readiness for this slice.
- Startup-failed observability should be tested through injectable startup/server seams rather than fixed sleeps alone.
