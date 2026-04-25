# agentscan Roadmap

## Project Goal

Replace the current shell-heavy tmux agent discovery stack with a fast Rust
scanner and indexer that can power aliases, popups, and later other tools
without rescanning tmux on every interaction.

## Product Boundary

This repository is the product source of truth.

It owns:

- the Rust scanner and cache implementation
- the user-facing `agentscan` CLI
- tmux-facing commands and minimal integration guidance
- the documentation for supported contracts and workflows

It does not require immediate migration of host-specific dotfiles. Those remain
reference environment material until the Rust-native workflow is mature enough
to replace them intentionally.

## Non-Goals

These stay outside the core scanner unless explicitly justified later:

- replacing shell aliases in user dotfiles immediately
- replacing tmux key bindings in user dotfiles immediately
- owning provider launch semantics such as `resume`
- turning the scanner into a generic tmux session manager
- adding a permanent fast versus full scan split

## Documentation Posture

This file records durable direction and architectural decisions.

Active milestone sequencing, blockers, and execution detail live in Linear until
they settle. Stable implementation detail should be promoted back into:

- `docs/index.md`
- `docs/architecture.md`
- `docs/integration.md`
- `docs/harness-engineering.md`

## Durable Decisions

### Implementation Language

Use Rust for the core scanner and indexer.

Reasons:

- typed data model
- straightforward testing around parsing and classification
- good fit for a long-lived daemon
- reduced shell process churn in the hot path

### Primary Identity

Use `pane_id` as the canonical runtime key for pane state.

Implications:

- in-memory state is keyed by `pane_id`
- cache records are keyed by `pane_id`
- inspect and focus workflows target `pane_id`

### Steady-State Architecture

The intended mature architecture is a long-lived daemon plus short-lived
readers.

Implications:

- popup consumers read cache instead of rescanning tmux
- direct snapshots remain available for debugging and recovery
- daemon startup is explicit
- when tmux disappears, fail fast and let an external supervisor own restart policy

### Cache Policy

Use versioned JSON as the canonical persisted snapshot format.

Implications:

- the cache shape is an API contract
- schema versioning must be explicit
- TSV is an output adapter only, not the canonical store

### Detection Policy

The default detection path is:

1. explicit wrapper-published tmux metadata
2. tmux pane metadata and terminal titles
3. targeted `/proc` fallback for concrete ambiguous panes
4. incremental pane output parsing only if later justified

Implications:

- prefer tmux metadata and control-mode events over process scans
- keep labels conservative when evidence is weak
- treat pane inspection as fallback rather than the normal path
- `/proc` fallback is currently limited to unresolved Linux `node` and
  `python3` launcher panes, where a descendant process command matches a known
  provider binary
- `inspect` reports provider source, status source, classification reasons, and
  targeted `/proc` fallback outcomes so classification problems can be debugged
  from the CLI and JSON cache without reading implementation code

### Popup Contract

`agentscan popup` is an interactive UI, not an automation surface.

Implications:

- popup does not support `--format`
- popup rendering is not a stable machine-readable contract
- automation consumers should use `agentscan list --format json` for normal pane data
- raw cache consumers should use `agentscan cache show --format json`
- compatibility formatting paths must not be added back to popup

### Integration Boundary

Keep shell as the integration layer and Rust as the discovery and state layer.

Implications:

- shell may launch panes and bind keys
- shell may keep aliases, provider wrappers, and popup entrypoints
- shell should not classify panes or infer activity state
- shell should not shape machine-readable pane output
- wrapper behavior is integration context, not a reason to move launch logic into Rust

### Platform Priority

Linux is the primary target for early fallback logic.

Implications:

- targeted `/proc` fallback may be Linux-first
- macOS fallback behavior may remain reduced until explicitly designed

## Reference Baseline

The current shell stack in `~/.dotfiles` is useful as reference material, not as
the specification for `agentscan`.

It is helpful for understanding:

- which panes users currently expect to see
- which providers and wrappers show up in practice
- which display labels and status cues feel useful
- which popup and navigation flows exist today

It is not a requirement to preserve:

- the existing shell implementation strategy
- repeated `capture-pane` or broad `ps` usage
- popup-shaped TSV output
- every legacy heuristic or regex

`agentscan` should learn from that workflow, not clone it.

## Migration Posture

Delivered baseline:

- snapshot scanner from `tmux list-panes`
- provider inference from tmux metadata and titles
- text and JSON output
- interactive `agentscan popup`
- versioned JSON cache snapshot
- pane metadata model for explicit tmux user options
- daemon-backed cache maintenance from tmux control mode
- targeted `/proc` fallback for unresolved `node` and `python3` launcher panes
- inspect provenance for provider, status, classification, and fallback
  decisions

Further migration sequencing belongs in Linear until it becomes stable enough to
document as a contract in the repo docs.

## Future Direction

Likely next classes of durable improvement:

- continued hardening of tmux client interaction flows
- incremental output parsing only if later justified by concrete unresolved panes

Those should move from Linear into the repo docs only when they become settled
behavior or durable engineering policy.
