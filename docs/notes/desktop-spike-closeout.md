# Desktop Spike Closeout

This note records the stop/go decision for the first macOS desktop spike.

## Decision

Go: continue hardening the Tauri desktop app instead of running another
architecture spike.

The spike proved the important product and architecture claims:

- a macOS desktop shell can consume the same picker model as terminal workflows;
- local and SSH targets can share one command-runner contract;
- live desktop state can come from `agentscan subscribe --format json`;
- picker rendering can use `agentscan hotkeys --format json`;
- activation can remain delegated to `agentscan focus <pane-id>`;
- the desktop app does not need scanner, provider, title, process, pane-output,
  tmux, or status heuristics of its own.

This is not a release-hardening decision. The current app is a viable MVP
foundation, not a polished desktop product.

## Shipped Spike Behavior

The desktop shell now has:

- a macOS-first Tauri 2 app with a Rust backend and React/TypeScript frontend;
- a local profile that preflights the configured `agentscan` binary;
- local picker loading through `agentscan hotkeys --format json`;
- keyboard selection and row activation through `agentscan focus <pane-id>`;
- an app-global `CommandOrControl+Shift+A` shortcut for showing the picker;
- a supervised live subscription worker for
  `agentscan subscribe --format json`;
- command and stream debug output;
- local runner settings for binary path and environment variables;
- a typed profile model;
- an SSH runner spike that executes the same preflight, picker, live
  subscription, focus, and daemon status diagnostic commands through the user's
  normal SSH configuration.

The desktop implementation still treats `agentscan` as the source of truth for
daemon lifecycle, tmux access, provider classification, picker key assignment,
row display shaping, and focus behavior.

## Evidence

The slice sequence stayed aligned with the shared client contract:

- desktop scaffold: no scanner linkage;
- local picker load: consumed `hotkeys --format json`;
- focus action: delegated to `focus <pane-id>`;
- global hotkey: owned only desktop window lifecycle;
- live state: consumed `subscribe --format json`;
- settings/debug: kept env values out of routine debug output;
- SSH runner: wrapped the same command arguments in SSH instead of forwarding
  daemon sockets or parsing remote tmux output.

Verification has covered:

- TypeScript production build with `npm run build`;
- desktop Rust tests through `cargo test --manifest-path desktop/src-tauri/Cargo.toml --lib`;
- focused SSH runner command-construction, quoting, host-validation, and IPC
  deserialization tests;
- repository formatting with `cargo fmt --all --check`;
- repository linting with
  `cargo clippy --all-targets --all-features -- -D warnings`;
- repository tests with `cargo test`.

## Known Gaps

The next phase should treat these as product hardening slices:

- SSH profile UX is intentionally minimal: host, binary path, manual client tty,
  and env only.
- Remote install/bootstrap is not implemented.
- Remote client-tty targeting is now available as a manual SSH profile field;
  automatic remote tty discovery remains future work.
- Failure presentation is command-output driven and needs friendlier grouped
  states for SSH auth/network, missing binary, incompatible remote version,
  daemon auto-start refusal, tmux missing, invalid JSON, and stale focus target.
- Settings do not yet have import/export or deeper remote bootstrap guidance.
- Search/filter is implemented for picker rows; future work is larger-list
  ergonomics such as grouping, quick actions, or saved filters.
- Picker window positioning is cursor-display aware with primary-display
  fallback; future work is further summon polish such as remembered sizing,
  edge preferences, and platform-specific focus behavior.
- Packaging/release hardening for the desktop app is documented in
  `docs/desktop-release-smoke.md`; CI/release automation can still grow from
  that local workflow.
- Windows and Linux posture is documented in
  `docs/desktop-platform-posture.md`; macOS local and SSH remote clients remain
  the near-term path.

## Follow-Up Issues

The follow-up Linear issues should keep the desktop app out of scanner logic:

- AUR-414 SSH polish and diagnostics: harden remote profile validation, remote
  client-tty targeting, and failure presentation.
- AUR-415 settings UX: add profile rename/delete/reset, stronger validation,
  and safer environment editing.
- AUR-416 packaging and release: make signed/notarized desktop builds
  reproducible and add a local smoke checklist.
- AUR-417 search and filter: support larger picker sets without changing the
  `hotkeys --format json` contract.
- AUR-418 window positioning: improve summon behavior, sizing, and multi-monitor
  placement.
- AUR-419 cross-platform posture: document what macOS-first implementation
  choices must become platform adapters later.

## Guardrails

Do not add desktop-side scanner or tmux parsing code while hardening the app.
If a desktop feature appears to require provider, status, pane-output, or tmux
semantics, first look for a missing `agentscan` CLI/JSON contract and add it to
the shared command surface instead.
