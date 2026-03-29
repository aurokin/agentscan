# agentscan

`agentscan` is a standalone replacement stack for tmux agent discovery.

The current shell workflow in `~/.dotfiles` does too much work at popup time:

- full tmux pane scans
- shell-heavy parsing
- optional process inspection
- pane capture heuristics for activity state

This project starts from a simpler baseline:

- a Rust binary
- tmux metadata as the primary source of truth
- no `ps` scan in the steady-state path
- no fast/full mode split

The shell scripts in `~/.dotfiles` are reference material, not the target design.
They are useful for understanding current user-visible behavior and edge cases, but
they do not define a requirement to preserve the same implementation strategy,
flags, heuristics, popup flow, or output shape.

This repository is the central source for the product. If tmux helpers or shell
integration are still needed while the product matures, they should live here
rather than being developed inside host-specific dotfiles. The host workflow can
remain unchanged until `agentscan` is ready to replace it.

## Docs

- `AGENTS.md`: repo-local agent guardrails and conventions
- `ROADMAP.md`: intended behavior, architecture, and migration plan
- `IMPLEMENTATION_PLAN.md`: concrete milestones, task breakdown, and execution plan

## Quality Gates

Current local baseline:

- `cargo fmt --all --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_arguments`
- `cargo test`

Current test coverage also includes committed file-based fixtures for representative
tmux title snapshots and cache snapshots, plus property tests for parser and
normalization invariants, so parser and schema regressions can be checked against
both stable examples and generated inputs.

Daemon reliability is also covered by isolated integration tests that start a
temporary tmux server, run `agentscan daemon run`, and assert cache behavior for
title changes, pane/window add-remove events, session add/remove and rename
events, window rename events, attached-session removal, wrapper-metadata helper
flows, and tmux server disappearance.

Current performance tooling:

- `cargo bench --bench core_paths -- --noplot`

The initial benchmark target covers snapshot row parsing, row-to-pane conversion,
cache deserialization, and popup entry generation against committed fixtures.

## Current scope

The first pass implements a snapshot scanner backed by:

- `tmux list-panes -a -F ...`

It can:

- run an explicit daemon baseline with tmux control mode
- fail fast when the daemon loses tmux, leaving restart policy to an external supervisor
- refresh individual panes on daemon title and metadata updates instead of full tmux rescans
- report daemon-backed cache health
- persist and read a local JSON cache
- show and validate the local JSON cache
- force a fresh tmux snapshot and cache rewrite for cache-backed pane and cache-inspection commands with `-f` / `--refresh`
- list panes through the default `list` flow
- inspect a pane by `pane_id`
- focus a pane by `pane_id`, with attached-client fallback when no explicit tty is provided and tested multi-client selection of the most recent attached client
- emit dedicated popup-oriented tmux output
- infer likely agent panes from tmux metadata
- normalize noisy provider prefixes and wrapper/script suffixes out of display labels for title-driven panes
- populate `display.activity_label` for title-driven panes when titles include meaningful activity text
- publish, clear, and consume explicit wrapper metadata via pane-local `@agent.*` tmux options
- refresh the existing cache immediately after repo-local metadata helper writes so wrapper-driven metadata changes stay visible to cache consumers whether the cache came from the daemon or a forced snapshot
- forward `-f` / `--refresh` through the bundled popup wrapper for on-demand cache refresh
- emit canonical snapshot JSON
- print the cache path

It does not yet:

- broaden title-driven status logic beyond the current Codex and Claude focus, with Gemini and OpenCode still treated as secondary coverage

## Reference Behavior

The existing shell stack is still relevant as an input to design work. It shows
the kinds of things users currently rely on:

- pane discovery across tmux
- stable pane targeting for navigation and focus
- provider inference and title normalization
- popup-oriented output and pane targeting
- rough busy/idle detection for some providers

But those scripts should be treated as a source of examples and migration context,
not as a contract for the Rust implementation. `agentscan` is free to adopt a
different internal model and different external commands as long as the new design
is faster, clearer, and intentionally documented.

The useful design inputs are mostly at the data-model level:

- wrapper-aware provider classification
- separation between raw pane metadata and cleaned display labels
- explicit `unknown` status when a fast answer is better than an expensive guess
- stable pane identity for downstream consumers such as popups or focus commands

## Near-term plan

1. Add a long-lived control-mode indexer as the core runtime.
2. Persist a local JSON cache for popup consumers.
3. Keep tmux popup consumption working from this repo while the product matures.
4. Let wrappers publish explicit pane metadata into tmux user options.
5. Fall back to procfs only for ambiguous panes.

## Likely CLI Shape

The mature CLI will probably center on:

- `agentscan daemon` as the primary runtime
- `agentscan scan` for direct tmux snapshots
- `agentscan list` for normal human or JSON output
- `agentscan inspect` for one-pane diagnostics
- `agentscan focus` for pane targeting
- `agentscan cache` for indexed operation
- `agentscan tmux` for tmux-facing integration helpers
