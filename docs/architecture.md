# Architecture

`agentscan` is moving to a daemon-required tmux agent scanner with short-lived
socket clients. This document captures the adopted engineering shape of the
system. Active milestone sequencing lives in Linear and should be reflected
back here only once behavior or contract decisions settle.

## Runtime Model

The intended steady-state model is:

1. Normal consumers connect to the daemon socket and auto-start the daemon when
   it is not already running.
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
`capture-pane` loops, and popup-time scraping are out of bounds.

Pane output is not a provider-identity source. It is a last status fallback for
providers with observed stable current prompt/footer shapes. Consumers can see
that provenance as `status.source="pane_output"` in JSON.

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
target transport is the daemon socket; a persisted cache file is no longer the
steady-state IPC boundary.

The snapshot envelope includes:

- `schema_version`
- `generated_at`
- `source`
- `panes`

The envelope shape should be treated as an API contract for local consumers.
Breaking changes must increment `schema_version`.

## Command Families

The target command surface is organized by concern:

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

Current lifecycle direction:

- auto-start by default for desktop commands
- explicit `daemon start`, `stop`, `status`, and `restart` commands
- `--no-auto-start` and `AGENTSCAN_NO_AUTO_START=1` for scripts and CI
- `--refresh` for one-shot recovery or forced tmux snapshots
- fail clearly when tmux disappears or the daemon protocol is incompatible
- leave crash/restart policy to explicit commands or an external supervisor

## Design Guardrails

- No permanent fast versus full split.
- The TUI remains interactive-only and not a machine-readable contract.
- Normal automation should use `agentscan list --format json`; raw snapshot
  consumers should use `agentscan snapshot --format json`.
- Keep shell wrappers thin; discovery and classification belong in Rust.
- Prefer honest labels from tmux metadata over richer weak inference.
- Treat pane inspection as a narrow fallback, not a primary detection strategy.
