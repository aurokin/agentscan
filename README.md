<p align="center">
  <img src="assets/agentscan-logo.png" alt="agentscan" width="280" />
</p>

# agentscan

`agentscan` is a standalone replacement stack for tmux agent discovery.

The current shell workflow in `~/.dotfiles` does too much work at TUI launch time:

- full tmux pane scans
- shell-heavy parsing
- optional process inspection
- pane capture heuristics for activity state

This project starts from a simpler baseline:

- a Rust binary
- tmux metadata as the primary source of truth
- no `ps` scan in the steady-state path
- no fast/full mode split
- plug-and-play detection as a core product invariant
- no provider log, transcript, or session-store scanning in the default
  detection path

Common agent panes should be discoverable without asking users to install
provider hooks, extensions, launch wrappers, or shell integration. Those
integrations may eventually enrich labels, session ids, or state, but they are
deep-roadmap additions behind source analysis, local probing, and conservative
plug-and-play detection.

The shell scripts in `~/.dotfiles` are reference material, not the target design.
They are useful for understanding current user-visible behavior and edge cases, but
they do not define a requirement to preserve the same implementation strategy,
flags, heuristics, interactive flow, or output shape.

This repository is the central source for the product. If tmux helpers or shell
integration are still needed while the product matures, they should live here
rather than being developed inside host-specific dotfiles. The host workflow can
remain unchanged until `agentscan` is ready to replace it.

## Docs

- `docs/index.md`: map of the repo's progressively disclosed documentation
- `AGENTS.md`: repo-local agent guardrails and conventions
- `ROADMAP.md`: durable product direction, boundaries, and decision log
- `CHANGELOG.md`: unreleased user-facing changes and migration notes
- `docs/architecture.md`: runtime model, daemon/socket contract, command families, and guardrails
- `docs/integration.md`: wrapper metadata, daemon-backed automation surfaces,
  picker/live stream contracts, shell boundary, and migration posture
- `docs/daemon-operations.md`: daemon auto-start, status, telemetry, and troubleshooting
- `docs/desktop.md`: desktop app operation, local/SSH profiles, picker behavior, and debug log
- `docs/desktop-client-contract.md`: local/SSH desktop command contract and failure surfaces
- `docs/harness-engineering.md`: progressively disclosed harness engineering approach for the repo
- `docs/macos-release-signing.md`: local and GitHub Actions Developer ID signing/notarization workflow
- `docs/desktop-release-smoke.md`: macOS desktop build, signing, install, and smoke workflow
- `docs/desktop-platform-posture.md`: desktop platform posture and future adapter seams

Active milestone sequencing lives in Linear. The repo docs are intentionally for
stable engineering guidance and operator-facing contracts, not live task
tracking.

## Quality Gates

Current local baseline:

- `cargo fmt --all --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_arguments`
- `cargo test`

Desktop shell checks:

- `cd desktop && npm run build`
- `cargo test --manifest-path desktop/src-tauri/Cargo.toml`
- `cd desktop && npm run tauri dev`
- `scripts/check-desktop-version.sh`

Current test coverage also includes committed file-based fixtures for representative
tmux title snapshots and snapshot envelopes, plus property tests for parser and
normalization invariants, so parser and schema regressions can be checked against
both stable examples and generated inputs.

Daemon reliability is also covered by isolated integration tests that start a
temporary tmux server, run the daemon, and assert state behavior for title
changes, pane/window add-remove events, session add/remove and rename events,
window rename events, attached-session removal, wrapper-metadata helper flows,
and tmux server disappearance.

Current performance tooling:

- `cargo bench --bench core_paths -- --noplot`

The benchmark target covers snapshot row parsing, row-to-pane conversion,
snapshot deserialization, and interactive row rendering against committed
fixtures.

## Current Architecture

The core architecture is a daemon-required, socket-backed model:

- the daemon is the single source of live pane state
- normal consumers auto-start the daemon unless explicitly opted out; on macOS,
  detached auto-start runs only after parent-side executable trust preflight
  succeeds
- consumers read full `SnapshotEnvelope` frames over a Unix socket
- the cache file is removed as an IPC boundary
- the interactive command is `agentscan tui`
- the `agentscan cache` command family is removed; use `agentscan snapshot`
  for raw snapshot envelopes
- `agentscan scan` and refresh-capable command flags remain direct tmux
  recovery paths that do not start or require the daemon

Active future sequencing lives in Linear.

## Current Shipped Scope

The current branch centers on:

- direct tmux snapshots from `tmux list-panes -a -F ...`
- a control-mode daemon that serves socket snapshots
- repo-local tmux helpers that stay thin and call the CLI
- a Mac-first Tauri desktop shell in `desktop/` that talks to the installed
  `agentscan` CLI through a narrow IPC/preflight boundary
- local and SSH desktop profiles that consume the same CLI command contract
- a runtime split by concern under `src/app/` rather than a single monolithic application file

It can:

- run the daemon with tmux control mode and auto-start it for normal consumers,
  including signed/trusted macOS binaries
