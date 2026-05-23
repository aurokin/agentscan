# Daemon Redesign Decisions

This note records implementation decisions made during the daemon redesign
slices so they can be reviewed together at the end of the work.

## Slice 1: Refresh Transport Boundary

- The current code already has a `TmuxReadProvider` boundary for daemon pane,
  scope, and full-snapshot refreshes.
- I treated that as satisfying the first migration slice because the refresh
  tests already exercise fake providers without spawning tmux.
- I did not move direct `scan`, `--refresh`, focus, metadata, lifecycle, or
  macOS signing paths behind this boundary.

## Slice 2: Snapshot Store Extraction

- `SnapshotStore` owns only the latest typed snapshot, encoded snapshot frame,
  and update telemetry.
- Subscriber registration, pending handshakes, lifecycle state, and fanout
  remain in `DaemonSocketState`, because they are socket-server concerns rather
  than snapshot-storage concerns.
- Oversized later snapshots still fail before mutating the store, preserving
  the previous good frame for snapshot clients and subscribers.

## Slice 3: Broker Transcript Harness

- The first broker harness is test-only and does not change production daemon
  wiring.
- The harness models scripted control-mode lines plus explicit timeout and EOF
  steps so broker behavior can be tested without live tmux flake.
- It treats subscription lines before a command frame as deferred events and
  also defers known async control-mode events that arrive while a command frame
  is open.
- It treats nested command frames, matching `%error`, timeout, and EOF as
  command failures.

## Slice 4: Targeted Broker Command Prototype

- Targeted `list-panes -t <pane>` behavior is still test-only and runs on the
  transcript harness, not the production daemon loop.
- The prototype records the exact command that would be written, parses rows
  through the existing tmux pane parser, and carries deferred async events back
  to the caller.
- Missing pane/window `%error` messages map to `None`, matching the current
  short-lived tmux command behavior, while preserving deferred events observed
  before the missing-target response.

## Slice 5: Brokered Targeted Pane Refresh

- Production pane-level daemon refreshes now try the long-lived control-mode
  stream first for `list-panes -t <pane>`.
- Window/session refreshes and full reconcile reads remain on the short-lived
  tmux command provider.
- Broker-consumed async events are queued back into the daemon loop before new
  socket reads, preserving event ordering as closely as possible during this
  transitional slice.
- Lines that have the tmux pane-row delimiter are treated as command output
  before control-frame or async-event classification, because user-controlled
  session names can otherwise look like control-mode notifications or command
  markers.
- If the brokered pane read fails unexpectedly, the daemon logs the failure and
  falls back to the existing short-lived pane read rather than dropping the
  targeted refresh.

## Slice 6: Brokered Window/Session Refresh

- Window and session daemon refreshes now use the same brokered `list-panes -t
  <target>` command path as pane refreshes.
- Full reconcile reads remain on the short-lived `list-panes -a` command path.
- Scope refresh missing-target handling accepts pane/window/session missing
  errors; pane refresh keeps the narrower pane/window missing-target behavior.

## Slice 7: Brokered Full Reconcile

- Steady-state daemon reconcile reads now use brokered `list-panes -a` when the
  broker is healthy.
- The initial daemon snapshot still uses the existing short-lived snapshot path
  before socket readiness is published.
- Unexpected broker failures during full reconcile disable future brokered reads
  for the daemon lifetime and fall back to the short-lived `list-panes -a`
  command path.
- The same poison-on-unexpected-error policy applies to brokered pane and scope
  reads; expected missing-target responses are converted to successful `None`
  results before this error path.

## Slice 8: `capture-pane` Transport Boundary

- Provider-scoped `capture-pane` status fallback remains a short-lived tmux
  command.
- The fallback is already constrained by provider identity, unknown status, and
  a short daemon-local cache, so it is lower priority than canonical
  `list-panes` inventory reads.
- Keeping it outside the broker avoids mixing scrollback-sensitive parsing with
  the control-mode inventory transport while the broker is still being hardened.

## Slice 9: Broker Failure Recovery

- Broker health is tracked explicitly instead of as an anonymous boolean.
- Any unexpected broker command failure falls back to the short-lived tmux
  provider for the current pane, scope, or reconcile read.
- Expected missing-target responses remain successful refresh outcomes and do
  not poison broker health.
- Empty `%error` marker payloads use the last command output line as the error
  message, matching tmux control-mode framing for missing-target errors.
- After a poisoned broker read, the daemon attempts to attach a replacement
  control-mode client. A successful reconnect clears the disabled reason,
  increments the reconnect count, and returns later refreshes to the brokered
  path. A failed reconnect leaves the existing event stream alive and keeps
  short-lived tmux reads as fallback.
- `agentscan daemon status` now reports broker mode, disabled reason, and
  reconnect count through lifecycle text and JSON output.
- Faster brokered pane removals exposed an existing TUI race where render could
  prune a key target before the user selected it. TUI state now keeps retired
  key targets so selecting a just-removed pane still closes with the existing
  "pane is no longer available" flow.

## Slice 10: Daemon Module Split

- `SnapshotStore` moved out of the monolithic daemon module into
  `src/app/daemon/snapshot_store.rs`.
- The control-mode runtime, brokered read provider, command-frame parser, and
  broker transcript harness moved into `src/app/daemon/control_mode.rs`.
- The daemon loop still owns when to refresh, recover, publish, and reconcile;
  the control-mode module owns how brokered tmux reads and reconnect state work.
