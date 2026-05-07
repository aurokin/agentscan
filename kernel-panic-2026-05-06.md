# macOS Kernel Panic During Agentscan Daemon Auto-Start

Date prepared: 2026-05-06
Repo: `git@github.com:aurokin/agentscan.git`
Repo commit at investigation time: `3376eae886010fe1947aa77066dfa3994b8ce9d7`
Affected Agentscan version: `0.2.0`

This report is intentionally written as a handoff for an experienced debugging
agent. It includes local paths and panic identifiers, but omits serial numbers,
hardware UUIDs, and other unnecessary machine identifiers.

## Executive Summary

A Mac Studio running macOS 26.4.1 panicked immediately after Agentscan 0.2.0
introduced daemon auto-start. The panic was in Apple's kernel/system policy
path, not in user-space Rust execution:

```text
panic(cpu 15 caller 0xfffffe0051dac1d4): os_refcnt: overflow
(rc=0xfffffe241eb894b4, count=268435456, max=268435455) @refcnt.c:68
```

The panicked task was `agentscan`:

```text
Panicked task ... pid 67340: agentscan
Kernel Extensions in backtrace:
  com.apple.AppleSystemPolicy(2.0)
    dependency: com.apple.driver.AppleMobileFileIntegrity
    dependency: com.apple.security.quarantine
    dependency: com.apple.security.sandbox
```

The stackshot shows two adjacent `agentscan` processes:

- `pid 67339`: `agentscan`, thread running in kernel, with some prior CPU time.
- `pid 67340`: `agentscan`, `terminatedSnapshot`, 16 KB resident, zero user/system
  CPU, one thread running in kernel.

That pattern strongly suggests the panic happened while a new `agentscan`
process was being spawned/execed and validated by macOS code-signing/system
policy, not while Agentscan user-space logic was executing.

Primary working theory: Agentscan's auto-start path spawned the current
ad-hoc-signed executable as `agentscan daemon run`, triggering a macOS
AppleSystemPolicy refcount overflow bug. Agentscan cannot directly corrupt
kernel memory from safe Rust here, but it can trigger the kernel bug by the way
it repeatedly spawns or validates an ad-hoc executable.

## Environment

Machine:

- Model: Mac Studio, Mac16,9
- Chip: Apple M4 Max
- CPU: 16 cores
- RAM: 64 GB

OS:

- macOS: 26.4.1
- Build: `25E253`
- Kernel: `Darwin Kernel Version 25.4.0: Thu Mar 19 19:33:25 PDT 2026; root:xnu-12377.101.15~1/RELEASE_ARM64_T6041`
- Developer mode: enabled in panic report
- SIP: enabled
- Secure boot: yes

Relevant installed binaries:

```text
/Users/auro/.cargo/bin/agentscan
  mtime: 2026-05-06 14:21:24 -0600
  sha256: 963e0c93d81b0bfc385cd23a49057850aed0fdcb531ea5381a8272ce6ac80d01
  codesign: ad-hoc/linker-signed, TeamIdentifier not set
  spctl --assess --type execute: rejected

/Users/auro/.local/share/mise/installs/github-aurokin-agentscan/0.2.0/agentscan
  mtime: 2026-05-06 14:12:47 -0600
  sha256: ea72364818b1faf0b1206719058eca4f95db1733e522c3baf6c8d24fe80679f1
  codesign: ad-hoc/linker-signed, TeamIdentifier not set
  spctl --assess --type execute: rejected

/Users/auro/code/agentscan/target/release/agentscan
  mtime: 2026-05-06 16:32:52 -0600
  sha256: 1096f7e2ff825ec1379210a6316f7c0107d872492653247520bf9b57ea1221c4
  codesign: ad-hoc/linker-signed, TeamIdentifier not set
  spctl --assess --type execute: rejected
```

Current `which -a agentscan` ordering:

```text
/Users/auro/.cargo/bin/agentscan
/Users/auro/.local/share/mise/installs/github-aurokin-agentscan/0.2.0/agentscan
```

## Panic Artifacts

Primary panic report:

```text
/Library/Logs/DiagnosticReports/panic-full-2026-05-06-170918.0002.panic
```

Reset counter:

```text
/Library/Logs/DiagnosticReports/ResetCounter-2026-05-06-170921.diag
```

Reset counter summary:

```text
Reset count: 1
Boot failure count: 0
Boot faults: wdog,reset_in_1
```

