# Architecture

`agentscan` is a daemon-required tmux agent scanner with short-lived
socket clients. This document captures the adopted engineering shape of the
system. Active future sequencing lives in Linear and should be reflected
back here only once behavior or contract decisions settle.

## Runtime Model

The current runtime model is:

1. Normal consumers connect to the daemon socket and auto-start the daemon when
   it is not already running. On macOS, the parent command first runs executable
   trust preflight and only starts detached signed/trusted binaries.
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
broker failures fall back to short-lived tmux commands for the current read so
the daemon can keep publishing snapshots, then attempt to reconnect the
control-mode client. `agentscan daemon status` reports whether the broker is
active or in fallback, the last disabled reason, the reconnect count, and the
fallback count.

`agentscan daemon status` also reports cumulative runtime telemetry for event
and reconcile behavior. The counters separate control-mode-driven refreshes,
full reconcile attempts, no-op reconciles, reconciles that found material
snapshot changes, targeted refreshes that fell back to full snapshots, and
broker fallback activations. Reconcile materiality ignores timestamp-only
differences and cache-origin churn so the counters can show whether the safety
loop is catching missed state changes or mostly confirming the event stream.
No-op interval and timeout reconciles update telemetry but do not fan out
snapshot frames to subscribers; only materially changed reconcile results
publish a new snapshot.

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
  implicit auto-start
- macOS `codesign` inspection before detached daemon start

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
- `agentscan subscribe`: JSON Lines daemon subscription stream for live local
  or SSH-transported clients
- `agentscan providers`: supported provider names, icon modes, marker
  codepoints, and matching aliases
- `agentscan hotkeys`: stable picker-row model for tmux binds, terminal
  surfaces, and desktop picker surfaces
- `agentscan hotkey <key>`: activate a stable picker-row key through the shared
  focus path with strict non-zero failures for automation callers
- `agentscan tui`: interactive-only pane picker, not a stdout automation API
- `agentscan tmux`: tmux-facing helpers, including metadata commands and
  `tmux hotkey` for display-message failure reporting from tmux binds

The default bare `agentscan` flow is daemon-backed `list`.

Icon mode is a presentation concern resolved by short-lived consumers from CLI,
environment, config file, then the built-in default. It must not affect daemon
snapshot payloads, provider classification, or socket protocol state.

Picker key order is a shared picker concern resolved from the core config file.
Consumers should render keys returned by `agentscan hotkeys --format json`
instead of assuming the built-in default order.

## Internal Module Boundaries

The Rust implementation keeps high-churn behavior behind small concern-focused
modules:

- live client subscription events live outside daemon and TUI modules so
  terminal and JSONL stream consumers share one event vocabulary
- the desktop backend owns process supervision for `agentscan subscribe`, while
  picker row shaping remains delegated to the CLI hotkeys contract
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

- auto-start by default for desktop commands
- explicit `daemon start`, `run`, `stop`, `status`, and `restart` commands
- on macOS, any detached daemon start requires a non-ad-hoc, validly signed
  executable because detached self-exec can enter AppleSystemPolicy before
  application guards run; see
  `docs/adr/macos-daemon-autostart-and-executable-assessment.md`
- `--no-auto-start` and `AGENTSCAN_NO_AUTO_START=1` for scripts and CI
- `--refresh` for one-shot recovery or forced tmux snapshots
- fail clearly when tmux disappears or the daemon protocol is incompatible
- leave crash/restart policy to explicit commands or an external supervisor

## Desktop And SSH Client Boundary

Desktop surfaces are thin clients over the same command families. A local
desktop runner executes `agentscan` directly. A remote desktop runner executes
the same commands through SSH, using the user's normal SSH configuration and
authentication. Both runners consume stdout JSON/JSONL, stderr, exit status,
and cancellation; neither runner connects to tmux or the daemon Unix socket
directly.

The scanner contract remains on the machine that owns tmux:

- local desktop target: local `agentscan` owns daemon lifecycle and tmux access
- remote desktop target: remote `agentscan` owns daemon lifecycle and tmux access
- desktop shell: owns host selection, process supervision, rendering, keyboard
  lifecycle, and error presentation

On macOS, the desktop shell also owns app-global shortcut registration for
summoning the picker window. The initial fixed shortcut is
`CommandOrControl+Shift+A`; customization, multi-monitor positioning, and
tmux-prefix-originated launch are separate product slices. The shortcut only
controls desktop window lifecycle. Picker data and focus actions still flow
through `agentscan` command surfaces.

The primary remote design is command execution over SSH, not socket forwarding.
Remote install/bootstrap UX is outside the scanner contract and should be
handled as a desktop product follow-up.

See `docs/desktop-client-contract.md` for the detailed command contract,
failure surfaces, and remote smoke plan.

## Design Guardrails

- No permanent fast versus full split.
- The TUI remains interactive-only and not a machine-readable contract.
- Normal automation should use `agentscan list --format json`; raw snapshot
  consumers should use `agentscan snapshot --format json`.
- Live automation and desktop clients should use
  `agentscan subscribe --format json` rather than connecting directly to the
  daemon Unix socket.
- Daemon health automation should use `agentscan daemon status --format json`.
- Keep shell wrappers thin; discovery and classification belong in Rust.
- Prefer honest labels from tmux metadata over richer weak inference.
- Treat pane inspection as a narrow fallback, not a primary detection strategy.