- fail fast when the daemon loses tmux, leaving restart policy to an external supervisor
- preserve raw tmux `session_id` and `window_id` values in the canonical pane model for socket consumers and local daemon updates
- refresh individual panes on daemon title and metadata updates, refresh affected windows or sessions when tmux emits stable ids for those scopes, and keep a periodic full reconcile as a safety net
- preserve helper-published metadata across unrelated daemon writes
- bypass daemon-backed state with a fresh direct tmux snapshot for refresh-capable one-shot commands using `-f` / `--refresh`
- list panes through the default `list` flow
- inspect a pane by `pane_id`, including provider source, status source, classification reasons, and targeted `/proc` fallback decisions
- focus a pane by `pane_id`, with attached-client fallback when no explicit tty is provided and tested multi-client selection of the most recent attached client
- open an interactive `agentscan tui` UI directly from the Rust binary
- bootstrap `agentscan tui` from a live daemon socket subscription, show
  connection state, preserve the last snapshot while reconnecting, and avoid
  cache/direct-tmux discovery fallback in the TUI
- page TUI rows when more panes exist than can fit the current key budget or viewport
- redraw the TUI immediately on terminal resize and keep keys stable for rows that remain visible on the current page
- infer likely agent panes from tmux metadata
- normalize noisy provider prefixes and wrapper/script suffixes out of display labels for title-driven panes
- populate `display.activity_label` for meaningful title-driven panes and authoritative wrapper labels, including non-generic Codex wrapper titles
- keep labels conservative: show what tmux metadata actually tells us and avoid inventing richer task names from weak signals
- use targeted live process evidence, including pane TTY foreground process
  groups, only for unresolved ambiguous panes
- use tightly scoped provider-specific pane output parsing as a final status
  fallback for already-identified supported providers. When this path wins, JSON
  reports `status.source="pane_output"`.
- treat Cursor CLI as metadata-first: command detection is enough to identify the provider, but generic tmux titles fall back to conservative pane labels until wrappers publish stronger metadata
- infer Cursor CLI busy/idle status from the current Cursor footer only after
  provider identity is already established
- infer GitHub Copilot busy/idle status from current Copilot prompt, footer,
  thinking, and trust-prompt shapes only after provider identity is already
  established
- classify Factory Droid CLI panes from the exact `droid` command or explicit
  metadata aliases, treat `⛬ ...` titles as display labels only after provider
  identity is known, and infer busy/idle from the current Droid prompt/footer
  only after identity is established
- classify Grok and Hermes panes from provider-specific command/title/metadata
  evidence while keeping pane-output status fallback provider-scoped
- resolve unresolved Claude Code launcher panes from targeted process evidence, including Claude Code CLI paths and tmux teammate-spawn argv/env markers
- classify Antigravity CLI panes from the exact native `agy` command while
  keeping status unknown until wrapper metadata or a future provider-scoped
  output fallback supplies direct state
- classify Pi coding agent panes from upstream-observed Greek terminal titles,
  Linux `PI_CODING_AGENT=true` process evidence, and targeted package or shim
  path evidence while keeping bare `pi` commands conservative
- classify opencode panes from upstream-observed `OpenCode` / `OC | ...`
  terminal titles, targeted package or shim path evidence, and Linux
  `OPENCODE` process evidence while keeping default opencode status unknown
  unless explicit metadata publishes state
- publish, clear, and consume explicit wrapper metadata via pane-local `@agent.*` tmux options
- emit canonical snapshot JSON

Automation contract:

- `agentscan tui` is interactive-only and is not a supported machine-readable surface
- `agentscan popup` has been removed rather than kept as a compatibility alias
- local unsupported flags on the interactive command should remain normal parse errors,
  and root-level `--format` routed to it should fail with migration
  guidance; do not add TUI-specific compatibility shims to intercept or
  emulate legacy formatting
- `agentscan list --format json` is the supported machine-readable command for downstream consumers in normal automation flows
- `agentscan list --all --format json` is the supported way to include non-agent panes in that machine-readable output
- `agentscan snapshot --format json` exposes the raw snapshot envelope when a consumer explicitly needs envelope details rather than the normal `list` view
- `agentscan subscribe --format json` exposes live JSON Lines daemon events for
  terminal-adjacent tools and desktop clients
- `agentscan providers --format json` exposes supported provider names,
  display markers for all icon modes, marker codepoints, and matching aliases
- `agentscan hotkeys --format json` exposes the shared picker row model
- `agentscan hotkey <key>` activates a shared picker key through the same focus path
- TUI-shaped TSV or JSON output is not a supported long-term contract

Operational commands:

- `agentscan`
- `agentscan scan`
- `agentscan list`
- `agentscan inspect <pane_id>`
- `agentscan focus <pane_id>`
- `agentscan daemon start`
- `agentscan daemon run`
- `agentscan daemon status`
- `agentscan daemon stop`
- `agentscan daemon restart`
- `agentscan snapshot`
- `agentscan subscribe`
- `agentscan providers`
- `agentscan hotkeys`
- `agentscan hotkey <key>`
- `agentscan tui`
- `agentscan tmux set-metadata`
- `agentscan tmux clear-metadata`

