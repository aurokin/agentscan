# Changelog

## Unreleased

### Added

- Added Antigravity CLI provider identity detection for exact `agy` command
  panes, with status kept conservative until direct state evidence is available.
- Added `agentscan daemon status --format json` for machine-readable daemon
  lifecycle and readiness checks.

## 0.2.6 - 2026-05-13

### Safety

- Replaced macOS process-inspection fallback shell-outs to `ps` and `pgrep`
  with native `libproc` and `sysctl` calls, reducing executable policy churn
  during daemon refreshes while preserving Linux `/proc` behavior.

## 0.2.5 - 2026-05-08

### Release

- Added local scripts and GitHub Actions release steps for Developer ID signing
  and notarization of macOS release binaries.

## 0.2.4 - 2026-05-08

### Safety

- Removed implicit daemon auto-start on macOS. Daemon-backed commands now require
  an already-running daemon on macOS and guide users to run
  `agentscan daemon run` in a long-lived tmux pane; explicit detached
  `agentscan daemon start` remains signed-binary-only.

### Diagnostics

- Added daemon snapshot update telemetry to `agentscan daemon status`, including
  the latest update source, detail, and update duration.

## 0.2.3 - 2026-05-08

### Safety

- Removed the debug override for detached macOS daemon starts from untrusted
  binaries. Ad-hoc local builds must use foreground `agentscan daemon run`;
  detached `agentscan daemon start` remains available for signed binaries.

## 0.2.2 - 2026-05-07

### Safety

- Block detached macOS daemon starts for ad-hoc or invalidly signed
  executables, including explicit `agentscan daemon start`; use
  `agentscan daemon run` for foreground debugging or a signed release binary
  for detached daemon operation.
- Removed `spctl` from daemon-start preflight so startup checks do not invoke
  the AppleSystemPolicy/Gatekeeper assessment path.

## 0.2.1 - 2026-05-06

### Safety

- Disabled implicit daemon auto-start on macOS for ad-hoc, invalidly signed, or
  Gatekeeper-rejected executables to avoid re-triggering a macOS
  AppleSystemPolicy panic observed after the 0.2.0 auto-start rollout.
- Kept recovery paths available: `agentscan scan`, refresh-capable one-shot
  commands, and foreground `agentscan daemon run` do not depend on detached
  daemon auto-start.
- Added `AGENTSCAN_ALLOW_UNTRUSTED_DAEMON_AUTOSTART=1` as a debugging-only
  override for intentional local reproduction.
- Logged daemon start preflight context before spawning the daemon, so failed
  or blocked starts have actionable diagnostics in the daemon log.

## 0.2.0 - 2026-05-06

### Highlights

- The daemon now auto-starts on first use. Normal `agentscan`, `list`,
  `inspect`, `focus`, `snapshot`, and `tui` invocations spawn a background
  daemon if one is not already running, then read state from its socket. No
  manual `daemon start` is required for everyday use.

### Breaking Changes

| Previous surface | Current surface |
|------------------|-----------------|
| `agentscan popup` for human pane picking | `agentscan tui` |
| TUI-shaped stdout parsing | `agentscan list --format json` |
| Raw cache/envelope inspection through `agentscan cache` | `agentscan snapshot --format json` |
| Cache file IPC and `AGENTSCAN_CACHE_PATH` | Daemon socket snapshots |
| Implicit direct tmux reads in normal consumers | Daemon-backed reads with explicit `scan` or `--refresh` recovery |

- Normal `agentscan`, `list`, `inspect`, `focus`, `snapshot`, and `tui` flows
  now read daemon socket state by default.
- Normal consumers auto-start the daemon. Use `--no-auto-start` or
  `AGENTSCAN_NO_AUTO_START=1` when a script or CI job must not spawn a daemon;
  opt-out failures do not fall back to direct tmux reads.
- `agentscan scan` remains daemon-free and always reads tmux directly.
- Supported `--refresh` flags remain the direct tmux bypass for one-shot
  recovery and debugging.
- `agentscan tui` is interactive-only and subscribes to live daemon socket
  snapshots. It has no cache bootstrap, no direct tmux discovery fallback, and
  no machine-readable `--format` mode. Pane selection still uses the normal tmux
  focus behavior.
- The `agentscan cache` command family is removed, including `cache path` and
  `cache validate`.
- The persisted cache file is not a supported IPC boundary. Snapshot JSON may
  still include compatibility vocabulary such as `diagnostics.cache_origin`;
  that field does not indicate an active cache transport.
- There is no cache-file IPC replacement. Socket-isolated tests and harnesses
  should use `AGENTSCAN_SOCKET_PATH` when they need a non-default daemon socket.

### Operator Notes

- Use `agentscan list --format json` for normal automation.
- Use `agentscan list --all --format json` when automation intentionally needs
  non-agent panes.
- Use `agentscan snapshot --format json` only for raw `SnapshotEnvelope`
  consumers.
- Keep tmux key bindings and other human-facing launch paths on
  `agentscan tui`.
- Tmux `display-popup` remains a valid way to launch the binary inside tmux;
  it is separate from the removed `agentscan popup` command.

### Daemon Lifecycle

- `agentscan daemon run` runs the daemon in the foreground.
- `agentscan daemon start` detaches a background daemon through the same path.
- `agentscan daemon stop`, `daemon status`, and `daemon restart` manage the
  running daemon over the socket.

### Providers

- Added Hermes provider detection.
