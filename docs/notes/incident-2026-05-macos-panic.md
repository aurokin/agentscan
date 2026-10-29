# Incident: macOS AppleSystemPolicy Kernel Panics (2026-05)

This note preserves the forensic evidence behind
`docs/adr/macos-daemon-autostart-and-executable-assessment.md`. The ADR owns the
policy and the later stability update; this note records what was observed.
Formerly tracked as AUR-318.

## Panics

Two host panics on 2026-05-08 (morning and afternoon, local time), same
signature:

- panic string: `os_refcnt: overflow (count=268435456, max=268435455) @refcnt.c:68`
- panicked task: a process named `agentscan`
- kernel backtrace: `com.apple.AppleSystemPolicy(2.0)` with AMFI, quarantine,
  and sandbox dependencies
- unified log around the first panic: `syspolicyd` and `amfid` doing
  signature and trust work (`SecKeyVerifySignature`,
  `SecTrustEvaluateIfNecessary`, misses under `/private/var/db/DetachedSignatures`,
  Security error `-67062`, Gatekeeper and provenance activity)

The panicked process had no meaningful userspace state in both cases: one
thread in kernel frames, zero user and system time, 0 pages and a 16 KiB
resident set, `codeSigningAuxiliaryInfo: 0`. The crash happened during
launch-time policy evaluation, before any Rust code ran.

## What the second panic showed

The second panic happened while a manually started foreground
`agentscan daemon run` was running in tmux. That long-running daemon appeared
in the panic report but was **not** the panicked task. The panicked task was a
separate, fresh `agentscan` exec, and another fresh `agentscan` process with
zero runtime was also present. So:

- the failure was not the long-running daemon crashing in userland;
- removing implicit daemon auto-start removed one launch path but did not
  eliminate other launches of the same binary.

Unified logs available after reboot started a few minutes after the panic
timestamp, so the parent that launched the fresh exec was not captured. No
LaunchAgent, LaunchDaemon, cron entry, or shell status hook invoked
`agentscan`; the only references were the mise install and a manual tmux popup
binding.

## Binary state at the time

Every installed copy (cargo install, mise install, `target/debug`) was ad-hoc or
linker signed (`Signature=adhoc`, no team identifier) and carried the protected
`com.apple.provenance` extended attribute. The running daemon's image matched
the cargo install.

## Response

Shipped on `main` by 2026-05-20:

- no implicit daemon auto-start on macOS for daemon-backed commands or the TUI;
  users are pointed at `agentscan daemon run` in a long-lived tmux pane
- detached `agentscan daemon start` on macOS only through the signed-binary
  preflight; `daemon run`, `stop`, and `status` skip signature assessment;
  `daemon restart` preflights the start side before stopping the old daemon
- macOS process inspection uses native `libproc` / `sysctl` instead of
  spawning `ps` / `pgrep`; Linux foreground detection reads `/proc` inline
- Developer ID signing and notarization for release binaries
  (`docs/macos-release-signing.md`)

During the incident, a host-local mitigation also renamed the installed
binaries and put logging shell wrappers in front of them so unexpected
invocations could be attributed. That was diagnostic only and later removed.

The later stability evidence, and why it does not isolate a single root cause,
is in the ADR's 2026-05-14 update.

## If it recurs

Treat a new panic after these mitigations as a fresh incident. Capture the
exact panic file and time, the panicked process fields, the unified log window
around it, and the installed binary's signing and provenance state
(`codesign -dv`, `xattr -l`).
