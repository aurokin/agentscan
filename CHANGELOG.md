# Changelog

## Unreleased

## 0.8.1 - 2026-07-14

### Added

- The TUI picker now shows a highlight bar on the selected row: Up/Down move
  the selection across visible rows (continuing across pages at the edges)
  and Enter focuses the selected pane through the same path as the letter
  hotkeys, which keep working unchanged. The selection is pane-anchored, so
  live snapshot updates keep it on the same pane and snap it to the first
  visible row when that pane disappears. Fuzzy search (`/`) was deliberately
  split into a follow-up issue.

### Changed

- Desktop accessibility: the always-on animations (reconnect spinner, pulsing
  status dots, boot spinner, settings entrance) respect
  `prefers-reduced-motion` while keeping static state feedback; the picker
  rows use valid listbox/option roles for their selection state; and dim
  metadata text in dark mode now clears WCAG AA contrast.
- Desktop multi-client badges now name the affected host in their tooltip
  ("2 clients attached to koopa") instead of a generic "this server", on both
  the folder headers and the bar's source trigger.
- Pane-output status captures now read exactly the visible screen instead of
  the last 30 rows including scrollback, so classifiers reason over displayed
  rows only and dismissed prompts in history can no longer read as current
  state. Gemini busy markers are additionally anchored to the current bottom
  frame.
- The desktop close button now hides the dock in both framed and frameless
  mode (matching Escape and the summonable-dock model); quitting moved to the
  app menu / Cmd+Q.
- The desktop webview now ships a restrictive Content Security Policy
  (previously none).
- The daemon's subscriber health poll runs at 1s instead of 250ms, cutting
  idle wakeups; subscriber exits remain event-driven.
- Declared the minimum supported Rust version (1.88) in both crates.

### Fixed

- Targeted pane refreshes (subscription, title, and activity events) now select
  the requested pane from the window listing by id. `list-panes -t <pane>`
  returns every pane of the containing window, and the previous positional pick
  took the window's last pane, so any refresh in a multi-pane window replaced
  the target pane's record with a duplicate of its neighbor and dropped the
  real pane from the snapshot (AUR-679).
- The daemon now marks the socket closing before control-mode teardown on
  error exits too, so clients see a clean closing state instead of a dropped
  socket (the error path kills the control-mode children rather than waiting,
  since the tmux server is still alive).
- The daemon lifecycle client no longer fails when the peer tears the
  connection down before the write-side close (transient `ENOTCONN` under
  parallel load).
- Editing the desktop environment-variable list no longer drops caret focus on
  a mid-list delete (rows are keyed by stable ids instead of array index).
- `set_window_glass` no longer blocks its worker thread indefinitely if the
  main thread never runs the vibrancy closure.

## 0.8.0 - 2026-07-14

### Added

- Added snapshot diff frames to the daemon wire protocol (now v2): subscribers
  bootstrap with a full snapshot, then receive sequence-numbered per-pane diffs,
  reconnecting on any sequence gap. Slow-subscriber coalescing upgrades a
  pending diff to a full frame so subscribers can never silently diverge.
  Mismatched client/daemon protocol versions reject cleanly at handshake.
- Added versioned envelopes to machine-readable picker output: `hotkeys
  --format json` now emits `{schema_version, rows}` and `providers --format
  json` emits `{schema_version, providers}` (breaking change from the previous
  bare arrays).
- Added criterion benchmarks for snapshot material equality and full/diff
  frame encoding.

### Changed

- Subscribe frames now carry picker rows alongside each snapshot, so the
  desktop derives rows from the delivered frame instead of spawning a second
  `agentscan hotkeys` scan (and tmux sweep) per live update.
- Classification captures one process-table snapshot per scan instead of
  enumerating every system process per candidate pane, and computes one title
  analysis per pane instead of up to six.
- Reduced daemon hot-path allocation: zero-allocation material snapshot
  comparison, fewer full-snapshot clones per control event, one snapshot sort
  per control batch, and session-scoped refresh for pane focus events instead
  of full reconciles.
- Made the socket accept loop and subscriber monitors event-driven and bounded
  the control-mode event channel; steady-state idle wakeups drop from roughly
  one hundred per second to well under one per second.
- The TUI renders on the alternate screen, keeping picker frames out of
  terminal scrollback, and restores the terminal even when a panic occurs.
- Consolidated per-provider detection into ordered tables (proc-evidence arg
  patterns and title/status specs), so adding a provider's matching behavior is
  a table row plus pattern consts instead of edits across parallel if-chains.
- Derived the tmux pane format strings and row parser from one ordered field
  table, split the daemon lifecycle and doctor modules into focused
  submodules, unified root CLI flag validity into one allow-set table, and
  loaded the config file once per invocation.

### Fixed

- Split tmux metadata and output-activity subscriptions so noisy panes only
  trigger targeted reads when their current status path can require pane output;
  agent identity and metadata changes remain event-driven for every pane.
- The daemon now recovers from poisoned socket-state locks instead of becoming
  permanently unresponsive after a handler-thread panic.
- Pane-output capture errors on the direct scan path are recorded in
  diagnostics instead of being silently dropped.
- Codex footer model detection no longer treats bare English "o"-words ("on",
  "or", "of") as model names.
- Wrapper-published labels now surface as activity labels for every resolved
  provider instead of silently defaulting to none for unlisted providers.
- Session-scoped refreshes (pane focus events) now list panes with `-s`, so
  panes in the focused session's other windows are no longer dropped from the
  snapshot and published as removed on every focus action.
- A focus event for a pane that moved sessions falls back to a full reconcile
  instead of leaving the pane missing until self-heal.
