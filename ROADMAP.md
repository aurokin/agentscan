# agentscan Roadmap

## Project Goal

Replace the current shell-heavy tmux agent discovery stack with a fast Rust-based scanner and indexer that can power aliases, popups, and later other tools without rescanning tmux on every interaction.

## Current Problem

The current workflow in `~/.dotfiles` does discovery at popup time:

- scan all tmux panes
- infer providers from shell heuristics
- optionally inspect process state
- optionally capture pane output
- render after the scan completes

That design is functional but slow because it redoes discovery work on demand and spends too much time in shell process churn.

## Reference Baseline

The current scripts in `~/.dotfiles` are an important reference baseline, but
they are not the specification for `agentscan`.

They are useful for understanding:

- which panes users currently expect to see
- which providers and wrappers show up in practice
- which display labels and status cues feel useful
- which popup and navigation flows exist today

They are not a requirement to preserve:

- the current shell implementation strategy
- `--fast` versus full scan behavior
- repeated `capture-pane` or broad `ps` usage
- the current popup TSV shape
- every existing regex, environment variable, or heuristic

`agentscan` should learn from the shell stack, not clone it. Once `agentscan`
publishes a Rust-native cache schema and consumer-facing commands, those become
the documented contract instead of the legacy scripts.

## Replacement Scope

`agentscan` should eventually cover:

- tmux-wide pane discovery
- provider detection for `codex`, `claude`, `gemini`, and `opencode`
- normalized pane metadata
- stable text, JSON, and popup-oriented outputs
- busy or idle state tracking
- a long-lived index or cache
- fallback process inspection for ambiguous panes
- integration points for launch wrappers to publish explicit pane metadata

## Product Boundary

This repository is the central source for the product.

That means it should own:

- the Rust scanner and cache implementation
- the user-facing `agentscan` CLI
- any bundled shell helpers needed for local integration
- any tmux-facing scripts, templates, or commands needed to consume `agentscan`
- the documentation for the supported workflow

That does not mean immediate rollout into host-specific dotfiles. Until the
product is mature, `~/.dotfiles` remains the current production workflow and is
treated as a reference environment, not the development surface for this repo.

## Non-Goals

These stay outside the core scanner unless explicitly needed later:

- replacing shell aliases in user dotfiles immediately
- replacing tmux key bindings in user dotfiles immediately
- owning provider launch semantics such as `resume` logic
- turning the scanner into a generic tmux session manager

## Implementation Planning

Concrete milestones, task sequencing, and execution details live in
`IMPLEMENTATION_PLAN.md`.

## Proposed CLI Surface

The CLI should likely split into a few stable command families:

- `agentscan scan`
- `agentscan list`
- `agentscan inspect`
- `agentscan focus`
- `agentscan daemon`
- `agentscan cache`
- `agentscan tmux`

Suggested responsibilities:

- `agentscan scan`: direct snapshot from tmux for debugging and recovery
- `agentscan list`: read the best available state source and emit text or JSON
- `agentscan inspect <pane_id>`: show one pane with classification and diagnostics
- `agentscan focus <pane_id>`: switch tmux client to a pane by stable id
- `agentscan daemon`: start, run, or supervise the long-lived indexer
- `agentscan cache`: print cache path, validate cache, or dump raw snapshot state
- `agentscan tmux`: tmux-oriented helpers such as popup output or integration setup

The important CLI distinction is between:

- direct snapshot commands for debugging
- cached/indexed commands for normal use
- integration commands for tmux or shell consumers

## Decision Log

### Implementation Language

Use Rust for the core scanner and indexer.

Reasons:

- typed data model
- easy testing around parsing and classification
- good fit for a long-lived daemon
- avoids shell process churn in the hot path

### Primary Identity

Use `pane_id` as the canonical runtime key for pane state.

Implications:

- in-memory state is keyed by `pane_id`
- cache records are keyed by `pane_id`
- inspect and focus style operations should target `pane_id`

### Platform Priority

Linux is the primary target for early fallback logic.

Implications:

- `/proc` may be used for targeted fallback inspection
- Linux behavior should be considered the reference implementation first
- macOS fallback behavior may remain reduced until explicitly designed

### Steady-State Architecture

The intended mature architecture is a long-lived daemon plus short-lived readers.

Implications:

- popup consumers should read cache, not rescan tmux
- direct snapshot scans remain useful for debugging and recovery
- daemon lifecycle behavior must be defined before migration

### Cache Policy

Use versioned JSON as the first canonical persisted snapshot format.

Implications:

- cache shape is an API contract
- schema versioning must be explicit
- TSV remains an adapter output only

### Integration Boundary

Keep shell as the integration layer and Rust as the discovery and state layer.

Implications:

- shell may launch or focus panes
- shell should not classify panes or infer activity state
- migration can happen incrementally without moving all user ergonomics into Rust
- wrapper behavior in shell is reference context for detection and metadata, not a reason to move launch or resume semantics into Rust

