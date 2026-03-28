# agentscan Implementation Plan

## Purpose

This document turns the high-level product direction from `ROADMAP.md` into a
concrete implementation plan for the first buildout of `agentscan`.

It assumes:

- this repo is the central source for the product
- the production host workflow in `~/.dotfiles` stays unchanged until rollout
- the daemon-backed index is part of v1, not a later optimization
- the first consumer contract is a versioned JSON file cache
- tmux popup consumption remains a first-class v1 workflow

## Locked Decisions

These decisions are already made and should be treated as planning inputs:

- architecture is daemon-first
- the first canonical consumer contract is a JSON file cache
- tmux popup consumption remains in v1
- `busy` means a reliable positive signal
- `idle` means a reliable negative signal
- `unknown` means ambiguous or not cheaply knowable
- published wrapper `provider` and `label` are authoritative when present
- published wrapper `status` is advisory
- tmux and shell integration should ship from this repo as thin wrappers around the CLI
- legacy shell detection bugs are reasons to redesign the detector rather than preserve old heuristics
- terminal titles and tmux metadata are the default detection path for all providers
- pane-content inspection is a later fallback only if title and tmux metadata are proven insufficient

## Product Surface

The initial product surface should center on these commands:

- `agentscan daemon`
- `agentscan scan`
- `agentscan list`
- `agentscan inspect <pane_id>`
- `agentscan focus <pane_id>`
- `agentscan cache`
- `agentscan tmux`

Expected command roles:

- `agentscan daemon`: run and supervise the long-lived indexer
- `agentscan scan`: direct tmux snapshot for debugging and recovery
- `agentscan list`: normal user-facing listing from cache, with JSON and text output
- `agentscan inspect`: show one pane with classification and diagnostics
- `agentscan focus`: switch tmux client to a pane by `pane_id`
- `agentscan cache`: expose cache location, health, and raw state inspection
- `agentscan tmux`: tmux-facing helper commands used by bundled scripts

## Current Progress

Completed baseline work:

- canonical pane model and snapshot envelope are implemented
- `agentscan scan`, `agentscan list`, and `agentscan inspect` are implemented
- `agentscan cache path` is implemented with XDG default plus override support
- title-first metadata classification is wired into snapshot ingestion

Still pending in Phase 1:

- daemon runtime and control-mode ingestion
- cache writes and reads
- `agentscan focus` validation in normal tmux workflows
- dedicated popup-oriented tmux subcommand and thin integration wrapper

## Phase 1

Phase 1 delivers the minimum daemon-backed product that can replace popup-time
rescans without requiring host dotfile migration.

### Outcomes

- control-mode daemon maintains pane state in memory
- daemon writes a canonical JSON cache atomically
- `list`, `inspect`, and `focus` operate against the cache or daemon-managed state
- tmux popup workflow can consume this repo's integration entrypoint
- `scan` remains available as a direct snapshot debug path

### Workstreams

#### 1. Canonical Data Model

Implement typed Rust structs for:

- pane identity and location
- raw tmux metadata
- normalized display metadata
- provider classification
- structured status with `kind` and `source`
- diagnostics and classification reasons
- persisted snapshot envelope with `schema_version`, `generated_at`, and `source`

Acceptance criteria:

- snapshot JSON shape matches the schema direction in `ROADMAP.md`
- the model can represent raw and normalized fields separately
- `unknown` status is represented explicitly without ad hoc string handling

#### 2. Snapshot Ingestion

Implement a reusable snapshot path that:

- runs `tmux list-panes -a -F ...`
- parses pane records into the canonical model
- applies initial provider classification from tmux metadata, with title-aware logic as the primary signal path
- supports a direct `agentscan scan` command for debugging

Acceptance criteria:

- parse failures are surfaced clearly
- snapshot ingestion can be tested from fixtures
- direct scan output can serialize as text and JSON
- detection does not depend on pane-content inspection for common provider cases when title data is sufficient

#### 3. Daemon Runtime

Implement the first daemon runtime around tmux control mode.

Responsibilities:

- initial full snapshot on startup
- subscribe to control-mode events
- update in-memory pane state
- write the JSON cache after state changes
- recover cleanly if tmux is briefly unavailable

Detection posture:

- use tmux metadata and control-mode state as the primary detection path
- make terminal-title analysis the default classification and status path for all providers
- treat pane-content inspection as a later fallback for concrete unresolved cases only

Acceptance criteria:

- daemon can start with a fresh tmux snapshot
- daemon updates cache after pane topology changes
- daemon exits with clear errors or retries in defined cases