Boot history:

```text
reboot time  Wed May  6 17:08
```

The panic timestamp in the report is:

```text
2026-05-06 17:09:18.51 -0600
```

The panic report's stackshot notes:

```text
Process 67339 is in transition type 1
```

## Panic Signature Interpretation

The panic is an XNU reference count overflow:

```text
count=268435456, max=268435455
```

The max value is `0x0fffffff`. Apple's XNU source defines:

```c
#define OS_REFCNT_MAX_COUNT ((os_ref_count_t)0x0FFFFFFFUL)
```

When a retain/check observes the count at or above the max, XNU panics instead
of allowing a refcount overflow. The panic therefore means a kernel object in
the AppleSystemPolicy/code-signing path either leaked retains until it hit the
limit or was observed in a corrupted/refcount-invalid state. Agentscan is the
triggering userspace process, but the failing invariant is inside the kernel.

## Relevant Agentscan Change

Agentscan 0.2.0 shipped daemon auto-start on 2026-05-06. The changelog says:

- Normal `agentscan`, `list`, `inspect`, `focus`, `snapshot`, and `tui`
  invocations spawn a background daemon if one is not already running.
- `--no-auto-start` and `AGENTSCAN_NO_AUTO_START=1` are supported opt-outs.
- `agentscan scan` remains daemon-free and reads tmux directly.

Relevant source locations:

- `src/app/commands.rs:171`: `snapshot_for_consumer(...)`
- `src/app/commands.rs:179`: non-refresh consumers call
  `daemon::snapshot_via_socket(...)`
- `src/app/daemon.rs:976`: `snapshot_via_socket_path(...)`
- `src/app/daemon.rs:1011`: missing socket triggers `start_daemon(socket_path)?`
- `src/app/daemon.rs:1449`: `daemon_start_with_socket_path_and_output(...)`
- `src/app/daemon.rs:1453`: daemon start uses `env::current_exe()`
- `src/app/daemon.rs:1572`: `Command::new(executable_path)`
- `src/app/daemon.rs:1574`: child args are `["daemon", "run"]`
- `src/app/daemon.rs:1582`: `pre_exec` calls `setsid()`
- `src/app/daemon.rs:1591`: `.spawn()`

The critical user-space path is:

```text
agentscan list/default/snapshot/inspect/focus
  -> snapshot_for_consumer(refresh=false)
  -> daemon::snapshot_via_socket(AutoStartPolicy)
  -> snapshot_once_from_socket(...)
  -> SnapshotQuery::NotRunning("socket is missing")
  -> daemon_start_with_socket_path_and_output(...)
  -> current_exe()
  -> Command::new(current_exe).args(["daemon", "run"]).spawn()
```

At investigation time, `AGENTSCAN_NO_AUTO_START=1 agentscan daemon status`
reported:

```text
daemon_state: not_running
socket_path: /var/folders/6s/.../T/agentscan-501/agentscan.sock
reason: socket is missing
```

No `agentscan` process was left running after reboot.

## Evidence Supporting Exec/Code-Signing Trigger

The stackshot process entry for `pid 67340`:

```json
{
  "procname": "agentscan",
  "residentMemoryBytes": 16384,
  "userTimeTask": 0,
  "systemTimeTask": 0,
  "flags": [
    "terminatedSnapshot",
    "isImpDonor",
    "isLiveImpDonor",
    "sharedRegionNone"
  ]
}
```

The single `pid 67340` thread had only kernel frames. Three frames were in
binary image index `845`, which maps to:

```text
com.apple.AppleSystemPolicy(2.0)
UUID: ABFFD77B-457A-3F56-BB10-6AC8CA5257F4
```

The panic text explicitly lists `AppleSystemPolicy` in the backtrace.

This is consistent with process launch/exec validation:

- new executable is spawned,
- macOS validates policy/code signature/quarantine/sandbox metadata,
- kernel panics before user-space `agentscan daemon run` accumulates any CPU.

## Existing Prior Agentscan Diagnostics

There is one older Agentscan diagnostic:

```text
/Library/Logs/DiagnosticReports/agentscan_2026-05-03-061745_koopa.diag
```

That was a disk-write resource report, not a panic:

```text
Event: disk writes
Writes: 2147.51 MB over 48232 seconds
Heaviest stack:
  agentscan::app::daemon::daemon_run
  agentscan::app::cache::write_snapshot_to_cache
```