## Architectural Direction

### Source of Truth

Use tmux first:

- `tmux list-panes -a -F ...` for initial snapshot
- tmux control mode for live updates
- tmux user options on panes for explicit metadata

Use process inspection second:

- prefer `/proc/<pid>` on Linux for targeted fallback
- avoid broad `ps -t ...` scans in steady state

Use pane content last:

- parse incremental `%output` for tracked panes if needed
- avoid popup-time `capture-pane` scans as the default behavior

### Migration Notes

The legacy popup, wrapper, and scanner scripts reveal useful behavior that should
be considered during design review:

- title normalization matters because wrapper-heavy titles are noisy
- users benefit from stable pane targeting keyed by `pane_id`
- provider status may need provider-specific signals
- shell wrappers may eventually publish explicit pane metadata

Those observations do not imply a one-to-one compatibility goal. In particular:

- legacy shell outputs are informative, not canonical
- legacy popup UX is optional to preserve exactly
- legacy scan phases are not a design target
- legacy detection heuristics may be replaced when tmux metadata or published pane metadata gives a better answer
- known bugs and false positives in legacy shell detection are reasons to redesign the logic, not preserve it

### Design Inputs From The Legacy Workflow

Some parts of the current shell behavior are worth preserving at the level of
intent, even if the implementation changes completely:

- wrapper-heavy panes should still classify to the underlying provider when reliable signals exist
- raw pane metadata and user-facing display labels should remain separate concepts
- popup or focus consumers need stable pane targeting keyed by `pane_id`
- a temporary `unknown` status is acceptable when it avoids expensive synchronous inspection
- diagnostics and match reasons are valuable for debugging even if they are hidden from normal output

Those design inputs suggest a few concrete requirements for the Rust model:

- keep both raw tmux fields and normalized display fields
- model status explicitly instead of forcing a binary busy/idle answer
- preserve stable location and identity fields needed by short-lived consumers
- make room for explicit pane metadata published by wrappers so classification can get simpler over time
- make terminal-title and tmux-metadata analysis the default detection path for all providers
- treat pane inspection as a later fallback only when title and tmux metadata prove insufficient

### Runtime Model

The intended steady-state model is:

1. a long-lived `agentscan daemon`
2. one initial tmux snapshot
3. continuous control-mode updates
4. cached pane state exposed to short-lived consumers
5. popup and shell commands read the cache instead of rescanning tmux

## State Model And Cache Format

### Canonical State

The canonical model should live in typed Rust structs keyed by `pane_id`.

That model should include:

- stable pane identity
- normalized tmux metadata
- raw tmux metadata where normalization would discard useful context
- provider classification
- status fields such as `idle`, `busy`, or `unknown`
- match reasons or diagnostics for debugging

### Persisted Cache

Use a versioned JSON snapshot as the first persisted cache format.

Reasons:

- tiny data volume
- easy to inspect manually
- easy to test with fixtures
- easy to evolve with `schema_version`
- simple atomic write and replace behavior

The persisted cache should be treated as an API contract and should include at least:

- `schema_version`
- `generated_at`
- `source`
- `panes`

Each pane record should eventually include:

- pane identity and location
- raw tmux metadata
- normalized display metadata
- provider
- display label
- status
- classification source or reasons
- optional diagnostics

### Schema Draft

Initial snapshot shape:

```json
{
  "schema_version": 1,
  "generated_at": "2026-03-26T23:59:59Z",
  "source": {
    "kind": "snapshot",
    "tmux_version": "3.6a"
  },
  "panes": [
    {
      "pane_id": "%50",
      "location": {
        "session_name": "dotfiles",
        "window_index": 1,
        "pane_index": 1,
        "window_name": "editor"
      },
      "tmux": {
        "pane_pid": 438455,
        "pane_tty": "/dev/pts/55",
        "pane_current_path": "/home/auro/.dotfiles",
        "pane_current_command": "codex",
        "pane_title_raw": "(bront) .dotfiles: codex --dangerously-bypass-approvals-and-sandbox"
      },
      "display": {
        "label": "(bront) .dotfiles: codex",
        "activity_label": null
      },
      "provider": "codex",
      "status": {
        "kind": "unknown",
        "source": "not_checked"
      },
      "classification": {
        "matched_by": "pane_current_command",
        "confidence": "high",
        "reasons": [
          "pane_current_command=codex"
        ]
      },
      "agent_metadata": {
        "provider": null,
        "label": null,
        "cwd": null,
        "state": null,
        "session_id": null
      },
      "diagnostics": {
        "cache_origin": "direct_snapshot"
      }
    }
  ]
}
```

This draft is intentionally Rust-native. It separates raw tmux fields from
normalized display data so downstream consumers do not need to reverse-engineer
presentation logic from lossy labels.

Planned enum values:

