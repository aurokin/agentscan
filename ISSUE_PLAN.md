# AUR-175 Issue Plan: Add Auto-Start and Daemon Opt-Out Flow

## Scope

Add the shared daemon auto-start foundation that later socket-backed one-shot consumers will use. The helper should connect to the daemon socket, optionally invoke the same internal start path as `agentscan daemon start`, retry transient readiness states, and preserve precise startup or socket failure outcomes.

This issue adds:

- one shared connect/start/retry helper for daemon socket consumers
- internal start plumbing reused by both `daemon start` and auto-start
- explicit auto-start opt-out parsing for the one-shot commands that become daemon-backed in `AUR-176`
- `AGENTSCAN_NO_AUTO_START=1` global opt-out handling
- focused tests for success, already-running daemon, concurrent auto-start, opt-out, not-ready retry, startup failure, incompatible daemon guidance, shutdown handling, and failure message preservation

## Non-Goals

- Do not migrate bare `agentscan`, `list`, `inspect`, `focus`, or `snapshot` to socket snapshots; that is `AUR-176`.
- Do not move the TUI from cache polling to subscription or add `tui --no-auto-start`; that is `AUR-177`.
- Do not remove cache transport, cache commands, or `AGENTSCAN_CACHE_PATH`.
- Do not make `scan` daemon-backed or auto-starting.
- Do not auto-start after an intentional closing/shutdown response in the same invocation.
- Do not add a second daemonization path for auto-start.
- Do not add the `snapshot` command yet; `AUR-176` adds it and should reuse the helper and opt-out args defined here.
- Do not finalize user docs beyond narrow help text or comments needed to keep CLI behavior understandable; durable docs remain `AUR-180`.

## Implementation Outline

1. Add auto-start options to the CLI data model.
   - Add an `AutoStartArgs` struct with `--no-auto-start`.
   - Split `ScanArgs` from `ListArgs` before adding the flag, because `scan` currently reuses `ListArgs` but must remain daemon-free.
   - Flatten `AutoStartArgs` into root/default `ListArgs`, explicit `list`, `inspect`, and `focus`.
   - Do not add it to `scan`, `tui`, `daemon`, `tmux`, or `cache` in this issue.
   - Merge root `--no-auto-start` into explicit list-like commands the same way root refresh/all/format are merged.
   - Reject misplaced root auto-start flags for command families that are not daemon-backed, matching the existing root-arg rejection style.
   - Add tests that `agentscan scan --no-auto-start` and `agentscan --no-auto-start scan` are rejected.
   - Leave a small reusable args/policy shape that `AUR-176` can attach to `snapshot --no-auto-start`.

2. Define auto-start policy.
   - Add a small internal policy type, proposed shape:
     - `AutoStartPolicy { disabled: bool }`
     - disabled when the command flag is set
     - disabled when `AGENTSCAN_NO_AUTO_START=1`
   - Treat env values conservatively: only literal `1` disables auto-start for now. Empty, absent, or other values do not.
   - Make the final helper accept an explicit policy so most tests do not depend on ambient process env.
   - `AGENTSCAN_NO_AUTO_START=1 agentscan daemon start` must still start the daemon; the env var only affects daemon-backed consumers/helpers.

3. Refactor `daemon start` into reusable internals.
   - Keep `daemon_start()` as the user-facing command wrapper.
   - Move the current start implementation into an internal function that accepts:
     - resolved socket path or lifecycle paths
     - output mode: user-facing status printing vs quiet auto-start
   - Preserve the AUR-174 behavior:
     - start lock serialization
     - compatible existing daemon reuse
     - initializing readiness wait
     - stale Unix socket cleanup
     - non-socket refusal
     - per-socket log file
     - child handle cleanup on failed readiness
     - precise startup failure messages
   - Auto-start must call this internal start path, not spawn its own daemon process.
   - Quiet auto-start must not write to stdout or stderr on success, so future JSON/text consumers are not polluted before they print their own output.

