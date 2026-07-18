# Automation Migration

Migration guidance relocated from the README for consumers moving off the
removed interactive automation surfaces. The supported machine-readable
commands themselves are summarized in the README's "Automation & JSON output"
section and specified in `docs/integration.md`.

## Contract summary

- `agentscan tui` is interactive-only and is not a supported machine-readable
  surface
- `agentscan popup` has been removed rather than kept as a compatibility alias
- unsupported flags on the interactive command should remain normal parse
  errors, and root-level `--format` routed to it should fail with migration
  guidance; do not add TUI-specific compatibility shims to intercept or emulate
  legacy formatting
- `agentscan list --format json` is the supported machine-readable command for
  downstream consumers in normal automation flows
- `agentscan list --all --format json` is the supported way to include
  non-agent panes in that machine-readable output
- `agentscan snapshot --format json` exposes the raw snapshot envelope when a
  consumer explicitly needs envelope details rather than the normal `list` view
- `agentscan subscribe --format json` exposes live JSON Lines daemon events for
  terminal-adjacent tools and desktop clients
- `agentscan providers --format json` exposes supported provider names,
  display markers for all icon modes, marker codepoints, and matching aliases
- `agentscan hotkeys --format json` exposes the shared picker row model
- `agentscan hotkey <key>` activates a shared picker key through the same
  focus path
- `agentscan tmux hotkey <key>` activates a shared picker key from tmux binds
  and reports misses with `display-message`
- TUI-shaped output is not a supported machine-readable contract

## Migration mapping

Machine-readable consumers should not call `agentscan tui`. The legacy
`agentscan popup` command has been removed and is not a compatibility path.

Use:

- `agentscan list --format json` for the supported JSON automation surface
- `agentscan list --all --format json` if the consumer previously depended on
  interactive `--all`
- `agentscan snapshot --format json` only when the consumer intentionally
  needs the raw snapshot envelope
- `agentscan subscribe --format json` for live JSON Lines daemon events
- `agentscan doctor --format json` for a versioned environment, daemon, and
  discovery diagnostics report
- `agentscan daemon status --format json` for daemon lifecycle and readiness
  checks
- `agentscan providers --format json` for supported provider names, display
  markers for all icon modes, marker codepoints, and aliases
- `agentscan hotkeys --format json` for shared picker rows
- `agentscan hotkey <key>` for strict picker-key activation from automation
- `agentscan tmux hotkey <key>` for tmux key bindings that should display
  expected picker misses without opening command output view
- `agentscan scan` or supported `--refresh` flags when a script intentionally
  needs direct tmux state instead of daemon state

Keep `agentscan tui` in tmux key bindings and other human-facing launch paths.
Do not call it from scripts that parse stdout, and do not depend on terminal
rendering, row ordering, key labels, or error frame text as a data contract.

The removed `agentscan cache` command family and `AGENTSCAN_CACHE_PATH` are
not compatibility paths. Use daemon socket snapshots through the documented
command surfaces instead.

If an automation consumer cannot migrate because required fields are missing
from the documented JSON surfaces, treat that as an API gap to close in `list`
or snapshot JSON. Do not add `--format` back to the interactive command,
including hidden or compatibility-only parser paths.
