# ADR: Desktop Shell And Shared Client Contract

Status: accepted
Date: 2026-05-23

## Context

`agentscan` is moving from a terminal-only workflow toward a desktop hotkey
picker. The desktop app should make agent panes easier to summon and navigate,
but it must not become a second scanner. The scanner, daemon, provider
classification, status inference, pane-output fallback, and tmux focus behavior
already live in the Rust CLI/daemon contract.

Recent contract work established the shared surfaces the desktop can consume:

- `agentscan subscribe --format json` for live JSON Lines client events
- `agentscan hotkeys --format json` for the picker row model
- `agentscan hotkey <key>` for shared picker-key activation
- `agentscan focus <pane_id>` for direct row activation
- `agentscan daemon status --format json` for lifecycle, readiness, telemetry,
  and compatibility diagnostics

The terminal TUI and the future desktop app should be peers over this contract.
They may render differently, but they should consume the same live model and
invoke the same action surfaces.

## Decision

Build the first desktop app as a macOS-first Tauri 2 application:

- shell: Tauri 2
- native/backend layer: Rust
- frontend: React and TypeScript
- styling/state: small app-local choices, selected by the desktop slice, with
  no dependency on scanner internals

The desktop app is a command runner and renderer. It owns:

- global hotkey registration
- window/display lifecycle
- local and remote profile selection
- process supervision, stdout/stderr capture, exit handling, cancellation, and
  reconnect policy
- rendering picker rows and live connection/error state
- keyboard and mouse interaction inside the picker

`agentscan` continues to own:

- tmux discovery and control-mode interaction
- daemon lifecycle and socket protocol internals
- provider classification and status inference
- pane-output parsing and fallback decisions
- picker row assignment and focus/action semantics

The first implementation target is local macOS. The app should still model
execution through a runner boundary from the start:

- `LocalRunner` executes `agentscan ...` directly.
- `SshRunner` later executes the same command argv over SSH.

Remote clients use SSH as command transport around the same CLI surfaces. They
do not forward daemon sockets as the primary design, parse remote tmux directly,
or implement a desktop-private scanner protocol.

## Options Considered

### Tauri 2 With Rust Backend And React/TypeScript UI

Pros:

- keeps native process, window, hotkey, signing, and packaging work close to the
  existing Rust codebase
- ships a smaller shell than Electron while still allowing a productive web UI
- fits a picker/list app where the UI is interactive but not a full terminal
  emulator
- keeps local and SSH command runners in Rust, where process supervision and
  platform behavior are easier to test carefully
- can support macOS first while preserving a plausible Linux and Windows
  application shell later

Cons:

- Tauri-specific plugin and packaging behavior will need verification on macOS
  before release
- global hotkey and focus behavior can still vary by desktop environment on
  later Linux targets
- Windows local scanner support remains a separate product question because
  `agentscan` is tmux-centered

### Electron

Pros:

- mature global hotkey, tray, window, auto-update, and frontend ecosystem
- familiar path for fast desktop UI iteration
- broad cross-platform packaging story

Cons:

- heavier runtime than this app needs
- process supervision and signing concerns would be split further from the Rust
  CLI/daemon code
- easier to drift toward a JavaScript-side product core or duplicated scanner
  behavior

Electron remains viable if Tauri blocks core UX, but it is not the first choice
for a small command-runner picker.

### Lightweight Zig/Webview Or Zero-Native-Style Shell

Pros:

- potentially very small and fast
- attractive long-term direction for native shells around web UIs
- may become compelling if the ecosystem stabilizes around this use case

Cons:

- less proven for this app's immediate needs: global hotkey behavior, focused
  floating windows, process streaming, signing/notarization, and release
  packaging
- would add a new systems-language toolchain beside the existing Rust product
  core
- higher integration risk before the product behavior is validated

This direction is worth watching, not choosing for the MVP.

### Native macOS-First App

Pros:

- best chance at platform-native window feel, focus behavior, and system
  integration on the first target
- direct access to macOS APIs without webview/plugin abstraction

Cons:

- splits implementation away from the existing Rust product core
- makes future Linux and Windows shells more expensive
- forces more UI/state work into platform-specific code before the product loop
  is proven

Native macOS is a reasonable later optimization only if Tauri cannot deliver the
hotkey/window experience.

## Platform Posture

macOS local mode is the priority. The first desktop milestones should optimize
for the current Mac workflow and signed local release path.

Linux remains a plausible later local target because `agentscan` already has
Linux process evidence and tmux support. The app should avoid baking macOS-only
assumptions into shared state, runner, or live-client models.

Windows is remote-client-first. The desktop shell can run on Windows in the
future and connect over SSH to Linux or macOS hosts running `agentscan`. Local
Windows scanning is not part of the initial design because `agentscan` is built
around tmux.

`docs/desktop-platform-posture.md` is the working platform posture note. It
records the macOS-specific pieces in the current desktop app, the adapter seams
that future Linux and Windows work should extend, and the platform work
intentionally deferred out of the macOS MVP.

## Client And Daemon Lifecycle

The desktop app does not own daemon internals. It invokes daemon-backed CLI
surfaces and displays the resulting lifecycle state.

The live local path is:

```text
desktop LocalRunner
  -> agentscan subscribe --format json
  -> JSON Lines live client events
  -> desktop state and renderer
```

The local action path uses one of the shared action surfaces:

```text
desktop selection
  -> agentscan focus <pane_id>        # normal row-based desktop selection
  -> agentscan hotkey <key>           # when replaying an assigned picker key
```

The remote path is the same command contract over SSH:

```text
desktop SshRunner
  -> ssh <host> agentscan subscribe --format json
  -> JSON Lines live client events
  -> desktop state and renderer
```

Desktop preflight and diagnostics use:

```text
agentscan daemon status --format json
```

For remote targets, the SSH runner executes the same command remotely and
surfaces SSH failures, missing binary errors, daemon auto-start refusal, tmux
availability problems, and compatibility failures without falling back to
desktop-side scanning.

## Consequences

- The desktop app can be implemented in slices without waiting for a new daemon
  protocol.
- The TUI and desktop stay aligned because they consume the same event/action
  vocabulary.
- `agentscan hotkeys --format json` remains the stable picker model instead of
  recreating row assignment in UI code.
- macOS signing and global hotkey behavior are first-class release concerns for
  the desktop shell.
- Later Linux and Windows work should extend runner/window/platform layers, not
  scanner semantics.

## Non-Goals

- Do not link scanner internals directly into the desktop app.
- Do not parse tmux directly from desktop code.
- Do not add provider, title, process, pane-output, or status heuristics to the
  frontend or desktop backend.
- Do not forward daemon Unix sockets as the primary SSH design.
- Do not add a terminal emulator to the desktop MVP.
- Do not make Windows local scanning part of the first desktop plan.

## Follow-Ups

- Scaffold the macOS-first Tauri app with a local profile and no scanner logic.
- Add the local runner and picker data load through
  `agentscan hotkeys --format json`.
- Add selection and focus actions through `agentscan hotkey` and
  `agentscan focus`.
- Add global hotkey and picker window lifecycle.
- Move from one-shot picker loads to `agentscan subscribe --format json`.
- Add local profile settings and command debug output before the SSH spike.