4. Add a daemon socket snapshot client helper.
   - Add an internal one-shot snapshot client that sends `ClientMode::Snapshot`.
   - On compatible ready daemon, return the `SnapshotEnvelope`.
   - On `Unavailable { reason: DaemonNotReady }`, retry until the readiness deadline.
   - On `Unavailable { reason: StartupFailed }`, fail terminally with the daemon message.
   - On `Unavailable { reason: ServerClosing }`, fail terminally and do not auto-start a replacement in the same invocation.
   - On `Shutdown { reason: ServerBusy }`, retry briefly as pressure.
   - On protocol/schema mismatch or unexpected frames, return incompatible-daemon guidance.
   - Return structured outcomes/errors rather than relying on string matching. Proposed public-in-crate shape:
     - `DaemonSnapshotError::NotRunning { reason }`
     - `DaemonSnapshotError::AutoStartDisabled { reason }`
     - `DaemonSnapshotError::Incompatible { message }`
     - `DaemonSnapshotError::StartupFailed { message, log_path }`
     - `DaemonSnapshotError::ChildExited { status, log_path }`
     - `DaemonSnapshotError::ReadinessTimeout { log_path }`
     - `DaemonSnapshotError::ServerBusy { message }`
     - `DaemonSnapshotError::ServerClosing { message }`
     - `DaemonSnapshotError::UnexpectedFrame { message }`
   - Preserve clear distinctions in errors: disabled auto-start with no daemon, incompatible daemon, startup failed, child exited before readiness, timeout, server busy, and intentional shutdown.
   - Add command-facing rendering helpers only where needed; AUR-176 can add command-specific guidance when it wires consumers.

5. Add shared connect/start/retry helper.
   - Proposed shape:
     - `daemon::snapshot_via_socket(policy: AutoStartPolicy) -> Result<SnapshotEnvelope>`
     - or `daemon::connect_snapshot_with_auto_start(policy)`.
   - Flow:
     - try snapshot socket read
     - if missing/refused and auto-start is disabled, fail with opt-out guidance
     - if missing/refused and auto-start is enabled, call the shared start helper
     - after start succeeds, retry snapshot read
     - if another starter wins the race, reuse the ready daemon
   - Attempt daemon start at most once per helper invocation.
   - Reuse AUR-174 readiness timeout and 50ms poll cadence for not-ready/server-busy retry loops.
   - After a successful start, retry missing/refused/busy/not-ready only until the same readiness deadline; then return the structured timeout/busy/not-running result.
   - Terminal responses (`startup_failed`, `server_closing`, incompatible protocol/schema, unexpected lifecycle frames) stop the loop immediately.
   - The helper should be callable by AUR-176 without leaking lifecycle-specific types across command code.

6. Keep current command behavior until AUR-176.
   - `command_list`, `command_inspect`, and `command_focus` should continue to use cache/direct refresh behavior in this issue unless a narrow test-only seam is needed.
   - Parsing `--no-auto-start` before the migration is acceptable only for the AUR-176 one-shot command set; it should not change cache-backed command output yet.
   - Keep help text scoped to daemon-backed operation: "Disable daemon auto-start when this command uses daemon-backed state."
   - Add unchanged-behavior tests proving accepted no-op flags and `AGENTSCAN_NO_AUTO_START=1` do not connect to the socket, do not start the daemon, and preserve current cache-backed errors/output before AUR-176.

