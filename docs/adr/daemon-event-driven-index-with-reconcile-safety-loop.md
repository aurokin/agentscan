# ADR: Daemon Event-Driven Index With Reconcile Safety Loop

Status: accepted
Date: 2026-05-24

## Context

`agentscan` is moving from repeated direct tmux scans toward a daemon that
maintains live pane state. Tmux control-mode events are the desired primary
signal because they avoid repeated subprocess work and let terminal, TUI, and
desktop clients share the same live model.

Control-mode events are not yet trusted enough to remove periodic and targeted
recovery reads. Real tmux workflows can include missed, ambiguous, or
out-of-order updates, and desktop clients need stable behavior before the
daemon is redesigned further.

## Decision

Keep the daemon event-driven first, with a reconcile safety loop and observable
fallback counters.

The daemon should:

- subscribe to tmux control-mode events as the primary steady-state input;
- refresh the narrowest reliable scope for control events when pane, window, or
  session identity is available;
- use a brokered control-mode command path for steady-state `list-panes` reads;
- keep periodic/timeout reconcile passes as a safety net;
- publish subscription updates only for material snapshot changes;
- keep no-op reconcile passes silent on the live stream;
- expose counters through `agentscan daemon status --format json` so developers
  can see when reconcile or fallback behavior is doing real work.

Reconcile materiality should ignore timestamp-only changes and cache-origin
churn. The safety loop is for missed state, not for creating heartbeat frames.

## Consequences

- TUI and desktop clients can treat `agentscan subscribe --format json` as the
  shared live model.
- Developers can use daemon status telemetry to decide when event coverage is
  reliable enough to reduce or remove safety-loop work.
- The daemon remains robust while control-mode handling is hardened.
- Desktop code stays out of tmux parsing and scanner logic.
- Repeated `capture-pane`, broad process scans, and repeated short-lived tmux
  reads remain outside the normal steady-state path.

## Non-Goals

- Do not remove the reconcile loop until telemetry and real usage justify it.
- Do not add subscription heartbeat snapshots for no-op reconcile passes.
- Do not make desktop or TUI clients connect directly to the daemon socket.
- Do not use broad `ps`, `pgrep`, `grep`, or repeated `capture-pane` loops as
  the default event-recovery strategy.

## Follow-Ups

- Track how often reconcile changes snapshots that control events missed.
- Keep daemon status JSON stable enough for developer diagnostics.
- Use future daemon redesign work to tighten lifecycle and transport internals
  without changing the shared TUI/desktop client model.
