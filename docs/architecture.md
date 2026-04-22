# Architecture

`agentscan` is a daemon-first tmux agent scanner with short-lived cache readers.
This document captures the stable engineering shape of the system. Active
milestone sequencing lives in Linear and should be reflected back here only once
the behavior or contract has settled.

## Runtime Model

The intended steady-state model is:

1. `agentscan daemon run` takes an initial tmux snapshot.
2. The daemon subscribes to tmux control-mode updates.
3. The daemon maintains in-memory pane state keyed by `pane_id`.
4. The daemon writes a versioned JSON cache atomically.
5. Short-lived commands and popup flows read the cache instead of rescanning tmux.

Direct tmux snapshots remain available for debugging and recovery through
`agentscan scan` and `--refresh`.

## Source Of Truth

Detection follows a strict precedence ladder:

1. explicit wrapper-published tmux pane metadata
2. tmux pane metadata and terminal titles
3. targeted `/proc` fallback for confirmed ambiguous panes
4. incremental pane output parsing only if later justified

The steady-state path must stay tmux-first. Broad `ps` scans, repeated
`capture-pane` loops, and popup-time scraping are out of bounds.

## Canonical State Model

The canonical model is a typed Rust structure keyed by `pane_id`. It separates:

- stable pane identity and location
- raw tmux metadata
- normalized display metadata
- provider classification
- explicit status fields such as `idle`, `busy`, and `unknown`
- classification reasons and diagnostics
- optional wrapper-published agent metadata

Stable raw tmux identifiers such as `session_id` and `window_id` should be
preserved when tmux exposes them so daemon refreshes can stay local to the
affected scope.

## Cache Contract

The persisted cache is a versioned JSON snapshot. It is the first canonical
persisted contract and should remain easy to inspect manually, serialize in
fixtures, and evolve through additive schema changes when possible.

The cache envelope includes:

- `schema_version`
- `generated_at`
- `source`
- `panes`

The cache should be treated as an API contract for local consumers. Breaking
changes must increment `schema_version`.

## Command Families

The current command surface is organized by concern:

- `agentscan scan`: direct tmux snapshot for debugging and recovery
- `agentscan list`: normal human output and supported JSON automation output
- `agentscan inspect <pane_id>`: one-pane diagnostics with provenance
- `agentscan focus <pane_id>`: tmux pane targeting by stable id
- `agentscan daemon`: long-lived indexer and daemon health commands
- `agentscan cache`: cache path, validation, and raw cache inspection
- `agentscan popup`: interactive-only pane picker
- `agentscan tmux`: tmux-facing metadata helpers

The default bare `agentscan` flow is cache-backed `list`.

## Daemon Lifecycle Policy

The daemon is an explicit entrypoint. Short-lived commands stay passive by
default.

Current lifecycle direction:

- explicit daemon startup first
- `--refresh` for one-shot recovery or forced tmux snapshots
- fail fast when tmux disappears
- leave restart policy to an external supervisor

## Design Guardrails

- No permanent fast versus full split.
- Popup remains interactive-only and not a machine-readable contract.
- Keep shell wrappers thin; discovery and classification belong in Rust.
- Prefer honest labels from tmux metadata over richer weak inference.
- Treat pane inspection as a narrow fallback, not a primary detection strategy.
