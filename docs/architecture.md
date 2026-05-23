# Architecture

`agentscan` is a daemon-required tmux agent scanner with short-lived
socket clients. This document captures the adopted engineering shape of the
system. Active future sequencing lives in Linear and should be reflected
back here only once behavior or contract decisions settle.

## Runtime Model

The current runtime model is:

1. Normal consumers connect to the daemon socket. On non-macOS platforms they
   auto-start the daemon when it is not already running; on macOS they require
   an already-running daemon.
2. The daemon subscribes to tmux control-mode updates.
3. The daemon maintains in-memory pane state keyed by `pane_id`.
4. The daemon publishes full versioned `SnapshotEnvelope` frames over a
   JSON-Lines Unix socket protocol.
5. Short-lived commands read one snapshot frame and disconnect; the TUI keeps a
   live subscription.

Direct tmux snapshots remain available for debugging and recovery through
`agentscan scan` and refresh-capable command flags. `--no-auto-start` and
`AGENTSCAN_NO_AUTO_START=1` are the opt-outs for CI and scripts that must not
spawn a daemon.

## Source Of Truth

Detection follows a strict precedence ladder:

1. explicit wrapper-published tmux pane metadata
2. tmux pane metadata and terminal titles
3. targeted process-tree fallback for confirmed ambiguous panes
4. tightly scoped provider-specific pane output parsing for status only, after
   provider identity has already been established

The steady-state path must stay tmux-first. Broad `ps` scans, repeated
`capture-pane` loops, and interactive-launch-time scraping are out of bounds.

Pane output is not a provider-identity source. It is a last status fallback for
providers with observed stable current prompt/footer shapes. Consumers can see
that provenance as `status.source="pane_output"` in JSON.

The daemon may reuse pane-output fallback results through a short-lived
in-memory cache keyed by pane id, provider, title, command, session, and window.
This cache is only a subprocess-throttling optimization for repeated daemon
refresh churn; it is not persisted, not serialized, and not used by direct
`scan` snapshots. Cached "no match" results are also short-lived so unsupported
or temporarily unparseable current output does not trigger repeated
`capture-pane` calls in a burst.

Daemon steady-state refresh reads use a brokered tmux control-mode command
path for `list-panes` inventory:

- targeted pane refreshes use `list-panes -t <pane>`
- window and session refreshes use `list-panes -t <target>`
- interval and timeout reconciles use `list-panes -a`

The broker shares the daemon's long-lived control-mode client, owns command
response collection while a command is pending, and replays unrelated
control-mode events back into the daemon loop before reading new socket events.
Expected missing-target responses remain normal refresh outcomes. Unexpected
broker failures disable brokered reads for the daemon lifetime and fall back to
short-lived tmux commands so the daemon can keep publishing snapshots.

The initial daemon snapshot still uses a short-lived tmux command before socket
readiness is published.

The remaining production child processes are intentional product-boundary or
lifecycle operations:

- one long-lived tmux control-mode client for daemon event subscription
- brokered control-mode `list-panes` reads for daemon steady-state refreshes
- short-lived tmux commands for initial snapshots, broker fallback, direct
  recovery scans, focus, metadata helpers, and provider-scoped pane-output
  status fallback
- detached `agentscan daemon run` self-spawn for explicit daemon start and
  non-macOS implicit auto-start
- macOS `codesign` inspection for explicit detached daemon start preflight

Process inspection itself must not shell out to `ps`, `pgrep`, `grep`, or
similar helpers. Linux uses procfs directly. macOS uses native `libproc` and
`sysctl` calls directly. macOS selected environment evidence is best-effort only
because live env visibility is not guaranteed by the current native API path.

## Canonical State Model

The canonical model is a typed Rust structure keyed by `pane_id`. It separates:

- stable pane identity and location
- raw tmux metadata
- normalized display metadata
- provider classification
- explicit status fields such as `idle`, `busy`, and `unknown`
- status provenance, including `pane_output` when a provider-scoped current
  prompt/footer pattern supplied the status
- classification reasons and diagnostics
- optional wrapper-published agent metadata

Stable raw tmux identifiers such as `session_id` and `window_id` should be
preserved when tmux exposes them so daemon refreshes can stay local to the
affected scope.

## Snapshot Contract

The canonical structured state is a versioned JSON `SnapshotEnvelope`. The
runtime transport is the daemon socket; a persisted cache file is not a
supported IPC boundary.

The snapshot envelope includes:

- `schema_version`
- `generated_at`
- `source`
- `panes`

The envelope shape should be treated as an API contract for local consumers.
Breaking changes must increment `schema_version`.

## Command Families

The command surface is organized by concern:

- `agentscan scan`: direct tmux snapshot for debugging and recovery
- `agentscan list`: normal human output and the supported JSON automation output
- `agentscan inspect <pane_id>`: one-pane diagnostics with provenance
- `agentscan focus <pane_id>`: tmux pane targeting by stable id
- `agentscan daemon`: long-lived indexer and daemon health commands
- `agentscan snapshot`: raw snapshot-envelope inspection for consumers that
  need the unfiltered envelope
- `agentscan tui`: interactive-only pane picker, not a stdout automation API
- `agentscan tmux`: tmux-facing metadata helpers

The default bare `agentscan` flow is daemon-backed `list`.

## Internal Module Boundaries

The Rust implementation keeps high-churn behavior behind small concern-focused
modules:

- provider-specific process evidence lives under `classify::proc_evidence`
  instead of the fallback orchestration path
- tmux command execution, parsing, focus/client handling, and pane metadata
  helpers are split under `tmux::*`
- TUI terminal lifecycle, state/key assignment, subscription handling, and
  frame rendering are separated under `tui::*`

These boundaries are internal, but they protect the product-level invariants
above: classification remains conservative, tmux remains the primary source, and
the TUI remains an interactive socket consumer rather than an automation API.

## Daemon Lifecycle Policy

The daemon is a hard requirement for normal consumers, but users should not have
to wire it up as a service.

Current lifecycle policy:

- auto-start by default for desktop commands on non-macOS platforms
- explicit `daemon start`, `run`, `stop`, `status`, and `restart` commands
- on macOS, implicit daemon auto-start is disabled entirely; users should start
  the daemon explicitly with foreground `agentscan daemon run`
- on macOS, explicit detached `agentscan daemon start` requires a non-ad-hoc,
  validly signed executable because detached self-exec can enter
  AppleSystemPolicy before application guards run; see
  `docs/adr/macos-daemon-autostart-and-executable-assessment.md`
- `--no-auto-start` and `AGENTSCAN_NO_AUTO_START=1` for scripts and CI
- `--refresh` for one-shot recovery or forced tmux snapshots
- fail clearly when tmux disappears or the daemon protocol is incompatible
- leave crash/restart policy to explicit commands or an external supervisor

## Design Guardrails

- No permanent fast versus full split.
- The TUI remains interactive-only and not a machine-readable contract.
- Normal automation should use `agentscan list --format json`; raw snapshot
  consumers should use `agentscan snapshot --format json`.
- Daemon health automation should use `agentscan daemon status --format json`.
- Keep shell wrappers thin; discovery and classification belong in Rust.
- Prefer honest labels from tmux metadata over richer weak inference.
- Treat pane inspection as a narrow fallback, not a primary detection strategy.
