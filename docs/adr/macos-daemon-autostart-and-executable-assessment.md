# ADR: macOS Daemon Auto-Start And Executable Assessment

Status: accepted, under review after new stability evidence
Date: 2026-05-08
Updated: 2026-05-14

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

This understanding is intentionally conservative. It explains why `agentscan`
removed implicit macOS auto-start during the incident, but it is not a proven
root-cause statement. Later evidence weakens the claim that daemon auto-start,
tmux restore, or local ad-hoc signing was independently sufficient to trigger
the panics.

## 2026-05-14 Update

The host became stable after several variables changed close together:

- `agentscan` release binaries were Developer ID signed and notarized.
- local entrypoints were aligned to signed `agentscan 0.2.6`.
- wrapper-based attribution was removed.
- the macOS process-inspection fallback stopped shelling out to `ps` and
  `pgrep`; it now uses native `libproc` and `sysctl` inspection.
- tmux resurrect and continuum were disabled.
- macOS was updated from `26.4.1 (25E253)` to `26.5 (25F71)`.
- the daemon was run explicitly in a long-lived tmux pane while a focused
  EndpointSecurity exec logger watched future invocations.

As of the post-update checks, signed `agentscan 0.2.6` had run for many hours
without a new panic, without `agentscan` AppleSystemPolicy denial lines, and
without an unexpected `cargo-agentscan` or wrapper process. A small number of
generic `syspolicyd` validation messages still appeared, but they did not name
`agentscan` and no nearby ES logger event tied them to an `agentscan` exec.

This does not prove which variable fixed the panics. Plausible explanations now
include:

- a macOS `25E253` AppleSystemPolicy/kernel bug fixed or avoided by `25F71`
- excessive process launch/policy activity from shelling out to `ps`/`pgrep`
  during daemon refreshes
- repeated execution of unsigned or provenance-tagged wrapper/binary paths
- tmux restore/continuum replaying an invocation path after crashes
- the original implicit daemon auto-start path
- an interaction among several of the above rather than one isolated cause

Future work should not treat this ADR as proof that macOS auto-start is
inherently unsafe. The no-implicit-auto-start period was a safety rollback made
under uncertainty. Signed-only auto-start is reasonable when the parent process
runs trust preflight before spawning, process inspection avoids shell helper
churn, wrapper indirection is not required, and users retain a hard opt-out.

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

## Historical Host Mitigation

During the investigation, the affected host temporarily replaced installed cargo
and mise entrypoints with shell intercept wrappers, and moved the real binaries
aside as:

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

This mitigation was host-local and not a product design. It was removed after
signed release binaries and a focused EndpointSecurity exec logger replaced the
wrapper-based attribution path.

## Accepted Decision

Detached daemon auto-start on macOS currently has a stricter product boundary
than Linux:

1. A normal foreground command may self-exec a detached daemon on macOS only
   after parent-side executable trust preflight succeeds.
2. A macOS child process must not be the first place where unsafe daemon startup
   is rejected; the parent must decide before spawning.
3. Foreground `agentscan daemon run` remains the recovery and development path
   for macOS users.
4. Detached daemon starts remain available on macOS only for non-ad-hoc,
   validly signed binaries.
5. The user must have a hard opt-out from auto-start through
   `--no-auto-start` and `AGENTSCAN_NO_AUTO_START=1`.

## Follow-Ups

- Release packaging now signs macOS release binaries with Developer ID, hardened
  runtime, and secure timestamp before submitting them to Apple's notary
  service. See `docs/macos-release-signing.md`.
- The daemon process-inspection fallback no longer shells out to `ps` or `pgrep`
  on macOS. Native `libproc` and `sysctl` inspection now provide foreground
  process, descendant, command, and argv evidence without adding executable
  policy events from helper processes during normal refreshes.
- Consider whether the daemon parent should write a structured pre-spawn
  attempt log for every macOS explicit start decision, including rejected
  starts.

## Next Options

Option A: signed-only detached daemon on macOS.

This is the implemented direction. Explicit starts, one-shot consumer
auto-start, and TUI subscription auto-start share the same parent-side
executable trust preflight before any detached child is spawned.

Option B: no implicit auto-start on macOS.

Normal daemon-backed commands connect only to an existing daemon. If no daemon
is running, they fail with guidance. Users start a daemon explicitly through a
known-safe path. This was the rollback policy during the incident and remains a
fallback option if signed-only auto-start shows new evidence of risk.

Option C: foreground helper instead of detached self-exec.

The TUI or command process can operate with direct tmux snapshots or a
foreground daemon mode, but does not spawn a detached child. This preserves some
ergonomics while avoiding the unattended launch edge.

Option D: keep auto-start but add richer assessment and audit logging.

This preserves the Linux-like UX but has the highest risk because the failing
component is the macOS policy path itself. It is only reasonable if a signed
binary path is available and the parent logs every assessment before spawning.

Option E: reintroduce Linux-like auto-start behind macOS safety gates.

This would restore product ergonomics while keeping observability from the
incident. Preconditions should include signed/notarized release binaries,
native macOS process inspection, no shell wrapper indirection in the daemon
path, structured parent-side spawn decision logs, and a hard opt-out through
`--no-auto-start` and `AGENTSCAN_NO_AUTO_START=1`. This option exists because
the stable post-26.5 run suggests the original panic may have been caused by a
kernel bug or by helper-process churn rather than by daemon auto-start alone.

## Implemented Policy

- macOS signed release binary: explicit detached `agentscan daemon start`,
  daemon-backed one-shot command auto-start, and TUI subscription auto-start
  are allowed after parent-side executable trust preflight succeeds.
- macOS ad-hoc or locally built binary: detached starts are rejected; users run
  `agentscan daemon run`, `agentscan scan`, or refresh-capable direct paths.
- `agentscan tui` on macOS uses the same signed-only detached auto-start policy
  as one-shot daemon-backed commands.

This keeps the child process out of the unsafe decision path: the parent logs
and enforces the macOS assessment before spawn, while local ad-hoc development
continues to use foreground daemon or direct tmux recovery paths.
