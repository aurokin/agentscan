# Desktop Platform Posture

The desktop app is macOS-first, but its architecture should keep Linux and
Windows support possible. This document records the current platform-specific
choices and the seams future platform work should extend.

## Product Priority

Current priority order:

1. macOS local desktop app for the primary dogfood workflow.
2. SSH profiles from the desktop app to remote machines that already run
   `agentscan`.
3. Linux local desktop support after macOS behavior is stable.
4. Windows desktop support as a remote-client-first shell.

Windows local scanning is intentionally deferred. Unless a later issue changes
the product direction, Windows should connect over SSH to Linux or macOS hosts
that own tmux, the daemon, classification, picker rows, and focus actions.

## macOS-Specific Pieces Today

Global shortcut:

- Implemented with Tauri's global shortcut plugin from the React shell.
- Current fixed shortcut is `CommandOrControl+Shift+A`.
- Future Linux desktop environments and Windows may have different registration,
  permission, conflict, or focus behavior.

Window lifecycle and positioning:

- The backend owns `place_picker_window`.
- Placement currently uses Tauri monitor APIs, the cursor display when
  available, and primary-display fallback.
- Focus and raise behavior is a desktop shell concern; it should not leak into
  scanner or daemon code.

Packaging, signing, and notarization:

- The release-quality desktop path is a macOS `.app` bundle built by Tauri.
- Developer ID signing and notarization are required for release-candidate
  dogfood builds.
- Linux and Windows packaging are not part of the current release gate.

Daemon auto-start trust:

- macOS detached daemon starts require signed executable assessment.
- The desktop app delegates daemon lifecycle to the configured `agentscan` CLI.
- The desktop app should surface CLI daemon errors rather than reimplementing
  trust checks, socket handling, or tmux discovery.

Local machine assumptions:

- Local mode assumes the app can execute a local `agentscan` binary and that
  local `agentscan` owns tmux access.
- SSH mode assumes the remote host owns its own `agentscan`, daemon, tmux
  server, and focus semantics.

## Adapter Seams

Local runner:

- Current seam: `AgentscanRunner::Local` and the frontend local profile.
- Extend this for platform-specific binary discovery, environment setup, and
  local preflight guidance.
- Do not add platform-specific scanner behavior to the desktop app.

SSH runner:

- Current seam: `AgentscanRunner::Ssh` and the frontend SSH profile.
- Extend this for remote bootstrap, remote install guidance, client tty
  selection, and grouped SSH failure presentation.
- Keep the transport as command execution over SSH around JSON/JSONL CLI
  surfaces.

Window lifecycle:

- Current seam: Tauri commands such as `place_picker_window` plus frontend
  show/hide/focus orchestration.
- Extend this for platform-specific placement, tray/menu behavior, and
  focus-stealing constraints.
- Keep picker data and actions on the shared CLI contract.

Global hotkeys:

- Current seam: frontend registration through
  `@tauri-apps/plugin-global-shortcut`.
- Extend this for configurable shortcuts, platform conflicts, permission
  prompts, and alternate summon mechanisms.

Packaging:

- Current seam: `desktop/src-tauri/tauri.conf.json`,
  `docs/desktop-release-smoke.md`, and release scripts/docs.
- Extend this for Linux packages or Windows installers only when those
  platforms become explicit release targets.

Shared command contract:

- `agentscan hotkeys --format json` supplies picker rows.
- `agentscan subscribe --format json` supplies live state.
- `agentscan focus <pane-id>` activates row selections.
- `agentscan daemon status --format json` supplies lifecycle diagnostics.
- Desktop code may render these surfaces and classify transport errors, but it
  must not duplicate provider detection or tmux parsing.

## Deferred Work

Linux:

- local desktop packaging format and signing policy;
- desktop-environment-specific global shortcut behavior;
- focus/raise behavior across Wayland and X11;
- local app discovery of `agentscan` beyond the current macOS-oriented paths.

Windows:

- local tmux/scanner support;
- Windows daemon lifecycle and trust policy;
- Windows local `agentscan` binary discovery;
- Windows installer/signing pipeline;
- Windows-specific focus integration.

Remote client:

- remote install/bootstrap UX;
- explicit remote tmux target selection;
- richer remote client tty discovery beyond the current manual SSH profile
  field;
- friendlier grouped failure states for SSH auth, network, missing binary,
  incompatible output, daemon refusal, tmux absence, and stale focus targets.

## Guardrail

If a future desktop feature seems to need provider, status, pane-output,
process, or tmux semantics in the desktop app, treat that as a missing shared
CLI contract first. Add or extend an `agentscan` command surface, then keep the
desktop app as a renderer and command runner.
