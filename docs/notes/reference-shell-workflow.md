# Reference Shell Workflow (`~/.dotfiles`)

Historical design context relocated from the README. `agentscan` began as a
standalone replacement stack for a shell-based tmux agent-discovery workflow
that lived in the author's `~/.dotfiles`.

## Why replace the shell stack

The prior shell workflow did too much work at TUI launch time:

- full tmux pane scans
- shell-heavy parsing
- optional process inspection
- pane capture heuristics for activity state

This project started from a simpler baseline:

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

## Status of the shell scripts

The shell scripts in `~/.dotfiles` are reference material, not the target
design. They are useful for understanding prior user-visible behavior and edge
cases, but they do not define a requirement to preserve the same implementation
strategy, flags, heuristics, interactive flow, or output shape.

This repository is the central source for the product. If tmux helpers or shell
integration are still needed while the product matures, they should live here
rather than being developed inside host-specific dotfiles.

## Reference behavior as design input

The shell stack shows the kinds of things users relied on:

- pane discovery across tmux
- stable pane targeting for navigation and focus
- provider inference and title normalization
- interactive pane selection and targeting
- rough busy/idle detection for some providers

Those scripts are a source of examples and migration context, not a contract
for the Rust implementation. `agentscan` is free to adopt a different internal
model and different external commands as long as the new design is faster,
clearer, and intentionally documented.

The useful design inputs are mostly at the data-model level:

- wrapper-aware provider classification
- separation between raw pane metadata and cleaned display labels
- explicit `unknown` status when a fast answer is better than an expensive guess
- explicit status provenance. `status.source` can be `tmux_title`,
  `pane_metadata`, `pane_output`, or `not_checked`; `pane_output` means a
  provider-scoped current prompt/footer pattern supplied the status after
  stronger metadata/title sources were unavailable.
- stable pane identity for downstream consumers such as TUIs or focus commands

## Shell boundary

Shell remains the right place for aliases, launch wrappers, tmux binds, and
TUI entrypoints. `agentscan` owns pane discovery, provider classification,
metadata consumption, daemon lifecycle policy, and the documented JSON surfaces
those shell entrypoints can call.
