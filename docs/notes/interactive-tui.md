# Interactive TUI Note

## Purpose

This document is a narrow note for the implemented `agentscan tui` flow. The
adopted architecture keeps the user-facing interactive command as
`agentscan tui` and drives live updates from the daemon socket subscription.

## Current State

Implemented in the current branch:

- `agentscan tui` is wired as a first-class command in
  `src/app/commands.rs`.
- `agentscan tui` is an interactive-only command. Automation consumers should
  use `agentscan list --format json`. Consumers that intentionally need the raw
  envelope should use `agentscan snapshot --format json`.
- The TUI loop, raw-mode TTY handling, daemon subscription path, stable key
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
- live rerender from daemon socket snapshots
- visible daemon connection state in the footer
- last-snapshot display while the socket reconnects after ordinary read errors
- page overflow rows instead of rendering visible-but-unselectable entries
- viewport-aware layout with reserved footer space
- terminal resize redraw with stable keys for rows that remain visible on the
  current page
- intentional width truncation for constrained TUI panes
- undersized-TUI fallback when the viewport cannot safely show one selectable
  row plus the required footer
- Ctrl-B passthrough to the tmux prefix table
- graceful missing-pane fallback through `tmux display-message`
- socket-only bootstrap through the daemon subscription client
- no interactive `--refresh`, cache bootstrap, or direct tmux discovery fallback
- in-TUI daemon unavailable rendering instead of a raw CLI failure

## Picker Key Configuration

The TUI uses the core `picker_keys` config value from
`${XDG_CONFIG_HOME:-~/.config}/agentscan/config.toml`. When the value is absent,
the default order is:

```text
1 2 3 4 5 Q E R F G T Z X C V B
```

Configured keys remap the 16 selection slots. They must be unique single ASCII
letters or digits, normalized case-insensitively. Key combinations are out of
scope for picker selection.

Reserved behavior:

- `Esc` closes the TUI.
- `Ctrl-C` closes the TUI.
- `Ctrl-B` passes through to the tmux prefix table.
- `Left`, `Right`, `PageUp`, and `PageDown` page the TUI.
- bare `N` and `P` page the TUI, so they are rejected as configured picker
  keys.
- `Enter`, `Tab`, `Backspace`, `Delete`, arrows, function keys, whitespace, and
  control characters are not picker selection keys.

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

Treat this file as a narrow TUI implementation note. `README.md`, `ROADMAP.md`,
and `docs/architecture.md` are the sources of truth for the adopted TUI
direction. Keep this note aligned with shipped behavior instead of preserving
old cache-polling plans.

## Out Of Scope

- reintroducing the legacy shell interactive stack as an implementation dependency
- bringing back `agentscan popup` or `agentscan tmux popup` as compatibility layers without a real consumer
- adding TUI-shaped TSV or JSON output for scripts
- adding hidden or compatibility-only interactive `--format` paths
- broadening provider detection or status heuristics unrelated to TUI UX