`agentscan` without a subcommand runs the default daemon-backed `list` flow.
For local ad-hoc macOS builds or debugging detached-start failures, run the
daemon in the foreground:

```sh
agentscan daemon run
```

For repo-local tmux `display-popup` testing without installing the binary on
`PATH`, use `tmux display-popup -E "$PWD/target/debug/agentscan" tui` after
building once.

## Configuration

`agentscan` reads optional user configuration from:

```toml
# ${XDG_CONFIG_HOME:-~/.config}/agentscan/config.toml
icons = "emoji"
disable_reconcile = false
disable_proc_fallback = false
```

Supported icon modes:

- `emoji`: default provider icons for terminals without Nerd Font coverage
- `nerd-font`: current Nerd Font provider icons
- `nerd-font-patched`: reserved for a future custom patched Nerd Font; it
  currently falls back to the `nerd-font` values

Icon mode precedence is CLI, then environment, then config file, then default:

```sh
agentscan list --icons nerd-font
AGENTSCAN_ICONS=nerd-font agentscan tui
```

Diagnostic toggles use environment values first, then config file values, then
the built-in `false` default:

```sh
AGENTSCAN_DISABLE_RECONCILE=1 agentscan daemon run
AGENTSCAN_DISABLE_PROC_FALLBACK=1 agentscan daemon run
```

`disable_reconcile` turns off the daemon's periodic/timeout reconcile safety
loop. `disable_proc_fallback` skips process-tree inspection for ambiguous panes.
The daemon reads these runtime options on startup. Both are intended for
debugging and observability, not as recommended defaults.

`agentscan providers` previews the active text icon mode, and
`agentscan providers --format json` exposes every icon mode and codepoint for
scripts or font tweaking.

## Automation Migration

Machine-readable consumers should not call `agentscan tui`. The legacy
`agentscan popup` command has been removed and is not a compatibility path.

Use:

- `agentscan list --format json` for the supported JSON automation surface
- `agentscan list --all --format json` if the consumer previously depended on interactive `--all`
- `agentscan snapshot --format json` only when the consumer intentionally needs the raw snapshot envelope
- `agentscan subscribe --format json` for live JSON Lines daemon events
- `agentscan daemon status --format json` for daemon lifecycle and readiness checks
- `agentscan providers --format json` for supported provider names, display
  markers for all icon modes, marker codepoints, and aliases
- `agentscan hotkeys --format json` for shared picker rows
- `agentscan hotkey <key>` for simple picker-key activation
- `agentscan scan` or supported `--refresh` flags when a script intentionally
  needs direct tmux state instead of daemon state

Keep `agentscan tui` in tmux key bindings and other human-facing launch paths.
Do not call it from scripts that parse stdout, and do not depend on terminal
rendering, row ordering, key labels, or error frame text as a data contract.

The removed `agentscan cache` command family and `AGENTSCAN_CACHE_PATH` are not
compatibility paths. Use daemon socket snapshots through the documented command
surfaces instead.

If an automation consumer cannot migrate because required fields are missing from
the documented JSON surfaces, treat that as an API gap to close in `list` or
snapshot JSON. Do not add `--format` back to the interactive command, including
hidden or compatibility-only parser paths.

## Reference Behavior

The existing shell stack is still relevant as an input to design work. It shows
the kinds of things users currently rely on:

- pane discovery across tmux
- stable pane targeting for navigation and focus
- provider inference and title normalization
- interactive pane selection and targeting
- rough busy/idle detection for some providers

But those scripts should be treated as a source of examples and migration context,
not as a contract for the Rust implementation. `agentscan` is free to adopt a
different internal model and different external commands as long as the new design
is faster, clearer, and intentionally documented.

The useful design inputs are mostly at the data-model level:

- wrapper-aware provider classification
- separation between raw pane metadata and cleaned display labels
- explicit `unknown` status when a fast answer is better than an expensive guess
- explicit status provenance. `status.source` can be `tmux_title`,
  `pane_metadata`, `pane_output`, or `not_checked`; `pane_output` means a
  provider-scoped current prompt/footer pattern supplied the status after
  stronger metadata/title sources were unavailable.
- stable pane identity for downstream consumers such as TUIs or focus commands

## CLI Families

The CLI centers on:

- `agentscan daemon` as the primary runtime
- `agentscan scan` for direct tmux snapshots
- `agentscan list` for normal human output and the supported JSON automation surface
- `agentscan inspect` for one-pane diagnostics
- `agentscan focus` for pane targeting
- `agentscan snapshot` for raw snapshot-envelope output
- `agentscan subscribe` for live JSON Lines daemon events
- `agentscan providers` for supported provider/icon metadata
- `agentscan hotkeys` for the shared picker row model
- `agentscan hotkey` for picker-key activation
- `agentscan tui` for interactive pane selection only
- `agentscan tmux` for tmux-facing integration helpers

Shell remains the right place for aliases, launch wrappers, tmux binds, and
TUI entrypoints. `agentscan` owns pane discovery, provider classification,
metadata consumption, daemon lifecycle policy, and the documented JSON surfaces
those shell entrypoints can call.
