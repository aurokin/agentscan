# AUR-174 Issue Plan: Add Daemon Lifecycle Commands

## Scope

Implement first-class daemon lifecycle commands around the socket server from `AUR-172` and subscriber server from `AUR-173`.

This issue adds:

- `agentscan daemon run` as the unchanged foreground daemon entrypoint, now with lifecycle identity and signal-aware shutdown
- `agentscan daemon start` as a detached process launcher that reuses the same run path
- socket-backed `agentscan daemon status` that never auto-starts and no longer judges daemon health from cache freshness
- `agentscan daemon stop` and `agentscan daemon restart`
- socket-derived sidecar paths for lock, identity, and log files
- startup readiness checks based on a compatible hello plus first snapshot or terminal startup failure
- guarded stop behavior using live socket identity plus PID validation
- stale socket cleanup for owned stale Unix sockets only
- focused integration tests for lifecycle commands and failure states

## Non-Goals

- Do not add command auto-start for `list`, `inspect`, `focus`, `snapshot`, or TUI; that is `AUR-175` and later issues.
- Do not migrate one-shot commands from cache to daemon snapshots; that is `AUR-176`.
- Do not move the TUI to subscriptions; that is `AUR-177`.
- Do not remove cache writes or cache commands.
- Do not add `daemon stop --force`; forced termination is deferred unless live identity and process validation are strong enough in this slice.
- Do not add log rotation beyond startup truncation.
- Do not add peer-credential checks or Windows support.
- Do not preserve the old cache-freshness `daemon status` semantics as a supported lifecycle contract.

## Implementation Outline

1. Extend CLI parsing and routing.
   - Add `DaemonCommands::{Start, Stop, Restart}`.
   - Keep `DaemonCommands::Run`.
   - Replace current cache-backed `daemon status` behavior with socket lifecycle status.
   - Keep `DaemonStatusArgs::max_age_seconds` only if needed for a transitional parse-compatible warning; prefer removing it from meaningful lifecycle status output because freshness is no longer cache-based.

2. Define lifecycle sidecar paths from the resolved socket path.
   - Derive:
     - lock path: `<socket>.lock`
     - identity path: `<socket>.identity.json`
     - log path: `<socket>.log`
   - Store sidecars next to the socket so `AGENTSCAN_SOCKET_PATH` test isolation naturally scopes all lifecycle files.
   - Before unlinking any existing socket path, confirm it is a Unix socket with `symlink_metadata().file_type().is_socket()`. Refuse regular files, directories, symlinks, and other non-socket paths with clear errors.
   - Stale socket cleanup is allowed only when connect fails with `ConnectionRefused`/equivalent and the path is a Unix socket.

3. Add daemon identity.
   - Define a JSON identity record with at least:
     - pid
     - daemon_start_time, using current timestamp at process start
     - process start/birth-time diagnostics when available from the OS
     - executable path
     - executable canonical path if available
     - socket path
     - wire protocol version
     - snapshot schema version
   - Write identity after acquiring the lock and before publishing readiness.
   - Remove identity on clean foreground exit only when it still belongs to this daemon.
   - Sidecar identity is diagnostic and a fallback input, not sufficient by itself for signaling.
   - Add a daemon-served lifecycle/status frame that returns the live identity from the process that owns the reachable socket. `stop` must bind the PID it signals to this live socket identity, not only to a sidecar.

4. Add single-instance lock.
   - Use a sidecar lock file opened with create/read/write and an exclusive non-blocking `flock`/`try_lock` equivalent.
   - `daemon run` fails clearly if another process holds the lock for the same socket path.
   - Keep the lock file itself as a persistent sidecar; lock ownership is the active-instance authority.
   - Tests must isolate sockets in tempdirs and verify a second daemon cannot own the same socket.