It predates the 0.2.0 socket auto-start migration and is probably unrelated,
except that it shows prior long-lived daemon behavior existed.

## Working Hypotheses

### H1: macOS AppleSystemPolicy bug triggered by spawning ad-hoc Agentscan binary

Most likely. All local Agentscan binaries are ad-hoc/linker-signed and rejected
by `spctl --assess --type execute`. Terminal can still run them, but daemon
auto-start means normal read commands now spawn an additional copy of the same
ad-hoc executable. The panic occurred in `AppleSystemPolicy`, before the child
had user-space runtime.

Expected mitigation if true:

- Avoid auto-start for unsigned/ad-hoc binaries on macOS.
- Prefer a stable, explicitly signed daemon binary.
- Gate auto-start behind a macOS executable assessment preflight.

### H2: repeated auto-start or concurrent auto-start creates a policy validation storm

Possible. `daemon_start_with_socket_path_output_and_command` uses a start lock,
but multiple clients may have attempted normal Agentscan calls around the same
time. The stackshot contains two adjacent `agentscan` PIDs and a note that
`pid 67339` was in transition. The agent should inspect whether a daemon start
attempt can race with another caller, stale socket cleanup, or incompatible
daemon status, causing rapid repeated execs.

Expected mitigation if true:

- Add stronger single-flight behavior around start attempts.
- Add a short cooldown/backoff file after failed/aborted starts.
- Log start attempt PID, executable path, code signature status, and parent PID
  before spawning.

### H3: current executable path changes under a running or just-built binary

Possible but less directly supported. At investigation time, several Agentscan
builds existed:

- `~/.cargo/bin/agentscan` modified at 14:21
- mise install modified at 14:12
- repo release/debug builds modified around 16:32

`current_exe()` starts the same executable path as the caller. If a caller runs
from a path being overwritten/reinstalled, macOS policy validation may see
unusual executable identity churn. The panic still belongs to macOS, but
Agentscan can reduce exposure by avoiding self-spawn from mutable build paths.

Expected mitigation if true:

- Refuse daemon auto-start when `current_exe()` is under `target/`.
- Prefer an installed release binary for daemon mode.
- Add an explicit `AGENTSCAN_DAEMON_BIN` override for supervisors/tests.

## Recommended Immediate Guardrail

Until root cause is known, disable daemon auto-start on macOS by default for
ad-hoc or rejected executables, while preserving explicit `agentscan daemon start`.

Concrete behavior:

1. On macOS, before auto-start, run a lightweight executable trust check on
   `current_exe()`.
2. If the executable is ad-hoc or fails assessment, return a clear
   `AutoStartDisabled` or new `UnsafeAutoStart` error:

   ```text
   daemon auto-start is disabled for this macOS executable because Gatekeeper
   assessment rejected it; run `agentscan scan`, pass `--refresh`, explicitly run
   `agentscan daemon start`, or install a signed release binary
   ```

3. Do not apply this guard to explicit `agentscan daemon start` unless desired.
   Explicit starts are operator intent and useful for reproducing.

Implementation notes:

- A robust first pass can call `/usr/sbin/spctl --assess --type execute --raw`
  or `/usr/bin/codesign -dv` only on macOS. Prefer a small helper function with
  tests that can be faked rather than shelling out deep inside the start path.
- The guard should live before `Command::new(executable_path).spawn()`.
- The guard should be bypassable with an explicit env var only for debugging,
  for example `AGENTSCAN_ALLOW_UNTRUSTED_DAEMON_AUTOSTART=1`.

## Recommended Diagnostics to Add

Add append-only daemon-start preflight logging before spawn, ideally to the
existing `agentscan.sock.log` path and stderr for explicit start:

```text
daemon_start_preflight:
  timestamp
  parent_pid
  executable_path
  executable_canonical
  executable_sha256
  codesign_summary
  spctl_assessment_result
  socket_path
  start_lock_path
  tmux_env_present
  agentscan_tmux_socket_present
  env_removes
```

The important part is to log before `.spawn()`, because this panic can happen
before the child writes anything.

## Reproduction Plan for a Debugging Agent

Do not start with panic reproduction on the user's primary machine. Use a
throwaway macOS 26.4.1 Apple Silicon host or VM if available.

1. Build Agentscan 0.2.0 from commit `3376eae`.
2. Confirm binary is ad-hoc and rejected:

   ```sh
   codesign -dv --verbose=4 ./target/release/agentscan
   spctl --assess --type execute -vv ./target/release/agentscan
   ```

