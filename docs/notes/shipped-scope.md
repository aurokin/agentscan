# Current Shipped Scope

Detailed capability inventory relocated from the README. For the runtime model
and guardrails, see `docs/architecture.md`; for the automation surfaces, see
`docs/integration.md` and `docs/notes/automation-migration.md`.

## Architecture summary

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

## Scope

The current scope centers on:

- direct tmux snapshots from `tmux list-panes -a -F ...`
- a control-mode daemon that serves socket snapshots
- repo-local tmux helpers that stay thin and call the CLI
- a Mac-first Tauri desktop shell in `desktop/` that talks to the installed
  `agentscan` CLI through a narrow IPC/preflight boundary
- local and SSH desktop profiles that consume the same CLI command contract
- a runtime split by concern under `src/app/` rather than a single monolithic
  application file

## Capabilities

It can:

- run the daemon with tmux control mode and auto-start it for normal consumers,
  including signed/trusted macOS binaries
- fail fast when the daemon loses tmux, leaving restart policy to an external
  supervisor
- preserve raw tmux `session_id` and `window_id` values in the canonical pane
  model for socket consumers and local daemon updates
- refresh individual panes on daemon title and metadata updates, refresh
  affected windows or sessions when tmux emits stable ids for those scopes, and
  keep a periodic full reconcile available as a safety net
- preserve helper-published metadata across unrelated daemon writes
- bypass daemon-backed state with a fresh direct tmux snapshot for
  refresh-capable one-shot commands using `-f` / `--refresh`
- list panes through the default `list` flow
- inspect a pane by `pane_id`, including provider source, status source,
  classification reasons, and targeted `/proc` fallback decisions
- focus a pane by `pane_id`, with attached-client fallback when no explicit tty
  is provided and tested multi-client selection of the most recent attached
  client
- open an interactive `agentscan tui` UI directly from the Rust binary
- bootstrap `agentscan tui` from a live daemon socket subscription, show
  connection state, preserve the last snapshot while reconnecting, and avoid
  cache/direct-tmux discovery fallback in the TUI
- page TUI rows when more panes exist than can fit the current key budget or
  viewport
- redraw the TUI immediately on terminal resize and keep keys stable for rows
  that remain visible on the current page
- infer likely agent panes from tmux metadata
- normalize noisy provider prefixes and wrapper/script suffixes out of display
  labels for title-driven panes
- populate `display.activity_label` for meaningful title-driven panes and
  authoritative wrapper labels, including non-generic Codex wrapper titles
- keep labels conservative: show what tmux metadata actually tells us and avoid
  inventing richer task names from weak signals
- use targeted live process evidence, including pane TTY foreground process
  groups, only for unresolved ambiguous panes
- use tightly scoped provider-specific pane output parsing as a final status
  fallback for already-identified supported providers. When this path wins,
  JSON reports `status.source="pane_output"`.
- treat Cursor CLI as metadata-first: command detection is enough to identify
  the provider, but generic tmux titles fall back to conservative pane labels
  until wrappers publish stronger metadata
- infer Cursor CLI busy/idle status from the current Cursor footer only after
  provider identity is already established
- infer GitHub Copilot busy/idle status from current Copilot prompt, footer,
  thinking, and trust-prompt shapes only after provider identity is already
  established
- classify Factory Droid CLI panes from the exact `droid` command or explicit
  metadata aliases, treat `⛬ ...` titles as display labels only after provider
  identity is known, and infer busy/idle from the current Droid prompt/footer
  only after identity is established
- classify Kimi Code (Moonshot Kimi CLI) panes from the exact `kimi` command or
  explicit metadata aliases, treat the `Kimi Code` startup title as a generic
  display label, and infer busy/idle from the current Kimi input box and
  moon-phase spinner only after identity is established
- classify Grok and Hermes panes from provider-specific command/title/metadata
  evidence while keeping pane-output status fallback provider-scoped
- resolve unresolved Claude Code launcher panes from targeted process evidence,
  including Claude Code CLI paths and tmux teammate-spawn argv/env markers
- classify Aider panes from exact `aider` commands, explicit metadata aliases,
  `python -m aider`, known `aider-chat` package paths, and Python
  console-script invocations while keeping status unknown unless explicit
  metadata publishes state
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
- publish, clear, and consume explicit wrapper metadata via pane-local
  `@agent.*` tmux options
- emit canonical snapshot JSON