5. Add readiness/status socket helpers.
   - Add a lifecycle client helper that connects to the resolved socket path without auto-start.
   - Extend IPC with a lifecycle/status client mode or request frame. The daemon response includes `hello_ack` plus a status frame carrying live identity, startup state, socket path, protocol/schema, subscriber count, latest snapshot timestamp/pane count when ready, and shutdown state.
   - Implement lifecycle/status IPC before `start`, `stop`, and `restart`; safe stop depends on this live identity.
   - Prefer a new client mode under the existing wire protocol only if unknown modes produce clear incompatibility. If older daemons close or reject unclearly, bump the wire protocol for the lifecycle-capable daemon and treat no lifecycle response as restart-needed incompatibility.
   - Keep snapshot-mode readiness as a fallback for start readiness only if the status frame is not needed for the assertion; stop must use the live identity/status frame.
   - Compatible ready daemon: returns `hello_ack` plus `snapshot`.
   - Compatible initializing daemon: returns `hello_ack` plus `daemon_not_ready`.
   - Compatible startup-failed/closing daemon: returns `hello_ack` plus terminal unavailable reason.
   - Incompatible daemon: returns `shutdown` protocol/schema mismatch; status should fail with restart guidance.
   - `server_busy` shutdown is pressure, not incompatibility. Lifecycle clients retry briefly, proposed timeout 2 seconds with 50ms cadence, then report busy if saturation persists.
   - Not running: missing socket or refused stale socket reports not running and exits 0 for `status`.

6. Implement `daemon start`.
   - Resolve socket and sidecars.
   - If a compatible ready daemon is already running, print status and return success.
   - If a stale owned Unix socket exists, remove it before launch.
   - Refuse non-socket path collisions.
   - Truncate the log file at startup when it is oversized; proposed threshold: 1 MiB. Otherwise append to preserve recent lifecycle diagnostics.
   - Spawn the current executable with `daemon run`, detached from the caller, with stdout and stderr both redirected to the per-socket log file.
   - Redirect stdin from `/dev/null`.
   - Detach into a new session/process group with `setsid` or equivalent Unix pre-exec behavior so the daemon survives terminal close and caller interruption.
   - Preserve the caller environment, including `AGENTSCAN_SOCKET_PATH`, `AGENTSCAN_TMUX_SOCKET`, `TMUX_TMPDIR`, and temporary cache path in tests.
   - Wait for readiness until timeout; proposed readiness timeout: 5 seconds, polling every 50ms.
   - Success requires compatible hello plus first `snapshot`.
   - Terminal startup failure reports the socket frame when observed and includes the log path.
   - If the detached child exits before readiness, report its exit and log path.
   - Keep the child handle while waiting for readiness so child exit/log inspection is the authoritative fallback when the startup-failed socket window is missed.

7. Implement `daemon status`.
   - Never auto-start.
   - Exit 0 for not running and print a concise text status including socket path, state, and reason.
   - For ready daemon, print socket path, state, protocol/schema, snapshot timestamp, pane count, live identity pid/start time/executable, log path, and subscriber count.
   - For initializing/startup_failed/closing, print the unavailable reason and message.
   - For incompatible reachable daemon, exit non-zero with restart guidance.
   - Keep JSON output out of scope unless the existing CLI already supports it for status; durable JSON status fields can be finalized with docs in `AUR-180`.

8. Implement `daemon stop`.
   - If not running, print not-running status and exit 0.
   - If reachable but incompatible, fail with restart/manual cleanup guidance.
   - Require a fresh compatible lifecycle/status handshake that returns live identity before sending any signal.
   - Validate PID before signaling:
     - live identity PID exists
     - PID is currently live
     - executable diagnostics are compatible when available
     - process start/birth-time matches the live identity when the OS can provide it
     - sidecar identity, if present, agrees with the live socket identity; malformed/mismatched sidecar identity is a warning and cleanup target, not signal authority
   - Send SIGTERM first.
   - Wait up to a short timeout; proposed graceful stop timeout: 3 seconds.
   - Prefer no SIGKILL fallback in this issue unless all live identity and process start-time validation checks pass. If validation is partial, fail after SIGTERM timeout with manual guidance rather than escalating.
   - After process exit, remove owned socket only if it is a Unix socket and remove identity only if it still matches.
   - Re-running `daemon stop` is idempotent and exits 0.

9. Implement `daemon restart`.
   - Run `stop`, then `start`.
   - If stop observes not-running, proceed to `start`.
   - Preserve errors from incompatible reachable daemon or unsafe path collisions.

10. Add daemon shutdown handling.
   - Install SIGTERM/SIGINT handling for `daemon run` using a minimal signal flag.
   - Ensure shutdown marks socket state closing, closes subscriber mailboxes, exits the control loop, explicitly cleans up the tmux control-mode child before any wait path, and avoids hanging on the control-mode client after SIGTERM.
   - Make socket server cleanup ownership-aware: capture bound socket filesystem identity after bind and unlink on drop only if the current path is still the same Unix socket. Avoid deleting non-socket path collisions or replacement sockets.

## Edge Cases

