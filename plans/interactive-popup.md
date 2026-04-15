# Interactive Popup Plan

## Purpose

This document is a delta plan for `agentscan popup`, not a from-scratch build
plan. The popup command is already implemented in the current branch and is part
of the repo-local workflow under review. This plan captures the remaining
popup-specific decisions and follow-up work so future implementation does not
treat already-landed work as pending.

## Current State

Implemented in the current branch:

- `agentscan popup` is wired as a first-class command in
  [src/app/commands.rs](/home/auro/code/agentscan/src/app/commands.rs:16).
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
- The repo docs in this branch already describe `agentscan popup` in
  [README.md](/home/auro/code/agentscan/README.md:81),
  [ROADMAP.md](/home/auro/code/agentscan/ROADMAP.md:409), and
  [IMPLEMENTATION_PLAN.md](/home/auro/code/agentscan/IMPLEMENTATION_PLAN.md:86).

This means the popup plan should focus on remaining UX and maintenance gaps, not
on re-planning the introduction of `crossterm`, `Commands::Popup`, or the
removal of `agentscan tmux popup`.

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

These behaviors are implemented in the current branch and should only change
through an explicit follow-up decision.

## Open Follow-ups

No popup implementation gaps are currently tracked in this branch that require a
new design pass for paging, resize handling, footer reservation, or viewport
constraints. Those behaviors are already part of the shipped popup baseline.

This file should stay limited to narrow popup-specific follow-up notes that are
not already captured in code, tests, and the primary product docs.

### 1. Repo-local tmux Invocation Example

The popup documentation should not use a single-quoted `$PWD` example such as:

- `tmux display-popup -E '$PWD/target/debug/agentscan popup'`

That leaves `$PWD` to be expanded by the shell tmux starts later, where it may
not point at the repository root.

Use one of these documented forms instead:

- `tmux display-popup -E "$PWD/target/debug/agentscan" popup`
- `export AGENTSCAN_BIN=/abs/path/to/agentscan`
- `tmux display-popup -E "$AGENTSCAN_BIN" popup`

The first form is already the safer documented repo-local workflow in
[README.md](/home/auro/code/agentscan/README.md:113).

### 2. Documentation Hygiene

This plan must stay aligned with the shipped command surface. Future updates
should not reintroduce already-completed migration steps such as:

- adding `src/app/popup_ui.rs`
- adding `Commands::Popup`
- adding `crossterm`
- removing `agentscan tmux popup`
- updating README/ROADMAP to mention `agentscan popup`

If a popup change is already reflected in code and primary docs, update this
plan by moving that item into `Current State` or deleting it.

## Testing Follow-up

The popup already has unit and integration coverage in this branch for:

- pane selection row rendering
- stable key assignments within the visible page
- popup-session ordering that appends new panes without reshuffling existing rows
- paging overflow instead of rendering unselectable rows
- page clamping after cache-driven pane removal
- resize-aware layout that keeps visible-row keys stable
- undersized popup fallback rendering
- missing-pane fallback
- Ctrl-B passthrough
- cache-error rendering

No new tests are needed merely to re-cover popup behaviors that are already
covered today unless the implementation changes.

## Maintenance Rule

Treat this file as a narrow popup follow-up note. `README.md`, `ROADMAP.md`,
and `IMPLEMENTATION_PLAN.md` remain the primary sources of truth for shipped
behavior. If a popup behavior is already implemented and documented there, move
it into `Current State` here or delete it from this file instead of re-planning
it as future work.

## Out of Scope

- reintroducing the legacy shell popup stack as an implementation dependency
- bringing back `agentscan tmux popup` as a compatibility layer without a real
  consumer
- broadening provider detection or status heuristics unrelated to popup UX
