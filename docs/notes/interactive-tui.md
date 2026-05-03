# Interactive TUI Note

## Purpose

This document is a narrow note for the implemented `agentscan tui` flow during
the cache-backed migration phase. The adopted architecture keeps the
user-facing interactive command as `agentscan tui` and later moves live updates
from cache polling to daemon socket subscription. Keep this file only as
context for the pre-socket TUI behavior until the daemon-socket migration
deletes or rewrites it.

## Current State

Implemented in the current branch:

- `agentscan tui` is wired as a first-class command in
  `src/app/commands.rs`.
- `agentscan tui` is an interactive-only command. Automation consumers should
  use `agentscan list --format json`. After the socket migration, consumers
  that intentionally need the raw envelope should use
  `agentscan snapshot --format json`.
- The TUI loop, raw-mode TTY handling, cache reload path, stable key
  assignment, paging, resize-aware layout, Ctrl-B passthrough, and
  missing-pane fallback live in
  `src/app/tui/`.
- Shared tmux client selection and pane-target resolution already exist in
  `src/app/tmux/`.
- TUI unit coverage already exists in
  `src/app/tests/tui.rs`.
- TUI integration coverage already exists in
  `tests/daemon_integration.rs`.
- The repo docs already describe `agentscan tui` in
  `README.md`, `ROADMAP.md`, and `docs/architecture.md`.

## Current Baseline

The current implementation remains the baseline:

- single-keystroke pane selection
- explicit close keys: `Esc` and `Ctrl-C`
- non-selection keys do not implicitly close the TUI
- live rerender from cache changes
- page overflow rows instead of rendering visible-but-unselectable entries
- viewport-aware layout with reserved footer space
- terminal resize redraw with stable keys for rows that remain visible on the
  current page
- intentional width truncation for constrained TUI panes
- undersized-TUI fallback when the viewport cannot safely show one selectable
  row plus the required footer
- Ctrl-B passthrough to the tmux prefix table
- graceful missing-pane fallback through `tmux display-message`
- cache-only open by default, with `-f` / `--refresh` for on-demand recovery
- in-TUI cache error rendering instead of a raw CLI failure

Target replacement behavior is tracked in Linear and summarized in the primary
architecture docs: no initial cache read, socket-only bootstrap, reconnect
state, and a daemon connection indicator.

## Follow-up Notes

### Repo-local tmux invocation

Do not document a single-quoted `$PWD` example such as:

- `tmux display-popup -E '$PWD/target/debug/agentscan tui'`

That leaves `$PWD` to be expanded by the shell tmux starts later, where it may
not point at the repository root.

Use one of these forms instead:

- `tmux display-popup -E "$PWD/target/debug/agentscan" tui`
- `export AGENTSCAN_BIN=/abs/path/to/agentscan`
- `tmux display-popup -E "$AGENTSCAN_BIN" tui`

### Maintenance rule

Treat this file as a narrow cache-backed TUI note. `README.md`, `ROADMAP.md`, and
`docs/architecture.md` are the sources of truth for the adopted TUI direction.
As socket phases land, delete sections here instead of keeping a parallel
cache-polling plan alive.

## Out Of Scope

- reintroducing the legacy shell interactive stack as an implementation dependency
- bringing back `agentscan popup` or `agentscan tmux popup` as compatibility layers without a real consumer
- adding TUI-shaped TSV or JSON output for scripts
- adding hidden or compatibility-only interactive `--format` paths
- broadening provider detection or status heuristics unrelated to TUI UX