- `daemon status` with no socket: exit 0, state `not_running`.
- `daemon status` with stale Unix socket: exit 0, state `not_running`, note stale socket.
- `daemon status` with regular file at socket path: exit non-zero with non-socket collision guidance.
- `daemon start` with existing compatible ready daemon: no duplicate daemon, exit 0.
- `daemon start` with stale Unix socket: remove stale socket and launch.
- `daemon start` with regular file/symlink/directory at socket path: refuse and do not unlink.
- `daemon start` when tmux startup fails: fail with log path and startup failure reason.
- `daemon start` when startup failure socket window is missed: fail from child-exit/log inspection and still name the log path.
- `daemon stop` with no socket/stale socket: exit 0.
- `daemon stop` with incompatible daemon: fail without signaling.
- `daemon stop` under persistent `server_busy`: retry briefly, then fail busy without signaling.
- `daemon stop` with malformed/missing live identity: fail without signaling.
- `daemon stop` with sidecar identity mismatch but valid live socket identity: signal only the live identity PID; report sidecar mismatch and clean stale sidecar only after successful stop.
- `daemon stop` with process start-time mismatch or executable mismatch: fail without signaling.
- `daemon restart` does not mask stop safety failures.
- Log sidecar grows past threshold: next detached start truncates before redirecting.
- Socket path replacement during daemon shutdown: daemon does not unlink the replacement.

## Test Plan

Focused tests:

- `cargo test daemon_socket`
- `cargo test --test daemon_integration daemon_lifecycle_start_status_stop`
- `cargo test --test daemon_integration daemon_lifecycle_restart`
- `cargo test --test daemon_integration daemon_lifecycle_start_reuses_running_daemon`
- `cargo test --test daemon_integration daemon_lifecycle_status_reports_not_running`
- `cargo test --test daemon_integration daemon_lifecycle_stop_is_idempotent`
- `cargo test --test daemon_integration daemon_lifecycle_refuses_non_socket_collision`
- `cargo test --test daemon_integration daemon_lifecycle_cleans_stale_socket`
- `cargo test --test daemon_integration daemon_lifecycle_startup_failure_names_log_path`
- `cargo test --test daemon_integration daemon_lifecycle_startup_failure_uses_child_exit_when_socket_window_is_missed`
- `cargo test --test daemon_integration daemon_lifecycle_concurrent_start_uses_single_lock_owner`
- `cargo test --test daemon_integration daemon_lifecycle_stop_retries_server_busy_without_incompatibility`
- `cargo test --test daemon_integration daemon_lifecycle_shutdown_does_not_hang_tmux_control_client`

Additional unit or synthetic tests:

- sidecar path derivation from socket path
- identity JSON roundtrip and same-identity matching
- lifecycle/status IPC frame roundtrip with live identity
- log truncation threshold behavior
- safe Unix-socket unlink guard refuses non-sockets
- owned socket unlink guard refuses replacement path
- incompatible daemon status handling with synthetic socket server returning protocol/schema shutdown
- server-busy lifecycle retry handling with synthetic socket server
- stop safety decision table: no live identity, dead PID, executable mismatch, process start-time mismatch, compatible live identity, sidecar mismatch

Regression checks after implementation:

- `cargo fmt --all --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test`

Run the complexity gate because lifecycle orchestration adds branching:

- `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_arguments`

## Documentation Impact

No full docs rewrite in this issue. Update narrow command help or status text tests as needed. Durable lifecycle docs, release notes, and cache-status removal explanations remain part of `AUR-180`.

## Plan Review Notes

Fresh plan review required these changes before implementation:

- Stop must not use sidecar identity alone. Add a live daemon lifecycle/status frame carrying identity from the reachable daemon, and bind any signaled PID to that live socket identity.
- Implement lifecycle/status IPC before lifecycle commands; decide mode vs protocol bump so older daemons become clear restart-needed incompatibility.
- Detached `daemon start` must redirect stdin from `/dev/null` and create a new session/process group.
- Signal-aware shutdown must explicitly clean up the tmux control-mode child before waiting, otherwise SIGTERM can hang and force SIGKILL.
- Socket unlink cleanup must be ownership-aware and must not delete replacement paths or non-sockets.
- Detached start must keep the child handle during readiness polling and use child exit/log inspection as the authoritative fallback when the short startup-failed socket observability window is missed.
- Lifecycle clients must treat `server_busy` as pressure with retry behavior, not as daemon incompatibility.
- Identity mismatch language must be strict: malformed/missing live identity or process validation mismatch means no signal.