7. Add tests.
   - CLI parse/merge tests:
     - root `--no-auto-start` applies to bare/default list and explicit `list`
     - explicit `list --no-auto-start`, `inspect --no-auto-start`, and `focus --no-auto-start` parse
     - `tui --no-auto-start`, `scan --no-auto-start`, `--no-auto-start scan`, `--no-auto-start daemon status`, `--no-auto-start tmux ...`, and `--no-auto-start cache ...` are rejected
     - `AGENTSCAN_NO_AUTO_START=1 agentscan daemon start` still starts the daemon
   - Pre-migration command behavior tests:
     - `list --no-auto-start` still reads the cache and does not connect/start when a cache fixture is present
     - `inspect --no-auto-start` still reads the cache and does not connect/start
     - `focus --no-auto-start --refresh` preserves current refresh/focus behavior
     - `AGENTSCAN_NO_AUTO_START=1` preserves current cache-backed list/inspect/focus behavior before AUR-176
   - Unit/synthetic socket tests:
     - snapshot helper returns a ready snapshot from an already-running daemon
     - helper retries `daemon_not_ready` and then succeeds
     - helper treats `startup_failed` as terminal
     - helper treats `server_closing` as terminal and does not invoke start
     - helper reports incompatible daemon guidance
     - helper reports disabled auto-start when no daemon is reachable
     - helper exposes distinct structured error variants for incompatible daemon, startup failed, child exit, timeout, server busy, server closing, and disabled auto-start
     - quiet auto-start writes no stdout/stderr on success
   - Integration tests:
     - auto-start starts the daemon through the shared start path and then reads a snapshot
     - already-running daemon is reused
     - concurrent auto-start calls serialize and use one daemon
     - `AGENTSCAN_NO_AUTO_START=1` disables auto-start
     - `--no-auto-start` policy disables auto-start
     - startup failure from missing harness tmux server reports the real log/startup failure, not a generic socket timeout
     - concurrent helper calls assert one daemon identity, not just two successful exits

## Edge Cases

- Missing socket with auto-start enabled starts once and retries the socket read.
- Missing socket with `--no-auto-start` fails clearly through a structured disabled-auto-start error; AUR-176 will render command-specific guidance such as `agentscan daemon start`, `--refresh`, or `agentscan scan`.
- Missing socket with `AGENTSCAN_NO_AUTO_START=1` behaves like the flag.
- Refused stale Unix socket can be cleaned by the shared start helper when auto-start is allowed.
- Regular file, symlink, or directory at the socket path remains a hard collision error and is not unlinked.
- Existing initializing daemon is waited on rather than replaced.
- Existing startup-failed daemon is terminal for the invocation.
- Existing closing daemon is terminal for the invocation and does not auto-start a replacement.
- Server busy is retryable pressure, not incompatibility.
- Protocol/schema mismatch is incompatible and should not be auto-started over.
- Concurrent auto-start calls must not spawn or report multiple successful daemon identities.
- Auto-start failures should preserve child exit/log details from the shared lifecycle start path.
- Root `--no-auto-start` before non-daemon command families is rejected.
- `scan` remains direct-tmux and does not accept `--no-auto-start`.
- Before AUR-176, accepted no-op flags on list/inspect/focus preserve current cache-backed behavior.

## Test Plan

Focused tests:

- `cargo test auto_start`
- `cargo test --test daemon_integration daemon_auto_start`
- `cargo test --test daemon_integration daemon_lifecycle_concurrent_start_uses_single_daemon`
- `cargo test --test daemon_integration daemon_lifecycle_start_failure_reports_log_and_cleans_socket`

Regression gates:

- `cargo fmt --all --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_arguments`
- `cargo test`

## Documentation Impact

No full documentation rewrite in this issue. Update CLI help text and any narrow test expectations introduced by `--no-auto-start`. The durable daemon auto-start docs, migration notes, and release notes belong to `AUR-180`.

## Plan Review Notes

Plan review required these changes before implementation:

- Split `ScanArgs` from `ListArgs` or otherwise prevent `--no-auto-start` from leaking into `scan`.
- Do not add `tui --no-auto-start` in AUR-175; TUI daemon subscription and opt-out behavior belongs to `AUR-177`.
- Add unchanged-behavior tests for accepted pre-migration no-op flags on `list`, `inspect`, and `focus`.
- Specify quiet auto-start mode and test that it does not pollute future consumer stdout/stderr.
- Use structured helper errors so AUR-176 can preserve incompatible daemon, startup failed, child exited, timeout, shutdown, server busy, and opt-out distinctions.
- Specify retry bounds: one start attempt per invocation, AUR-174 readiness deadline, 50ms cadence, and immediate stop on terminal frames.
- Leave a reusable path for future `snapshot --no-auto-start` without adding the command in this issue.
