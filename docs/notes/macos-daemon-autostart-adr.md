# ADR: macOS Daemon Auto-Start And Executable Assessment

Status: accepted
Date: 2026-05-08

## Context

`agentscan` originally behaved like a normal CLI: a user invoked the binary, it
read tmux state, printed output, and exited. The daemon rollout changed that
shape. Normal commands now connect to a Unix socket and, when the daemon is not
running, may launch a second copy of the current executable as:

```text
agentscan daemon run
```

That child is detached from the invoking terminal with daemon-style stdio and
session handling. On macOS this means a short-lived foreground command can cause
an unattended background `exec` of the same ad-hoc or provenance-tagged binary.

After the auto-start rollout, the host repeatedly kernel-panicked with the same
signature:

- panic: `os_refcnt: overflow ... @refcnt.c`
- panicked task: `agentscan`
- kernel backtrace: `com.apple.AppleSystemPolicy` with AMFI, quarantine, and
  sandbox dependencies
- unified log context: `syspolicyd` and `amfid` doing signature, trust,
  Gatekeeper, provenance, and detached-signature work

The latest observed panicked `agentscan` process had essentially no userspace
runtime state: zero user time, zero system time, one main thread in kernel
frames, and a tiny resident set. That points to macOS launch or policy
evaluation, not tmux scanning or Rust provider classification logic.

## Current Understanding

The issue is not that every macOS CLI must be Developer ID signed. Local ad-hoc
developer binaries normally run. The distinguishing behavior is that
`agentscan` became a self-launching daemon manager:

```text
shell / tmux / mise
  -> agentscan list | tui | inspect | snapshot
       -> daemon socket missing or stale
       -> resolve current executable
       -> macOS trust preflight
       -> spawn current executable as `daemon run`
            -> AppleSystemPolicy / AMFI / quarantine / provenance path
```

The failure is a macOS kernel failure in the policy path. Even if the executable
is ad-hoc signed, invalidly signed, quarantined, or provenance-tagged, correct
OS behavior should be launch denial or an error, not a host reboot.

Because the panic appears to happen during process launch or assessment, Rust
guards inside the child process cannot be relied on. A child that panics before
userspace initialization will not log, reject, or clean up through application
code.

## Evidence

Observed panic files:

- `/Library/Logs/DiagnosticReports/panic-full-2026-05-06-170918.0002.panic`
- `/Library/Logs/DiagnosticReports/panic-full-2026-05-07-084149.0002.panic`
- `/Library/Logs/DiagnosticReports/panic-full-2026-05-07-212349.0002.panic`
- `/Library/Logs/DiagnosticReports/panic-full-2026-05-08-071032.0002.panic`

All observed recent panics named `agentscan` as the panicked task and included
`AppleSystemPolicy` in the kernel backtrace.

Post-crash inspection found the installed cargo and mise binaries were
ad-hoc/linker-signed and carried `com.apple.provenance`. On this host that
attribute could not be removed with `xattr -d` or `xattr -c`.

No active LaunchAgent, LaunchDaemon, cron entry, or tmux pane was found that
explained the unattended launch. Durable invocation paths found locally were:

- tmux popup wrapper: `~/.zshrc.d/scripts/agentscan-popup.sh`
- cargo install path: `~/.cargo/bin/agentscan`
- mise install path:
  `~/.local/share/mise/installs/github-aurokin-agentscan/0.2.3/agentscan`

## Current Mitigation

The installed cargo and mise entrypoints on the affected host were replaced by
shell intercept wrappers, and the real binaries were moved aside as:

- `~/.cargo/bin/agentscan.real`
- `~/.local/share/mise/installs/github-aurokin-agentscan/0.2.3/agentscan.real`

The wrappers log attempted invocations to:

```text
~/.local/state/agentscan/invocations.log
```

The log includes timestamp, argv, cwd, pid, parent pid, parent command, tty,
`TMUX`, and `AGENTSCAN_EXEC_REAL`. The wrappers block by default. Intentional
execution of the real binary requires:

```text
AGENTSCAN_EXEC_REAL=1 agentscan daemon run
```

This mitigation is host-local and not a product design.

## Accepted Decision

Detached daemon auto-start on macOS has a stricter product boundary than Linux:

1. A normal foreground command may not silently self-exec a detached daemon on
   macOS.
2. A macOS child process must not be the first place where unsafe daemon startup
   is rejected; the parent must decide before spawning.
3. Foreground `agentscan daemon run` remains the recovery and development path
   for macOS users.
4. Explicit detached `agentscan daemon start` remains available on macOS only
   for non-ad-hoc, validly signed binaries.
5. The user must have a hard opt-out from auto-start through
   `--no-auto-start` and `AGENTSCAN_NO_AUTO_START=1`.

## Follow-Ups

- Release packaging now signs macOS release binaries with Developer ID, hardened
  runtime, and secure timestamp before submitting them to Apple's notary
  service. See `docs/macos-release-signing.md`.
- Consider whether the daemon parent should write a structured pre-spawn
  attempt log for every macOS explicit start decision, including rejected
  starts.

## Next Options

Option A: signed-only detached daemon on macOS.

This was rejected as too permissive for implicit starts. `daemon start` remains
signed-only, but implicit auto-start and TUI subscription auto-start are removed
on macOS.

Option B: no implicit auto-start on macOS.

Normal daemon-backed commands connect only to an existing daemon. If no daemon
is running, they fail with guidance. Users start a daemon explicitly through a
known-safe path. This reduces invisible behavior and makes invocation ownership
clear.

Option C: foreground helper instead of detached self-exec.

The TUI or command process can operate with direct tmux snapshots or a
foreground daemon mode, but does not spawn a detached child. This preserves some
ergonomics while avoiding the unattended launch edge.

Option D: keep auto-start but add richer assessment and audit logging.

This preserves the Linux-like UX but has the highest risk because the failing
component is the macOS policy path itself. It is only reasonable if a signed
binary path is available and the parent logs every assessment before spawning.

## Implemented Policy

- macOS signed release binary: explicit detached `agentscan daemon start` is
  allowed, but implicit auto-start remains disabled.
- macOS ad-hoc or locally built binary: detached starts are rejected; users run
  `agentscan daemon run`, `agentscan scan`, or refresh-capable direct paths.
- `agentscan tui` on macOS requires an already-running daemon and guides users
  to run `agentscan daemon run` in a long-lived tmux pane when none is running.

This avoids silent background self-exec on macOS while preserving explicit
daemon workflows.