- Oversized `snapshot_diff` frames fall back to a full-frame broadcast instead
  of failing the publish and leaving clients on a stale snapshot; the store's
  full-frame size bound also accounts for volatile pane fields omitted from
  diffs, so it never commits a snapshot whose full frame no longer encodes.
- Busy connection refusals are capped and both accept paths spawn threads
  fallibly, so a connect storm degrades to dropped connections instead of
  unbounded threads or an acceptor panic that kills the daemon.
- TUI terminal setup that fails partway now leaves the alternate screen
  instead of stranding the user's terminal on it.
- Targeted daemon refreshes share one lazily-captured process-table snapshot
  per pass instead of paying a full capture per unresolved candidate pane.
- The desktop live client now auto-recovers from daemon protocol/schema
  version mismatches with a slow retry instead of parking on "Live client
  failed" until a manual reconnect, and reports a host running an outdated
  agentscan as "update agentscan on the host" instead of generic invalid JSON
  (also on the slow retry, ending the once-per-second SSH subscribe churn).

## 0.7.6 - 2026-06-29

### Added

- Added Aider provider detection from exact `aider` commands, wrapper metadata,
  targeted Python process evidence for `python -m aider`, known `aider-chat`
  package paths, and Python console-script invocations.
- Added Aider desktop provider SVGs and mapped its patched Nerd Font provider
  glyph to `U+10005A` from `agent-icons-v9`.
- Added real-agent provider lifecycle E2E coverage, including the executable
  harness, provider catalog, ADR, and harness engineering guidance.
- Added install, requirements, and quickstart documentation, including the
  tmux 3.2+ runtime floor for live daemon updates.
- Added cargo-audit CI coverage for both Rust lockfiles and Dependabot update
  coverage for Rust, pnpm, and GitHub Actions dependencies.

### Changed

- Aligned desktop source headers with agent rows and quieted project group
  headers so the dock reads as one stable list.

### Fixed

- Treated broken output pipes as clean exits across one-shot CLI output paths
  instead of panicking when downstream consumers close early.
- Hardened the daemon socket accept loop so transient accept errors are retried
  and fatal acceptor failures request a clean daemon shutdown.
- Restored macOS Dock-click reopen behavior for hidden frameless desktop
  windows.
- Stabilized Antigravity provider lifecycle E2E prompt handling.

## 0.7.5 - 2026-06-23

### Changed

- Replaced the desktop multi-client banner with per-source badges so connection
  state stays attached to the source it describes.

### Fixed

- Restored Factory Droid pane-output status detection for current tmux footers
  that end in `TMUX ⧉` instead of `IDE ◌`.

## 0.7.4 - 2026-06-19

### Changed

- Reworked the desktop source menu so configured sources stay visible in the
  menu, SSH sources can be shown or hidden without deleting their settings, and
  drag reordering uses a clearer ghost row and insertion marker.
- Refined desktop empty and offline states. Resolved-empty folders now render a
  compact "No agents here" placeholder, while offline/no-daemon states avoid
  implying that a scan completed successfully.
- Migrated desktop dependency management, local release scripts, and CI/release
  workflows from npm/package-lock to pnpm/pnpm-lock.

### Fixed

- Kept disabled desktop SSH sources out of the open source set so hidden
  profiles do not arm live subscriptions or claim dock space.
- Hardened GitHub Copilot status detection for the current working footer and
  title working indicator while keeping pane-output status scoped to confirmed
  Copilot panes.
- Fixed the pnpm-based desktop app build helper and smoke command so Tauri
  receives `build` as the subcommand instead of an unexpected argument after
  `--`.

## 0.7.3 - 2026-06-11

### Added

- Added desktop smoke and unit coverage for the split dock/settings windows and
  extracted view-model/effect services, including activation, summon hotkey
  registration, hostname enrichment, preflight, picker, settings, window chrome,
  and window operations.
- Documented the desktop test harness, CI baseline, and Node 20.19+ frontend
  toolchain floor.

### Changed

- Split the desktop UI into `DockApp` and `SettingsApp`, moved repeated UI into
  focused components, and moved stateful side effects into Effect services and
  view models. The user-facing dock/settings behavior is preserved while the
  release surface is easier to test.
- Resolved local desktop hostname labeling through the Tauri host IPC boundary,
  so tests and runtime exercise the same enrichment path.

### Fixed

- Fixed a desktop atom setter edge case where function-valued inputs could be
  treated as updater functions instead of persisted values.
- Fixed duplicate in-use summon-hotkey failure publishing during retry loops.
- Fixed hostname enrichment races by marking attempts and forking probe work
  atomically.
- Restored a render-synchronized closed-source activation guard for desktop row
  selection settle time.
- Fixed stale desktop and test-layout documentation claims.

## 0.7.2 - 2026-06-10

### Added

- Added multi-source desktop dock folders. The vertical dock now renders one
  collapsible folder per enabled local or SSH source, keeps open/closed folder
  state across launches, and arms live subscriptions only for open folders.
  Each open source renders its own picker rows, status dot, and recovery strip.
- Added a keyed multi-source live pipeline for the desktop app. The Tauri worker
  now runs one supervised subscription per source key, emits source-keyed live
  events, fences stale epochs per source, and lets the Effect `LiveConnection`
  service manage independent per-source connection state.
- Added source ordering and keybind ownership for the desktop dock. Sources can
  be drag-reordered in Settings and from the footer order menu; row hotkeys are
  owned by the topmost open folder, while non-owner folders show their returned
  row keys as informational labels.
- Added local and remote hostname labels for desktop sources. The local source
  uses the machine's short hostname, and SSH preflight now captures the remote's
  short hostname so folders can show concise host labels instead of only raw SSH
  aliases.
