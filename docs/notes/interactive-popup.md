# Interactive Popup Note

## Purpose

This document is a narrow follow-up note for `agentscan popup`, not a live
execution plan. The popup command is already implemented in the current branch
and is part of the repo-local workflow. This note exists to preserve
popup-specific context that is too narrow for the primary docs.

## Current State

Implemented in the current branch:

- `agentscan popup` is wired as a first-class command in
  [src/app/commands.rs](/home/auro/code/agentscan/src/app/commands.rs:16).
- `agentscan popup` is an interactive-only command. Automation consumers should
  use `agentscan list --format json`, or `agentscan cache show --format json`
  when they intentionally need the raw cache envelope.
- The popup UI loop, raw-mode TTY handling, cache reload path, stable key
  assignment, paging, resize-aware layout, Ctrl-B passthrough, and
  missing-pane fallback live in
  [src/app/popup_ui.rs](/home/auro/code/agentscan/src/app/popup_ui.rs:19).
- Shared tmux client selection and pane-target resolution already exist in
  [src/app/tmux.rs](/home/auro/code/agentscan/src/app/tmux.rs:318).
- Popup unit coverage already exists in
  [src/app/tests.rs](/home/auro/code/agentscan/src/app/tests.rs:407).
- Popup integration coverage already exists in
  [tests/daemon_integration.rs](/home/auro/code/agentscan/tests/daemon_integration.rs:286).
- The repo docs already describe `agentscan popup` in
  [README.md](/home/auro/code/agentscan/README.md:1),
  [ROADMAP.md](/home/auro/code/agentscan/ROADMAP.md:1), and
  [docs/architecture.md](/home/auro/code/agentscan/docs/architecture.md:1).

## Current Baseline

The current implementation remains the baseline:

- single-keystroke pane selection
- explicit close keys: `Esc` and `Ctrl-C`
- non-selection keys do not implicitly close the popup
- live rerender from cache changes
- page overflow rows instead of rendering visible-but-unselectable entries
- viewport-aware layout with reserved footer space
- terminal resize redraw with stable keys for rows that remain visible on the
  current page
- intentional width truncation for constrained popups
- undersized-popup fallback when the viewport cannot safely show one selectable
  row plus the required footer
- Ctrl-B passthrough to the tmux prefix table
- graceful missing-pane fallback through `tmux display-message`
- cache-only open by default, with `-f` / `--refresh` for on-demand recovery
- in-popup cache error rendering instead of a raw CLI failure

## Follow-up Notes

### Repo-local tmux invocation

Do not document a single-quoted `$PWD` example such as:

- `tmux display-popup -E '$PWD/target/debug/agentscan popup'`

That leaves `$PWD` to be expanded by the shell tmux starts later, where it may
not point at the repository root.

Use one of these forms instead:

- `tmux display-popup -E "$PWD/target/debug/agentscan" popup`
- `export AGENTSCAN_BIN=/abs/path/to/agentscan`
- `tmux display-popup -E "$AGENTSCAN_BIN" popup`

### Maintenance rule

Treat this file as a narrow popup note. `README.md`, `ROADMAP.md`, and
`docs/architecture.md` remain the primary sources of truth for shipped popup
behavior. If a popup behavior is already implemented and documented there, move
it into `Current State` here or delete it instead of re-planning it.

## Out Of Scope

- reintroducing the legacy shell popup stack as an implementation dependency
- bringing back `agentscan tmux popup` as a compatibility layer without a real consumer
- adding popup-shaped TSV or JSON output for scripts
- adding hidden or compatibility-only `popup --format` paths
- broadening provider detection or status heuristics unrelated to popup UX