3. Use an isolated socket path:

   ```sh
   export AGENTSCAN_SOCKET_PATH="$(mktemp -d)/agentscan.sock"
   ```

4. Exercise no-autostart baseline:

   ```sh
   AGENTSCAN_NO_AUTO_START=1 ./target/release/agentscan list
   ./target/release/agentscan scan
   ```

5. Exercise explicit start:

   ```sh
   ./target/release/agentscan daemon start
   ./target/release/agentscan daemon status
   ./target/release/agentscan daemon stop
   ```

6. Exercise normal auto-start:

   ```sh
   rm -f "$AGENTSCAN_SOCKET_PATH" "$AGENTSCAN_SOCKET_PATH".*
   ./target/release/agentscan list
   ```

7. Exercise concurrent auto-start:

   ```sh
   rm -f "$AGENTSCAN_SOCKET_PATH" "$AGENTSCAN_SOCKET_PATH".*
   for i in $(seq 1 32); do
     ./target/release/agentscan list >/tmp/agentscan.$i.out 2>/tmp/agentscan.$i.err &
   done
   wait
   ```

8. Repeat with a signed binary. If the issue disappears when signed, prioritize
   the trust/assessment guard.

## Suggested Code Changes

### Short-term safety patch

- Add macOS-only auto-start preflight.
- If executable is ad-hoc/rejected and the start was implicit, do not spawn.
- Keep `scan`, `--refresh`, and `--no-auto-start` behavior as escape hatches.
- Add tests around policy decisions using injectable assessment results.

### Better daemon start API

Separate "implicit daemon auto-start" from "explicit lifecycle start" at the
type level. Today both converge on:

```rust
daemon_start_with_socket_path_and_output(&socket_path, StartOutput::Quiet)
```

Introduce an intent enum:

```rust
enum DaemonStartIntent {
    ExplicitLifecycleCommand,
    ImplicitConsumerAutoStart,
    TuiSubscriptionAutoStart,
}
```

Use the intent to decide:

- whether executable trust preflight is required,
- whether to print/log detailed guidance,
- whether to allow an override env var,
- whether to apply a cooldown after failure.

### Optional packaging improvement

Ship a properly signed/notarized macOS release binary for the daemon path. If
developer installs are expected to be ad-hoc, keep auto-start disabled by
default for those builds and document the explicit opt-in.

## Commands Used During Investigation

Useful commands to rerun:

```sh
sw_vers
uname -a
system_profiler SPSoftwareDataType SPHardwareDataType
ls -lt /Library/Logs/DiagnosticReports
sed '/^System Profile:/,$d' /Library/Logs/DiagnosticReports/panic-full-2026-05-06-170918.0002.panic | tail -n +2 | jq '.processByPid["67340"]'
codesign -dv --verbose=4 /Users/auro/.cargo/bin/agentscan
spctl --assess --type execute -vv /Users/auro/.cargo/bin/agentscan
AGENTSCAN_NO_AUTO_START=1 agentscan daemon status
which -a agentscan
```

## Current Operator Workaround

Use one of:

```sh
export AGENTSCAN_NO_AUTO_START=1
agentscan scan
agentscan list --refresh
agentscan snapshot --refresh
```

Avoid running `agentscan` commands that implicitly auto-start the daemon on the
affected machine until a guardrail is implemented or a signed binary is used.

## Open Questions

- Was the command that triggered the panic `agentscan`, `agentscan list`,
  `agentscan tui`, or another daemon-backed consumer?
- Did multiple shells or tmux bindings invoke Agentscan concurrently around
  17:09:18?
- Was the invoking binary `~/.cargo/bin/agentscan`, the mise install, or a
  `target/*/agentscan` build?
- Does a signed/notarized build avoid the panic on the same OS build?
- Does explicit `agentscan daemon start` reproduce, or only implicit auto-start?

## Bottom Line

Treat this as a macOS kernel bug triggered by Agentscan's new implicit
self-spawn behavior. The fix should not attempt to "fix" kernel refcounts.
Instead, reduce or gate the trigger surface:

- do not implicitly spawn untrusted/ad-hoc macOS binaries,
- add pre-spawn diagnostics,
- make daemon start intent explicit,
- test concurrent start behavior,
- prefer signed release binaries for daemon mode.