- Added persistent probed host labels for SSH sources, including a background
  one-shot hostname probe for never-probed remotes after they come online.
- Added macOS CI coverage for the standalone Tauri desktop crate, including
  desktop Rust format/clippy/test checks plus frontend build and Vitest runs.
- Added compatible tmux auto-resolution for daemon-backed lists, direct scans,
  and focus commands. When PATH resolves a tmux client that the running server
  drops mid-handshake (`server exited unexpectedly`, `lost server`, or
  `protocol version mismatch`), agentscan probes common tmux install paths
  against the same socket, caches the first compatible binary, and retries.
  `AGENTSCAN_TMUX_BIN` pins a binary and disables auto-resolution.
- Added a release-build desktop single-instance guard. Launching a second
  packaged app instance now brings the existing window forward before normal
  initialization can compete for global resources.

### Changed

- Desktop source identity is now derived from connection settings rather than
  user-editable profile names. Persisted duplicate SSH hosts are deduplicated to
  one runnable survivor, new duplicate-host saves are refused, and only one
  unconfigured remote draft is allowed at a time.
- The desktop footer source switcher is now an order menu rather than a
  quick-switch. In the vertical dock, folder headers own open/close state; in the
  horizontal dock, the footer advertises the current keybind owner.
- Desktop picker activation is now source-scoped. Mouse and keyboard activation
  run `focus` against the row's own source settings, so pane ids that collide
  across hosts do not route to the wrong source.
- Desktop live connection gating now treats preflight as a start gate rather
  than authority over an already-online channel. A same-source preflight failure
  no longer kills or masks a live source that is already streaming rows.
- The desktop settings rail now exposes folder open/close state and source
  ordering without clobbering unsaved settings form drafts.
- Documented the multi-source desktop model, including open-folder
  subscriptions, source ordering, keybind ownership, and the horizontal bar's
  single-source presentation tradeoff.
- Documented tmux client/server version split recovery and the
  `AGENTSCAN_TMUX_BIN` explicit override boundary.
- Raised the desktop Vite chunk-size warning ceiling to 700 kB. Tauri loads the
  bundle from disk, so the default 500 kB web payload warning was noisy, while
  the higher ceiling still catches meaningful dependency growth.

### Fixed

- Fixed stale or cross-source live events by tagging live envelopes with
  `sourceKey` and epoch, filtering Tauri event queues per source, and stopping
  only the supervisor for the requested source key.
- Fixed interrupted live-subscription starts so a worker that finishes starting
  after a target switch is still stopped for its epoch instead of leaking.
- Fixed re-added desktop sources inheriting stale rows from a just-removed
  source with the same runner key.
- Fixed activation failures and recovery UI so errors are scoped to the source
  that failed, remain visible while recovery settles, expire after a readable
  interval, and do not make healthy open folders look broken.
- Fixed source labels from hostname probes so stale probe results are fenced by
  runner identity, SSH user/host identities are respected, empty hostname probes
  keep the configured label, and colliding probed labels fall back to distinct
  connection labels.
- Fixed remote source apply flows so duplicate-host refusals surface as refused
  instead of being logged as successful applies, and reused remote drafts open
  like newly added sources.
- Fixed closed or deleted source activations so their pending focus result does
  not block or overwrite activation state for still-open sources.
- Fixed frameless boot/recovery drag behavior by applying the Tauri drag region
  to the full boot-state surface, not only the outer main area.
- Fixed long-lived daemon and focus recovery across tmux client/server version
  splits. Compatible-tmux selection is reused for later commands, refreshed if
  the chosen install starts being dropped too, and `switch-client` focus now
  routes through the shared retry path.
- Fixed desktop tmux dropped-client diagnostics so they describe a client/server
  version split and name the runner's own tmux resolution instead of suggesting
  the tmux server is wedged.
- Fixed desktop summon-hotkey recovery when `Cmd+Shift+A` is already owned by
  another agentscan instance. The app now shows an in-use banner and retries
  registration until the key is released, while non-recoverable registration
  failures still surface once.

## 0.7.1 - 2026-06-09

### Added

- Added `picker_group_by` config for grouping picker rows by tmux session, git
  repo, or cwd while preserving `session:window.pane` as the tmux location.
  `session` remains the default. `git-repo` and `cwd` order rows by workspace
  group first, then tmux location, and picker hotkeys are assigned from that
  backend order.
- Added workspace context to `agentscan hotkeys --format json` rows so TUI,
  desktop, automation, and other clients can render project-oriented groups
  without parsing `location_tag`. Rows now expose `workspace.id` for grouping,
  `workspace.label` for display, and `workspace.source` for provenance.
- Added git/cwd workspace resolution for picker rows. The resolver prefers
  provider-published `agent_metadata.cwd` over tmux's current path, supports
  normal git repositories, nested repositories, linked worktrees, bare linked
  worktrees, and submodule-style `.git` files, and keeps same-named directories
  distinct through stable workspace IDs.
- When a remote desktop preflight still fails because `agentscan` can't be found
  on the host, the error now includes an actionable hint. A bounded, best-effort
  SSH probe runs the remote's interactive login shell to locate agentscan and
  reports either "your shell finds it at `<path>` — set this profile's binary to
  it" or "not installed on the host." The probe is gated to the not-found case
  (so it never runs on auth/connectivity failures) and uses
  `BatchMode`/`ConnectTimeout` to fail fast offline.
- When that probe resolves a path, the dock's "Can't reach agentscan" recovery
  screen shows a **Use this path** button that sets the profile's binary to it
  and re-probes in one click — no trip through settings required.

### Changed

