# Desktop Effect Refactor Record

Historical record of the milestone that moved desktop runtime state onto Effect
services. The current desktop runtime model is described in `ROADMAP.md`
(desktop section), `docs/desktop.md`, and
`docs/adr/desktop-latch-only-daemon-launch.md`. Issue keys are history from the
retired tracker.

## Problem

Desktop UI state (connection lifecycle, profiles and settings, preflight,
appearance) had grown as ad-hoc React state reducers, refs, epoch tokens, and
`emitTo` cross-window messages. The daemon connection could wedge on terminal
events, state was hard to observe and debug, and ownership was spread across
mount-scoped effects that are fragile under two webviews, React StrictMode, and
Vite HMR.

## Approach

Move runtime state onto central, observable Effect services exposed to React
through atoms, one slice at a time, each with a clear owner, consumers, and
testable seams.

The first slice (connection lifecycle) shipped before the milestone: a
`LiveConnection` service owns the daemon subscription, epoch fencing, the
reconnect and latch schedule, runner gating, and a supervisor fiber, with
`TauriIpc` wrapping `invoke` / `listen`. The dock became a pure observer that
latches onto an existing daemon and never starts one; "Start agentscan" is the
only spawn path, and the per-snapshot `hotkeys` fetch always passes
`--no-auto-start` (regression-tested).

## Milestone slices

- Profiles and settings onto an Effect service (AUR-513).
- A Preflight service replacing the `emitTo` cross-window sync (AUR-514).
- Appearance, glass, and orientation onto an Effect service (AUR-515).
- Retire the remaining legacy state reducer and refs so `App.tsx` is a thin
  atom consumer (AUR-516, after the three above).
- Simplify the Rust live-picker worker's intra-session retry loop now that
  Effect owns re-arm, keeping the latch-only invariant and epoch fencing
  (AUR-517).
- A lightweight `daemon status` poll so latch-polling over SSH is cheaper than
  re-arming a full subscription (AUR-518).

Out of scope: changing the latch-only launch policy, TUI and one-shot CLI
behavior, and new daemon protocol surfaces.
