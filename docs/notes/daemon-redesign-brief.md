# Daemon Redesign Brief

This brief prepares the future daemon redesign. It is intentionally design
prep, not an implementation plan for this slice.

## Source Material

The relevant portless-inspired reference is the webmux local development
surface:

- `/Users/auro/code/webmux/docs/decisions/0006-portless-local-development.md`
- `/Users/auro/code/webmux/portless.json`

The useful ideas are not the browser hostnames themselves. The transferable
pattern is:

- one normal product entrypoint starts and coordinates multiple local services
- lower-level raw server commands remain available for debugging
- services accept assigned endpoints instead of owning fixed global addresses
- concurrent worktrees/checkouts avoid collisions by deriving runtime identity
  from context
- harnesses test non-default runtime endpoints instead of assuming defaults

For `agentscan`, the analogous endpoint is the daemon socket plus tmux server
identity, not an HTTP port.

## Current Contracts To Preserve

The redesign must preserve these current product and integration contracts:

- `SnapshotEnvelope` remains the canonical structured state model.
- `schema_version` remains the snapshot compatibility boundary.
- The daemon socket keeps a separately versioned JSON-Lines protocol with
  strict `hello` / `hello_ack` negotiation.
- `agentscan list --format json` remains the supported normal automation
  surface.
- `agentscan snapshot --format json` remains the raw snapshot-envelope surface.
- One-shot daemon-backed commands read a complete snapshot and disconnect.
- TUI subscription keeps receiving live snapshot frames and remains
  interactive-only.
- `agentscan scan` and refresh-capable flags remain direct tmux recovery paths.
- macOS daemon-backed commands may implicitly auto-start the daemon only after
  parent-side signed-binary trust preflight succeeds.
- Explicit detached `agentscan daemon start` on macOS remains signed-binary
  gated through the same preflight.
- `--no-auto-start` and `AGENTSCAN_NO_AUTO_START=1` remain hard opt-outs.
- `AGENTSCAN_SOCKET_PATH` remains the daemon socket override for tests and
  isolated workflows.
- `AGENTSCAN_TMUX_SOCKET` remains the tmux-server isolation override for
  harnessed `agentscan` subprocesses.
- Provider detection remains tmux-first, conservative, and plug-and-play.

## Current Tmux Process Boundary

Keep these short-lived subprocess paths outside the redesign unless a later
slice proves there is a meaningful benefit:

- direct `agentscan scan` fresh snapshots
- one-shot `--refresh` recovery paths
- focus and `display-message` helpers
- tmux metadata set/unset helpers
- macOS `codesign` preflight
- daemon self-spawn lifecycle

Candidates for redesign:

- daemon targeted `list-panes -t <pane>` refreshes
- daemon window/session `list-panes -t <scope>` refreshes
- daemon full `list-panes -a` reconcile reads
- daemon control-mode client recovery when tmux restarts or disappears

Provider-scoped `capture-pane` status fallback should remain a narrow fallback.
AUR-339 already throttles it with a short-lived daemon-local cache. Moving it
behind a broker is optional and lower priority than `list-panes` reads.

## Target Shape

The future daemon should have one explicit runtime owner and one explicit tmux
stream owner.

Suggested components:

- `DaemonRuntime`: owns lifecycle, socket publication, shutdown, and recovery.
- `SnapshotStore`: owns the latest typed snapshot and publish sequencing.
- `TmuxBroker`: owns the long-lived tmux control-mode child, stdout reader,
  stdin command writes, command request queue, and event dispatch.
- `TmuxReadProvider`: trait used by refresh logic so current subprocess reads
  and future broker-backed reads can coexist during migration.
- `RefreshEngine`: converts tmux rows plus process evidence into pane records
  without owning transport details.

The broker should be the only component that reads control-mode stdout. It
should route:

- subscription events to the daemon refresh loop
- command responses by frame id
- `%error` frames into typed command failures
- EOF, timeout, and malformed-frame cases into recovery signals

The daemon socket remains the public state boundary. The broker is an internal
tmux transport implementation detail, not a new integration API.

## Broker Requirements

A safe control-mode command broker needs:

- one owned stdout reader
- serialized stdin command writes
- frame id correlation for `%begin`, `%end`, and `%error`
- bounded command timeouts distinct from startup subscription timeouts
- event buffering or immediate event dispatch while command responses are
  pending
- missing-target parity with existing `tmux list-panes` stderr matching
- poisoned-client recovery after EOF, timeout, partial frames, or tmux server
  disappearance
- diagnostics that preserve the failed command, frame id, and last event lines

The AUR-340 prototype shows frame parsing is straightforward. The hard part is
ordering and stream ownership.

## Migration Slices

Future implementation should stay incremental:

1. Extract refresh transport behind `TmuxReadProvider` while keeping the current
   short-lived tmux command implementation.
2. Extract daemon snapshot publishing into a small `SnapshotStore` helper.
3. Introduce a broker test harness with scripted control-mode transcripts and
   no production wiring.
4. Add broker command request/response support for `list-panes -t <pane>` in
   tests.
5. Wire targeted pane refreshes through the broker behind the same
   `TmuxReadProvider` contract.
6. Move window/session refreshes through the broker.
7. Move full reconcile reads through the broker only after targeted refreshes
   are stable.
8. Decide whether `capture-pane` fallback remains a short-lived command or
   moves behind the broker.
9. Harden tmux disappearance, reconnect, timeout, and poisoned-client recovery.
10. Revisit macOS auto-start only after the redesigned daemon is stable and
    signed-release behavior is tested.

Each slice should preserve direct `scan`, `--refresh`, focus, metadata helpers,
and socket protocol compatibility.

## Risks

Event ordering:

- Socket subscribers must not see stale or duplicated snapshots when command
  responses and subscription events interleave.

Deadlocks:

- No daemon task should block waiting for a command response while also being
  responsible for draining the control-mode stream that would deliver it.

Recovery:

- A partial command frame, tmux EOF, or timeout must not leave the daemon
  publishing silently stale data.

Missing-target parity:

- Pane/window/session disappearance must continue to remove or refresh only the
  affected scope, matching current short-lived command behavior.

macOS lifecycle:

- The redesign must not reintroduce unassessed macOS auto-start or child-process
  assessment workarounds.

Harness flake:

- Real tmux integration tests already have parallelism-sensitive areas. Broker
  tests should prefer deterministic transcript fixtures until production wiring
  requires live tmux coverage.

## Harness Impact

Add or preserve harness coverage for:

- scripted control-mode command frames, including interleaved `%begin`, `%end`,
  `%error`, `%output`, and subscription lines
- broker timeout and EOF behavior
- missing pane/window/session targets
- daemon socket publication order after rapid topology changes
- `AGENTSCAN_TMUX_SOCKET` propagation into any broker-backed tmux child
- non-default `AGENTSCAN_SOCKET_PATH`
- macOS no-auto-start expectations

Live integration tests should continue to isolate tmux with harness sockets and
temp `TMUX_TMPDIR`. If broker wiring increases parallel flake, prefer targeted
serialized harness tests over weakening product assertions.

## Decision

The daemon redesign should be prepared around a brokered tmux control-mode
transport, but implementation should wait for separate slices. The current
short-lived daemon `list-panes` reads are acceptable until the broker exists.

The first implementation slice should be an internal transport abstraction,
not a control-mode rewrite.