- The terminal TUI now honors `picker_group_by`: it sorts through the shared
  backend picker model, renders workspace labels alongside the full tmux
  location when grouping by cwd or git repo, and keeps page anchoring and key
  targets coherent when workspace ordering changes.
- The desktop picker now groups rows by backend `workspace.id` and renders
  `workspace.label` as the section header, falling back to session-derived
  grouping for older remote `agentscan` binaries that do not emit workspace
  context. Desktop search now includes workspace label/source text.
- Text hotkey output now includes the workspace label before the tmux location
  when non-session grouping is configured, while leaving session grouping output
  unchanged.
- `agentscan doctor` now evaluates picker capacity and contract checks with the
  configured `picker_group_by`, so diagnostics match the actual picker order.
- Documented picker workspace grouping, client contracts, and the
  session/location/workspace identity split in the README, integration docs,
  desktop docs, architecture notes, and ADR.

### Fixed

- The desktop SSH runner now resolves `agentscan` installed via version managers
  (mise/asdf), `~/.cargo/bin`, or `~/.local/bin` on the remote host. A
  non-interactive `ssh host "cmd"` shell skips rc files, so those dirs were
  absent from PATH and preflight failed with `env: 'agentscan': No such file or
  directory`. The remote command now appends them to PATH (after the inherited
  `$PATH`, so the remote's own resolution still wins and a stale shim can't
  shadow a binary already on PATH), inside a POSIX `sh -c` wrapper so it stays
  correct on any login shell (incl. fish/csh) and safe when a PATH entry contains
  spaces. A remote agentscan is found without configuring an explicit binary
  path.
- The local desktop runner's auto-detect now searches the same common install
  locations, prefers a PATH-resolved executable over version-manager shims, and
  requires the candidate to be an executable file so a non-executable stub cannot
  shadow a real binary.
- Remote preflight missing-binary classification now matches the profile's
  configured binary name, so custom SSH binary names and wrappers get the same
  not-found guidance and probe behavior as the default `agentscan`.
- TUI key assignment no longer preserves a visible key slot across a
  workspace-driven reorder when doing so would make the rendered key disagree
  with the backend activation target.
- The desktop recovery action row now wraps, so showing both **Use this path**
  and **Open settings** does not clip in the horizontal dock.

## 0.7.0 - 2026-06-07

### Added

- Added `agentscan doctor`, a read-only diagnostics command for version,
  executable trust, config validity, tmux reachability, daemon health,
  discovery drift, picker capacity, and recent daemon events. It supports text
  output and a versioned JSON report, exits successfully with statuses encoded
  in the report, and never auto-starts or mutates daemon/tmux state.

### Changed

- Simplified the desktop live-picker worker to a single subscribe attempt per
  epoch, leaving reconnect ownership in the Effect `LiveConnection` service and
  preserving latch-only daemon behavior across retries.
- Reduced desktop no-daemon polling overhead by probing
  `agentscan daemon status --format json` before re-arming a full live
  subscription, which is especially important for SSH profiles.

### Fixed

- Made the desktop live worker forward-compatible with additive subscribe
  frames by ignoring unknown frame types while still rejecting malformed known
  frames.

## 0.6.1 - 2026-06-06

### Added

- Added an opt-in frameless macOS desktop window mode with custom drag,
  minimize, and close controls persisted through the Appearance service.

### Changed

- Refined the macOS desktop horizontal dock into a compact 56px bar with
  synchronized Rust/TypeScript sizing, collapsible search, tighter spacing,
  rounded glass corners, and a recovery screen fitted to the bar layout.

## 0.6.0 - 2026-06-06

### Added

- Added a first-class macOS desktop release artifact. Tagged releases now build,
  sign, notarize, staple, verify, zip, and publish the Apple Silicon Tauri app
  as `agentscan-desktop-aarch64-apple-darwin.zip` alongside the CLI tarballs.

### Changed

- Migrated desktop profile/settings, preflight, appearance, preference bridge,
  and picker connection state into Effect services with focused tests, keeping
  the desktop shell on the shared CLI command contract.

### Fixed

- Added a safe incompatible-daemon stop path for upgrades where protocol/schema
  mismatch prevents a normal RPC shutdown. `agentscan daemon stop` can now
  terminate a mismatched daemon only after validating the identity sidecar,
  socket peer PID, lifecycle lock, live PID, and executable.

## 0.5.1 - 2026-06-02

### Added

- Added configurable picker keys through the core `picker_keys` config setting.
  The default 16-key order is preserved, while `hotkeys`, `hotkey`, the TUI,
  and desktop picker surfaces now share the configured row keys.
- Added `agentscan tmux hotkey <key>` for tmux key bindings. It uses the shared
  picker hotkey path but reports invalid, unassigned, stale, or unfocusable
  selections through `tmux display-message` and exits successfully so expected
  picker misses do not open tmux command output view.
- Added best-effort daemon client focus events after successful focus actions so
  live `subscribe` consumers can refresh focused-pane UI immediately.

### Changed

- Moved the desktop live picker lifecycle into the Effect `LiveConnection`
  service. The dock now latches onto an existing daemon on launch/reconnect and
  only auto-starts the daemon from the explicit "Start agentscan" action.

### Fixed

- Fixed desktop dock recovery after daemon shutdown or restart by separating
  latch-only no-daemon states from recoverable reconnects and fatal start
  failures.

## 0.5.0 - 2026-06-01

### Changed

- Routed `nerd-font-patched` provider rendering to the custom
  `agent-icons-v8` private-use codepoints instead of falling back to the
  standard Nerd Font markers.

## 0.4.5 - 2026-05-31

### Added

