# Cursor CLI Support Hardening

Status: completed

## Goal

Bring Cursor CLI support in line with the live `cursor-agent` binary:

- classify command-only Cursor panes correctly
- keep display labels conservative when tmux titles are generic
- preserve explicit Cursor title handling when those titles are actually present
- document the metadata-first direction for Cursor labels and session ids

## Checklist

- [x] Re-baseline Cursor fixture coverage around both command-only and title-rich panes.
- [x] Make Cursor display handling ignore non-Cursor tmux titles and fall back to generic pane labels instead.
- [x] Keep `Cursor CLI | ...` and `Cursor | ...` title normalization and status inference as secondary enhancements.
- [x] Document Cursor's metadata-first direction, including wrapper-published session ids.
- [x] Run `cargo fmt --all`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_arguments`, and `cargo test`.

## Progress Log

1. Confirmed live tmux behavior from `cursor-agent` in a disposable tmux server. The pane exposed `pane_current_command=cursor-agent` but kept a generic tmux title, so command detection is the reliable baseline.
2. Confirmed binary capabilities from `cursor-agent --help`, `about --format json`, `status --format json`, and the bundled command code. `create-chat` returns a UUID-backed local chat id that fits `@agent.session_id`.
3. Updated Cursor display logic so generic tmux titles are ignored for `cursor-agent` panes unless the title is explicitly Cursor-shaped.
4. Added command-only Cursor fixture coverage and unit tests for conservative label fallback, while keeping the existing title-rich Cursor fixture as an enhancement path.
5. Updated repo docs to describe Cursor as metadata-first for labels and session ids rather than title-first.
6. Ran the repo quality gates listed above. `cargo fmt --all --check`, both clippy passes, and `cargo test` all passed.