#### 4. Cache Management

Implement the on-disk cache contract.

Responsibilities:

- choose cache path strategy
- atomically write versioned JSON snapshots
- validate schema version during reads
- expose cache inspection through `agentscan cache`

Acceptance criteria:

- cache writes are atomic
- consumers can detect missing or stale cache cleanly
- cache content is human-inspectable and fixture-friendly

#### 5. User Commands

Implement the first consumer-facing commands.

`agentscan list`

- default human-readable output
- JSON output option
- reads cached state by default

`agentscan inspect <pane_id>`

- shows raw fields, normalized display fields, status, and diagnostics

`agentscan focus <pane_id>`

- focuses pane by stable identity through tmux

Acceptance criteria:

- commands behave sensibly when cache is missing
- inspect output is useful for debugging classification issues
- focus succeeds or fails with actionable messages

#### 6. tmux Integration From This Repo

Ship repo-local tmux integration that stays thin.

Responsibilities:

- popup-oriented helper script or command entrypoint
- map popup selection to `agentscan focus`
- render data from `agentscan list` or `agentscan tmux`

Acceptance criteria:

- popup flow works without running shell-heavy discovery logic
- tmux integration code stays thin and delegates to the CLI
- no host dotfile edits are required for repo-local testing

## Phase 2

Phase 2 reduces heuristic dependence by letting wrappers publish pane metadata.

### Outcomes

- tmux user option contract is defined
- wrappers can publish authoritative provider and label metadata
- published metadata participates in classification and display generation

### Workstreams

- define pane option names and value format
- implement reader support in scanner and daemon paths
- implement repo-local helper for wrappers to publish metadata
- document precedence rules between published and inferred values

## Phase 3

Phase 3 hardens the product and expands ergonomics once the main architecture is stable.

### Outcomes

- daemon lifecycle is more resilient
- inspect and cache workflows are richer
- remaining tmux and shell integration that belongs in this repo is added

### Workstreams

- daemon restart and health semantics
- stale cache handling and diagnostics
- richer `agentscan cache` subcommands
- improved provider-specific status signals where justified
- broader fixture coverage and regression tests

## Open Implementation Questions

These are now implementation questions rather than product-shape questions.

### Cache Path

Need to choose:

- XDG cache directory
- repo-local development path
- override environment variable for tests and local experimentation

Chosen direction:

- XDG cache path by default with an override env var for tests

### Daemon Lifecycle UX

Need to choose:

- whether `agentscan list` auto-starts the daemon when cache is missing
- whether `agentscan daemon run` is the explicit entrypoint and all other commands stay read-only

Chosen direction:

- explicit daemon startup first, auto-start only if it becomes clearly necessary

### tmux Control-Mode Ownership

Need to choose:

- single tmux server assumption
- or explicit target selection for multiple tmux servers

Chosen direction:

- support the default current tmux environment first and defer multi-server handling

### Popup Output Shape

Need to choose:

- whether popup rendering reads text output from `agentscan list`
- or a dedicated structured `agentscan tmux popup` output

Chosen direction:

- dedicated popup-oriented subcommand so popup rendering does not depend on human text formatting

### Detection Strategy Validation

Need to choose:

- which provider cases can be covered from title and tmux metadata alone before any deeper inspection is justified
- how to capture failing examples from the legacy workflow as fixtures without inheriting legacy behavior wholesale

Chosen direction:

- start with fixture-driven analysis of real title samples across all supported providers
- add pane-content or procfs fallbacks only for concrete unresolved cases

## Phase 1 Task Breakdown

Suggested implementation order:

1. Refactor the current flat `PaneRecord` into the canonical Rust model.
2. Add `agentscan scan` and `agentscan list` around that model.
3. Implement JSON cache write and read paths.
4. Add the first `agentscan daemon run` path with initial snapshot plus cache writes.
5. Add tmux control-mode event handling and incremental state updates.
6. Implement `agentscan inspect <pane_id>`.
7. Implement `agentscan focus <pane_id>`.
8. Add repo-local tmux popup integration that consumes the CLI.
9. Add fixture-driven tests for parsing, classification, cache serialization, and focus behavior.

## Definition Of Done For V1

V1 is ready for controlled migration when:

- popup-time full rescans are no longer required for normal use
- daemon-backed cache updates are stable in day-to-day tmux usage
- the popup flow works using repo-local integration from this repo
- `list`, `inspect`, and `focus` cover the common daily workflows
- the schema and command surface are documented and stable enough for early adopters