- Added always-on daemon control-mode observability for cross-session event
  subscribers. `agentscan daemon status` now reports subscriber coverage,
  subscriber monitor timing, missing/dead subscriber sessions, per-subscriber
  liveness, and optional subscriber runtime counters so stale or dropped
  cross-session event coverage can be diagnosed without enabling the deep debug
  trace.
- Added source-tagged control-mode event accounting for primary and subscriber
  clients. Deep control-line tracing can now attribute each event batch to the
  control client that produced it, while routine runtime telemetry stays cheap
  and does not capture snapshot diffs for ignored-only output batches.

### Fixed

- Reattached event subscribers now recover cleanly after a subscriber control
  client exits. Dead-subscriber diagnostics persist until that exact session is
  reattached or leaves the desired subscriber set, and broker status no longer
  reports a recovered session as both live and dead.
- Control-mode broker command collection now ignores subscriber frames while
  waiting for primary command responses, and primary reconnect draining preserves
  queued subscriber events instead of dropping them with stale primary command
  frames.
- Output-only subscriber traffic now republishes runtime telemetry, keeping
  subscriber `last_line_at` status fresh even when the output batch does not
  materially change the daemon snapshot.

## 0.4.4 - 2026-05-28

### Added

- Antigravity busy detection. The closed-source antigravity CLI flips its footer between
  the idle `? for shortcuts` form and the active-turn `esc to cancel` form (alongside a
  `… Generating…`/`… Loading…` spinner above the bordered `>` input box). The pane-output
  matcher now anchors on the bottom-most footer line and discriminates by which form it
  carries, so an in-flight turn is reported busy and a stale idle footer in scrollback
  cannot shadow the live busy footer.
- Hermes idle detection now also accepts an un-submitted `❯ <draft>` prompt as idle, so
  a pane parked with a typed-but-not-sent message reads as idle rather than unknown. The
  busy prompt (`⚕ ❯ msg=interrupt …`) leads with `⚕`, so the two forms stay unambiguous.

### Fixed

- Hardened grok busy detection against footer rewording. In addition to the active-turn
  footer keybinds (`Ctrl+c:cancel` / `Ctrl+Enter:interject`), a live run spinner
  (`⠋ … [✗]`) sitting directly above the pinned input box now also marks the turn busy,
  so an in-flight turn is not misread as idle if grok relabels its interrupt hints. A
  stale spinner from a prior turn — which has completed-turn output (e.g. `Turn
  completed…`) between it and the box — is not directly above the box and so is still
  correctly read as idle.
- Hermes provider classification no longer trusts a stale `π -` OSC title left by a
  prior `pi` session in the same shell. The stale-title guard now defers to process
  evidence whenever the pane foreground is not a live pi runtime (`pi`/`node`/`bun`) and
  no spinner is repainting the title — a superset of the previous bare-shell check — so
  a different agent's runtime (e.g. hermes' python) inheriting the residual title no
  longer shadows the real provider.
- Hermes idle/busy classification is now anchored to the live input box rather than to
  any matching line in scrollback. The status bar must sit directly above the prompt
  (within ~3 rows, with only box rules or blanks between), and only box rules / blanks
  may follow the prompt. A `❯ <command>` line that appears in agent output (e.g. a quoted
  shell prompt like `❯ npm test`) or a stale prompt with later output below it is no
  longer mistaken for the live prompt.
- Opencode idle detection now works for used sessions. The live build (1.15.11) drops
  the `tab agents` command-bar hint after the first turn, folding the command bar into
  the bottom status bar (`<tokens> (<pct>) · $<cost>  ctrl+p commands  • OpenCode <ver>`).
  Anchoring idle on `tab agents` alone missed every session that had completed at least
  one turn. Idle is now also anchored on the stable `╹▀▀▀` input-box bottom border with
  a trailing-chrome guard, and the busy-marker currency check treats the input-box
  border as a valid footer anchor too — so an `esc interrupt` rendered above the box
  still wins over the new idle anchor.

## 0.4.3 - 2026-05-27

### Added

- Responsive busy/idle for providers whose status only shows in captured pane output and
  never in tmux metadata (e.g. pi without the `titlebar-spinner` extension, droid). The
  daemon subscription now includes `window_activity`, so a pane producing output fires a
  `%subscription-changed` event that drives a (cache-throttled) re-capture — making the
  idle→busy transition event-driven without taking the `%output` pty firehose. `window_activity`
  is window-scoped, so in a split window a noisy pane also refreshes its quiet siblings; those
  refreshes stay cheap because the capture itself remains gated by the pane-output capture cache
  (a still-fresh entry is reused without re-capturing), so capture cost stays proportional to
  turn activity rather than pane count. The flip side is a responsiveness floor: a short turn
  that both starts and finishes inside the cache TTL of a recent capture may not be observed as
  busy. A matching settle re-check polls any pane believed busy on a short cadence so the
  busy→idle transition (which emits no tmux event) is caught within ~2s. Installing the
  `titlebar-spinner` extension still gives pi the cheaper title-driven fast path
  automatically; both paths are supported and selected by the existing layered detection.
  Because `window_activity` ticks fire on any output in the window, the control-event refresh
  now skips publishing when the refreshed snapshot is materially unchanged (matching the settle
  and reconcile paths), so a spinner redraw or log tail in one pane no longer republishes an
  identical snapshot to subscribers. So a closed consumer is still detected promptly while the
  stream is silent, `agentscan subscribe` now emits a `{"type":"keepalive"}` heartbeat frame
  after each idle second; consumers that switch on the frame `type` ignore it.

### Fixed

