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
- `docs/architecture.md`: runtime model, daemon/socket contract, command families, and guardrails
- `docs/integration.md`: wrapper metadata, daemon-backed automation surfaces, shell boundary, and migration posture
- `docs/harness-engineering.md`: progressively disclosed harness engineering approach for the repo

Active milestone sequencing lives in Linear. The repo docs are intentionally for
stable engineering guidance and operator-facing contracts, not live task
tracking.

## Quality Gates

Current local baseline:

- `cargo fmt --all --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_arguments`
- `cargo test`

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

## Adopted Target Architecture

The core architecture is moving to a daemon-required, socket-backed model:

- the daemon is the single source of live pane state
- normal consumers auto-start the daemon unless explicitly opted out
- consumers read full `SnapshotEnvelope` frames over a Unix socket
- the cache file is removed as an IPC boundary
- the interactive command is `agentscan tui`
- the `agentscan cache` command family is removed; use `agentscan snapshot`
  for raw snapshot envelopes
- `agentscan scan` and refresh-capable command flags remain direct tmux
  recovery paths that do not start or require the daemon

Active migration sequencing lives in Linear.

## Current Shipped Scope

The current branch centers on:

- direct tmux snapshots from `tmux list-panes -a -F ...`
- a control-mode daemon that serves socket snapshots
- repo-local tmux helpers that stay thin and call the CLI
- a runtime split by concern under `src/app/` rather than a single monolithic application file

It can:

- run the existing explicit daemon baseline with tmux control mode
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
  fallback for already-identified Copilot and Cursor CLI panes. When this path
  wins, JSON reports `status.source="pane_output"`.
- treat Cursor CLI as metadata-first: command detection is enough to identify the provider, but generic tmux titles fall back to conservative pane labels until wrappers publish stronger metadata
- infer Cursor CLI busy/idle status from the current Cursor footer only after
  provider identity is already established
- infer GitHub Copilot busy/idle status from current Copilot prompt, footer,
  thinking, and trust-prompt shapes only after provider identity is already
  established
- resolve unresolved Claude Code launcher panes from targeted process evidence, including Claude Code CLI paths and tmux teammate-spawn argv/env markers
- classify Pi coding agent panes from upstream-observed Greek terminal titles,
  Linux `PI_CODING_AGENT=true` process evidence, and targeted package or shim
  path evidence while keeping bare `pi` commands conservative
- classify opencode panes from upstream-observed `OpenCode` / `OC | ...`
  terminal titles, targeted package or shim path evidence, and Linux
  `OPENCODE` process evidence while keeping default opencode status unknown
  unless explicit metadata publishes state
- publish, clear, and consume explicit wrapper metadata via pane-local `@agent.*` tmux options
- emit canonical snapshot JSON

Target automation contract:

- `agentscan tui` is interactive-only and is not a supported machine-readable surface
- `agentscan popup` has been removed rather than kept as a compatibility alias
- local unsupported flags on the interactive command should remain normal parse errors,
  and root-level `--format` routed to it should fail with migration
  guidance; do not add TUI-specific compatibility shims to intercept or
  emulate legacy formatting
- `agentscan list --format json` is the supported machine-readable command for downstream consumers in normal automation flows
- `agentscan list --all --format json` is the supported way to include non-agent panes in that machine-readable output
- `agentscan snapshot --format json` exposes the raw snapshot envelope when a consumer explicitly needs envelope details rather than the normal `list` view
- TUI-shaped TSV or JSON output is not a supported long-term contract

Target operational commands:

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
- `agentscan tui`
- `agentscan tmux set-metadata`
- `agentscan tmux clear-metadata`

`agentscan` without a subcommand runs the default daemon-backed `list` flow.

For repo-local tmux `display-popup` testing without installing the binary on
`PATH`, use `tmux display-popup -E "$PWD/target/debug/agentscan" tui` after
building once.

## Migration Note

Machine-readable consumers should not call `agentscan tui`. The legacy
`agentscan popup` command has been removed and is not a compatibility path.

Use:

- `agentscan list --format json` for the supported JSON automation surface
- `agentscan list --all --format json` if the consumer previously depended on interactive `--all`
- `agentscan snapshot --format json` only when the consumer intentionally needs the raw snapshot envelope

Keep `agentscan tui` in tmux key bindings and other human-facing launch paths.
Do not call it from scripts that parse stdout, and do not depend on terminal
rendering, row ordering, key labels, or error frame text as a data contract.

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

## Target CLI Families

The target CLI centers on:

- `agentscan daemon` as the primary runtime
- `agentscan scan` for direct tmux snapshots
- `agentscan list` for normal human output and the supported JSON automation surface
- `agentscan inspect` for one-pane diagnostics
- `agentscan focus` for pane targeting
- `agentscan snapshot` for raw snapshot-envelope output
- `agentscan tui` for interactive pane selection only
- `agentscan tmux` for tmux-facing integration helpers

Shell remains the right place for aliases, launch wrappers, tmux binds, and
TUI entrypoints. `agentscan` owns pane discovery, provider classification,
metadata consumption, daemon lifecycle policy, and the documented JSON surfaces
those shell entrypoints can call.