- `provider`: `codex`, `claude`, `gemini`, `opencode`
- `status.kind`: `idle`, `busy`, `unknown`
- `status.source`: `pane_metadata`, `tmux_title`, `output_sample`, `procfs`, `not_checked`
- `source.kind`: `snapshot`, `daemon`
- `classification.confidence`: `high`, `medium`, `low`
- `classification.matched_by`: `pane_metadata`, `pane_current_command`, `pane_title`, `procfs`, `output`

Schema rules:

- unknown or unavailable fields should be explicit `null` where needed, not omitted arbitrarily
- field names should remain stable once published
- new fields should be additive when possible
- breaking schema changes must increment `schema_version`
- short-lived consumers should be able to target panes using `pane_id` without reconstructing shell-specific output

### Output Adapters

Output formats are adapters, not the canonical store.

Near-term adapters:

- `text` for human-readable CLI output
- `json` for rich machine-readable consumers
- `popup-tsv` only for narrow popup compatibility if needed

TSV should not be used as the persisted cache format because it is too lossy and brittle for long-term evolution.

### Future Storage Option

SQLite is a future option only if the project grows beyond a simple snapshot cache.

That would be justified by needs such as:

- event history
- durable incremental updates
- multiple readers or writers
- richer query or inspection workflows
- subscription or audit features

## Planned Commands

### `agentscan snapshot`
- One-shot scan from tmux metadata
- Useful for debugging and tests

### `agentscan list`
- Read live or cached state
- Emit `text`, `json`, or popup TSV

### `agentscan daemon`
- Maintain long-lived pane index
- Subscribe to tmux updates
- Persist cache for other consumers

### `agentscan inspect <pane-id>`
- Explain classification and status decisions
- Show raw metadata and fallback sources used

## Wrapper Metadata Contract

Launch wrappers may publish explicit tmux pane metadata using pane-local tmux user options.

Initial option namespace:

- `@agent.provider`
- `@agent.label`
- `@agent.cwd`
- `@agent.state`
- `@agent.session_id`

Initial semantics:

- `@agent.provider`: normalized provider name such as `codex`
- `@agent.label`: user-facing short label for popup and list output
- `@agent.cwd`: working directory intended to represent the agent task root
- `@agent.state`: optional explicit state such as `busy` or `idle`
- `@agent.session_id`: provider-specific session or resume identifier when useful

Metadata precedence should be:

1. explicit tmux user options
2. tmux pane metadata such as `pane_current_command` and title
3. targeted `/proc` fallback
4. incremental pane output parsing

Rules:

- wrappers should set pane-local metadata as early as possible after launch
- wrappers should update only fields they know
- wrappers should not invent activity state unless they have strong evidence
- missing metadata must not block discovery
- explicit metadata should override heuristic title parsing when present

Open point:

- whether wrappers should proactively clear stale `@agent.*` options on exit or whether the daemon should treat pane disappearance as authoritative

## Daemon Lifecycle Questions

These still need explicit implementation decisions before daemon work begins:

- cache path
- pid or lock path
- single-instance behavior
- reconnect behavior when tmux restarts
- behavior when cache exists but daemon is absent
- whether `list` should auto-start the daemon or remain passive

## Planned Improvements Over Current Workflow

### Discovery
- Replace repeated shell loops with a typed Rust model
- Make provider detection deterministic and testable
- Remove false positives caused by loose substring matching

### Performance
- Remove the need for separate fast and full modes
- Avoid repeated process spawning in the hot path
- Make popup opens effectively constant-time from cache

### State Tracking
- Track busy or idle state continuously instead of recapturing pane output on demand
- Support explicit provider metadata from wrappers instead of inference-only detection

### Operability
- Add unit tests around parsing and classification
- Add inspectable reasoning for why a pane matched
- Keep machine-readable output contracts stable for shell consumers
- Keep persisted cache schema versioned and explicitly documented

## Migration Plan

### Phase 1
- Snapshot scanner from `tmux list-panes`
- Provider inference from tmux metadata and titles
- Text and JSON output

### Phase 2
- Add popup TSV output
- Add versioned JSON cache snapshot
- Add pane metadata model for explicit tmux user options
- Reduce title-based heuristics where wrappers can publish metadata

### Phase 3
- Add `daemon`
- Maintain live cache from tmux control mode
- Update popup consumer to read cached state

### Phase 4
- Add targeted `/proc` fallback for ambiguous panes
- Add optional incremental output parsing for busy or idle detection
- Remove the old shell scanner from the steady-state path

## Shell Boundary

Shell should remain for:

- aliases in `~/.zshrc`
- provider launch wrappers such as `lgpt.sh`
- tmux key bindings and popup entrypoints

Shell should not remain responsible for:

- pane discovery
- provider classification
- process scanning strategy
- activity-state inference
- cache management

## Guardrails

- No permanent fast/full split in the final design
- No broad `ps` scans in the steady-state path
- No popup-time full rescans once cached mode exists
- No TSV as canonical persisted state
- No migration of dotfiles integration unless the task includes the integration layer
- No breaking output-format changes without updating shell consumers and docs