- Reworked grok pane-output status detection against the current grok build (v0.2.3),
  which had regressed idle panes back to `unknown`. The matcher now reads grok's own
  state-driven keybind footer below the pinned input box rather than enumerating exact
  trailing chrome: an idle prompt shows `Shift+Tab:mode │ Ctrl+.:shortcuts` (or the
  version line `0.2.3 [stable] Beta` on a fresh prompt), while an active turn adds
  `Ctrl+c:cancel` / `Ctrl+Enter:interject`. This restores idle detection for both fresh
  and used sessions, fixes busy detection (the running-spinner line shape also changed),
  and drops the brittle version/channel-word allowlist that broke on `[stable]`.
- Generalized droid streaming detection so a turn is recognized as busy regardless of the
  streaming verb. droid's status line cycles the verb across a turn (`Streaming…`,
  `Invoking tools…`, `Thinking…`); the matcher now anchors on the live braille spinner glyph
  plus the verb-agnostic `(Press ESC to stop)` hint instead of only `Streaming…`, so the
  varying verb is handled without a bare-substring match treating prose that mentions the stop
  hint as an active turn.
- Stopped exited pi sessions from lingering as ghost panes. pi paints its `π - <cwd>`
  title via an OSC escape that tmux keeps painted after pi exits and the pane returns to
  the shell prompt, so a stale idle title kept the pane classified as pi indefinitely.
  An idle (non-spinner) greek pi title now requires liveness corroboration: over a plain
  interactive shell foreground it defers to process evidence and is dropped when no pi
  process remains. A running pi reports `node`/`pi`/`bun`, and a live spinner-frame title
  still classifies — this mirrors the corroboration grok and ascii `pi -` titles already
  require.

## 0.4.2 - 2026-05-27

### Fixed

- Stopped idle grok, pi, antigravity, and newer-build opencode panes from showing as
  `unknown`. The pane-output status path now trims trailing blank rows once before any
  provider matcher runs, so a top-rendered or freshly started agent's prompt/footer no
  longer falls outside the "near the current footer" window just because the pane is
  taller than its UI. Refreshed grok idle detection to its current bordered `❯` input
  box, added antigravity status detection (`? for shortcuts` footer over a bordered `>`
  prompt), and taught opencode the newer "OpenCode Go" command-bar input box.
- Anchored opencode busy detection to the current bottom frame: when no live idle
  prompt or command bar is visible, a stale approval/interrupt line scrolled up in the
  capture no longer forces `busy` — the busy marker must itself be in the current footer
  region or near the bottom rows.

## 0.4.1 - 2026-05-27

### Fixed

- Stopped Claude Code panes from disappearing from the list after a prompt was
  sent. Claude is identifiable only from its process (`claude` in the tree) — its
  command is its version string and its title is the current task — so the daemon
  now runs the bounded process-tree fallback on its live event path for
  unidentified, agent-shaped panes instead of only on full snapshots. The
  process-tree check stays the source of truth: a provider is carried across a
  title-only change only when it was process-confirmed and the process is
  unchanged, so title-only matches cannot keep a stale provider listed.

## 0.4.0 - 2026-05-27

### Added

- Made every tmux session event-driven through its own control-mode client: one
  primary client (commands plus its session's events) and an event-only
  subscriber client per other session, all feeding a shared event stream. Agent
  pane detection now scales across many sessions without `ps` scans or repeated
  `capture-pane`.
- Added runtime config toggles for daemon control-mode behavior.
- Added daemon observability diagnostics and expanded runtime telemetry with
  control-event firehose volume and reconnect, fallback, and subscriber counts.
- Captured tmux active-pane flags through the snapshot pipeline so the active
  pane can be distinguished downstream.

### Changed

- Made the safety reconcile coverage-aware: it relaxes to an infrequent
  self-heal/drift backstop once every session has an event client, and keeps the
  active 30-second reconcile while sessions exceed the subscriber cap.
- Paused `%output` globally on control clients (`-f ignore-size,no-output`) and
  moved metadata subscriptions to throttled `refresh-client -B`, cutting idle
  wakeups from high-volume pane output.

### Fixed

- Drained the retained shared control-mode channel on primary reconnect so a
  timed-out command's leftover frames can no longer be misattributed to a later
  brokered command and return a stale or mismatched snapshot.
- Filtered subscriber `%exit` so one dying session no longer bounces the daemon,
  and added a sub-second primary-liveness poll plus prompt dead-subscriber prune
  and re-attach.

## 0.3.3 - 2026-05-25

### Changed

- Limited daemon proc/libproc fallback to full snapshot reconcile paths; targeted
  control-mode refreshes now avoid process inspection.
- Slowed the safety reconcile interval to 30 seconds while control-mode broker
  reads are healthy, keeping the shorter interval for broker fallback and an
  env override for tests/diagnostics.
- Applied control-mode title updates as targeted pane refreshes, preserving
  existing proc-derived identity on coalesced pane/title updates while still
  removing panes that exited before the title event was processed.
- Increased control-mode batch coalescing to 100ms so bursty event groups fan
  out as fewer subscriber updates.
- Expanded daemon runtime telemetry for control-event batches, targeted refresh
  kinds, and full snapshot refreshes.
- Re-armed the safety reconcile timer when the control-mode broker recovers, so
  successful recovery returns to the active broker interval immediately.
- Capped the daemon control-mode receive wait so idle shutdown stays responsive
  even when the active safety reconcile interval is long.

## 0.3.2 - 2026-05-24

### Fixed

- Forced accepted daemon socket client streams back to blocking mode so macOS
  subscribers do not spin on inherited nonblocking `WouldBlock` reads.
- Added a regression test covering delayed client handshakes on nonblocking
  accepted streams.

## 0.3.1 - 2026-05-24

### Fixed

- Batched tmux control-mode event handling so high-volume pane output no longer
  wakes the daemon and refreshes telemetry once per ignored line.
- Preserved ordered pane, title, window, session, resnapshot, and exit behavior
  across coalesced control-mode batches.
- Added a guarded daemon socket identity check so orphaned daemons whose socket
  path disappears can self-exit instead of lingering after interrupted tests.
- Added defensive subscription read backoff for transient timeout or
  `WouldBlock` errors.

### Diagnostics

- Added `AGENTSCAN_DEEP_CONTROL_MODE_TELEMETRY=1` for investigating ignored
  control-mode event volume without making that telemetry churn the default.

## 0.3.0 - 2026-05-23

### Added

- Added a Mac-first Tauri desktop shell scaffold with a minimal local profile,
  frontend/backend IPC, and an `agentscan --version` preflight boundary that
  avoids linking scanner internals into the desktop app.
- Added local desktop picker-row loading through `agentscan hotkeys --format json`,
  including manual refresh and explicit error states for command, JSON, and
  incompatible-output failures.
- Added desktop picker keyboard selection and row activation through
  `agentscan focus <pane_id>`, keeping focus behavior delegated to the CLI.
- Added a macOS-first desktop global hotkey (`CommandOrControl+Shift+A`) that
  toggles the picker window while preserving the CLI-backed picker contract.
- Added sidebar-style desktop picker window placement on the cursor's display,
  with primary-display fallback for monitor lookup failures.
- Added a supervised desktop live-picker subscription that consumes
  `agentscan subscribe --format json`, preserves the last visible rows during
  reconnects, and surfaces daemon diagnostics for offline/fatal states.
- Added local desktop runner settings for the `agentscan` binary path and
  optional environment variables, plus a command and stream debug log.
- Added a typed desktop profile model with the local runner stored as the active
  profile, preparing the UI and command boundary for future SSH profiles.
- Added an SSH desktop runner profile spike that executes the same preflight,
  picker, live subscription, and focus command contract through the user's SSH
  configuration.
- Added desktop profile management controls for renaming, deleting, resetting,
  validating, and editing environment variables in local and SSH profiles.
- Added SSH desktop runner diagnostics and optional remote client tty targeting
  for focus actions.
- Added client-side desktop picker search and filtering with stable pane
  selection across filter and refresh changes.
- Added desktop release smoke documentation for signed/notarized macOS app
  builds, local install checks, local/SSH smoke coverage, and version metadata
  consistency.
- Added a desktop version check helper covering CLI, frontend package, Tauri
  backend, and app bundle metadata.
- Added desktop platform posture documentation for macOS-specific behavior,
  Linux/Windows deferrals, and future adapter seams.
- Added daemon operations, desktop operations, desktop client contract, provider
  evidence ledger, and daemon event/reconcile ADR docs to keep progressive
  disclosure current.
- Added an ADR choosing Tauri 2 with a Rust backend and React/TypeScript UI for
  the macOS-first desktop shell, while preserving the shared CLI client contract
  for terminal and desktop consumers.
- Added `agentscan hotkeys --format json` and `agentscan hotkey <key>` as the
  shared picker hotkey contract for tmux binds, the terminal TUI model, and
  future desktop picker surfaces.
- Added `agentscan subscribe --format json` as a live JSON Lines daemon
  subscription stream for terminal-adjacent tools and future local/SSH desktop
  clients.
- Added daemon runtime telemetry counters to `agentscan daemon status`, covering
  control-event refreshes, reconcile attempts/no-ops/material changes, targeted
  refresh full-snapshot fallbacks, and broker fallback activations.

### Changed

- Extracted the live client subscription event model so the terminal TUI and
  JSON Lines subscription stream share the same event vocabulary.
- Refreshed provider emoji and Nerd Font display markers for Codex, Claude,
  Gemini, Antigravity, Opencode, Copilot, Cursor CLI, Grok, and Droid.
- Suppressed no-op reconcile snapshot publications so live subscribers only
  receive reconcile frames when the reconciled state materially changes.
- Documented the SSH-ready desktop transport contract: desktop clients run the
  same local or remote `agentscan` command surfaces rather than scanning tmux or
  tunneling daemon sockets directly.
- Documented the desktop spike stop/go decision and follow-up hardening backlog.

### Fixed

- Prevented disabled desktop SSH profiles from being restored as the active
  runner profile before SSH execution is implemented.

## 0.2.9 - 2026-05-23

### Added

- Added `agentscan providers` to list supported coding agent providers, display
  markers, and marker codepoints, with JSON output for scripts.
- Added configurable provider icon rendering through `--icons`,
  `AGENTSCAN_ICONS`, and `${XDG_CONFIG_HOME:-~/.config}/agentscan/config.toml`.
- Added active icon mode, marker, and marker codepoints to
  `agentscan providers --format json` for configuration debugging.

## 0.2.8 - 2026-05-23

### Changed

- Refined daemon internals around explicit runtime ownership, refresh request
  handling, TUI subscription state, and detached-start coordination.
- Routed explicit daemon starts, one-shot daemon auto-starts, TUI subscription
  auto-starts, and injected start commands through a single start coordinator
  boundary while preserving existing behavior.

## 0.2.7 - 2026-05-23

### Added

- Added Antigravity CLI provider identity detection for exact `agy` command
  panes, with status kept conservative until direct state evidence is available.
- Added `agentscan daemon status --format json` for machine-readable daemon
  lifecycle and readiness checks.
- Added Droid CLI provider support across provider metadata, display labels,
  pane-output fallback, tests, and documentation.

### Changed

- Refactored daemon internals into explicit lifecycle, socket server,
  snapshot store, and control-mode broker modules.
- Moved steady-state daemon refresh reads onto the long-lived tmux control-mode
  broker, with short-lived tmux reads retained for startup and fallback paths.
- Enabled signed-only macOS daemon auto-start for daemon-backed consumers and
  TUI bootstrap. macOS detached starts now share parent-side executable trust
  preflight; ad-hoc, unsigned, or invalidly signed binaries remain blocked
  before spawning.

### Safety

- Kept `--no-auto-start` and `AGENTSCAN_NO_AUTO_START=1` as hard opt-outs before
  any platform-specific daemon start policy runs.

## 0.2.6 - 2026-05-13

### Safety

- Replaced macOS process-inspection fallback shell-outs to `ps` and `pgrep`
  with native `libproc` and `sysctl` calls, reducing executable policy churn
  during daemon refreshes while preserving Linux `/proc` behavior.

## 0.2.5 - 2026-05-08

### Release

- Added local scripts and GitHub Actions release steps for Developer ID signing
  and notarization of macOS release binaries.

## 0.2.4 - 2026-05-08

### Safety

- Removed implicit daemon auto-start on macOS. Daemon-backed commands now require
  an already-running daemon on macOS and guide users to run
  `agentscan daemon run` in a long-lived tmux pane; explicit detached
  `agentscan daemon start` remains signed-binary-only.

### Diagnostics

- Added daemon snapshot update telemetry to `agentscan daemon status`, including
  the latest update source, detail, and update duration.

## 0.2.3 - 2026-05-08

### Safety

- Removed the debug override for detached macOS daemon starts from untrusted
  binaries. Ad-hoc local builds must use foreground `agentscan daemon run`;
  detached `agentscan daemon start` remains available for signed binaries.

## 0.2.2 - 2026-05-07

### Safety

- Block detached macOS daemon starts for ad-hoc or invalidly signed
  executables, including explicit `agentscan daemon start`; use
  `agentscan daemon run` for foreground debugging or a signed release binary
  for detached daemon operation.
- Removed `spctl` from daemon-start preflight so startup checks do not invoke
  the AppleSystemPolicy/Gatekeeper assessment path.

## 0.2.1 - 2026-05-06

### Safety

- Disabled implicit daemon auto-start on macOS for ad-hoc, invalidly signed, or
  Gatekeeper-rejected executables to avoid re-triggering a macOS
  AppleSystemPolicy panic observed after the 0.2.0 auto-start rollout.
- Kept recovery paths available: `agentscan scan`, refresh-capable one-shot
  commands, and foreground `agentscan daemon run` do not depend on detached
  daemon auto-start.
- Added `AGENTSCAN_ALLOW_UNTRUSTED_DAEMON_AUTOSTART=1` as a debugging-only
  override for intentional local reproduction.
- Logged daemon start preflight context before spawning the daemon, so failed
  or blocked starts have actionable diagnostics in the daemon log.

## 0.2.0 - 2026-05-06

### Highlights

- The daemon now auto-starts on first use. Normal `agentscan`, `list`,
  `inspect`, `focus`, `snapshot`, and `tui` invocations spawn a background
  daemon if one is not already running, then read state from its socket. No
  manual `daemon start` is required for everyday use.

### Breaking Changes

| Previous surface | Current surface |
|------------------|-----------------|
| `agentscan popup` for human pane picking | `agentscan tui` |
| TUI-shaped stdout parsing | `agentscan list --format json` |
| Raw cache/envelope inspection through `agentscan cache` | `agentscan snapshot --format json` |
| Cache file IPC and `AGENTSCAN_CACHE_PATH` | Daemon socket snapshots |
| Implicit direct tmux reads in normal consumers | Daemon-backed reads with explicit `scan` or `--refresh` recovery |

- Normal `agentscan`, `list`, `inspect`, `focus`, `snapshot`, and `tui` flows
  now read daemon socket state by default.
- Normal consumers auto-start the daemon. Use `--no-auto-start` or
  `AGENTSCAN_NO_AUTO_START=1` when a script or CI job must not spawn a daemon;
  opt-out failures do not fall back to direct tmux reads.
- `agentscan scan` remains daemon-free and always reads tmux directly.
- Supported `--refresh` flags remain the direct tmux bypass for one-shot
  recovery and debugging.
- `agentscan tui` is interactive-only and subscribes to live daemon socket
  snapshots. It has no cache bootstrap, no direct tmux discovery fallback, and
  no machine-readable `--format` mode. Pane selection still uses the normal tmux
  focus behavior.
- The `agentscan cache` command family is removed, including `cache path` and
  `cache validate`.
- The persisted cache file is not a supported IPC boundary. Snapshot JSON may
  still include compatibility vocabulary such as `diagnostics.cache_origin`;
  that field does not indicate an active cache transport.
- There is no cache-file IPC replacement. Socket-isolated tests and harnesses
  should use `AGENTSCAN_SOCKET_PATH` when they need a non-default daemon socket.

### Operator Notes

- Use `agentscan list --format json` for normal automation.
- Use `agentscan list --all --format json` when automation intentionally needs
  non-agent panes.
- Use `agentscan snapshot --format json` only for raw `SnapshotEnvelope`
  consumers.
- Keep tmux key bindings and other human-facing launch paths on
  `agentscan tui`.
- Tmux `display-popup` remains a valid way to launch the binary inside tmux;
  it is separate from the removed `agentscan popup` command.

### Daemon Lifecycle

- `agentscan daemon run` runs the daemon in the foreground.
- `agentscan daemon start` detaches a background daemon through the same path.
- `agentscan daemon stop`, `daemon status`, and `daemon restart` manage the
  running daemon over the socket.

### Providers

- Added Hermes provider detection.
