# agentscan Implementation Plan

## Purpose

This document turns the high-level product direction from `ROADMAP.md` into a
concrete implementation plan for the first buildout of `agentscan`.

It assumes:

- this repo is the central source for the product
- the production host workflow in `~/.dotfiles` stays unchanged until rollout
- the daemon-backed index is part of v1, not a later optimization
- the first consumer contract is a versioned JSON file cache
- the interactive popup remains a first-class v1 workflow
- popup automation consumers migrate to documented JSON commands rather than popup-shaped output

## Locked Decisions

These decisions are already made and should be treated as planning inputs:

- architecture is daemon-first
- the first canonical consumer contract is a JSON file cache
- the interactive popup remains in v1
- `agentscan popup` is interactive-only; machine-readable consumers use documented JSON commands
- `busy` means a reliable positive signal
- `idle` means a reliable negative signal
- `unknown` means ambiguous or not cheaply knowable
- published wrapper `provider` and `label` are authoritative when present
- published wrapper `status` is advisory
- tmux integration should ship from this repo as thin configuration around the CLI
- legacy shell detection bugs are reasons to redesign the detector rather than preserve old heuristics
- terminal titles and tmux metadata are the default detection path for all providers
- pane-content inspection is a later fallback only if title and tmux metadata are proven insufficient
- display labels should stay conservative and reflect only high-signal metadata we actually have, rather than inventing richer labels from weak inference

## Product Surface

The initial product surface should center on these commands:

- `agentscan daemon`
- `agentscan scan`
- `agentscan list`
- `agentscan popup`
- `agentscan inspect <pane_id>`
- `agentscan focus <pane_id>`
- `agentscan cache`
- `agentscan tmux`

Expected command roles:

- `agentscan daemon`: run and supervise the long-lived indexer
- `agentscan scan`: direct tmux snapshot for debugging and recovery
- `agentscan list`: normal user-facing listing from cache, with JSON as the supported machine-readable automation surface
- `agentscan popup`: interactive pane picker from cached state
- `agentscan inspect`: show one pane with classification and diagnostics
- `agentscan focus`: switch tmux client to a pane by `pane_id`
- `agentscan cache`: expose cache location, health, and raw state inspection
- `agentscan popup`: interactive popup UI from cached pane state
- `agentscan tmux`: tmux-facing helper commands for metadata publishing and repo-local integration
- `agentscan cache show --format json`: lower-level raw cache contract for consumers that intentionally need the full snapshot envelope

## Current Progress

Completed baseline work:

- formatting, lint, complexity, and unit-test checks now run clean locally
- CI runs format, lint, complexity, and unit-test checks
- fixture-backed tests now cover representative tmux title snapshots and cache snapshot deserialization
- fixture-backed tests now cover legacy and current tmux row shapes, including raw tmux `session_id` and `window_id` fields in the current format
- fixture-backed title coverage now includes current Codex waiting-state titles, Claude textual `Claude Code | ...` states, and additional live Claude task-title samples
- property tests now cover parser round-trips and normalization invariants
- benchmark tooling now measures snapshot parsing, row-to-pane conversion, cache deserialization, and popup row rendering
- isolated daemon integration tests now cover title updates, pane/window add-remove topology changes, session add/remove and rename churn, window rename churn, attached-session removal, wrapper-metadata helper flows, and fail-fast tmux server disappearance
- the runtime is now split by concern under `src/app/` so command dispatch, cache logic, daemon handling, tmux integration, output formatting, and classification logic can evolve independently without continuing to grow a single monolithic file
- canonical pane model and snapshot envelope are implemented
- `agentscan scan`, `agentscan list`, and `agentscan inspect` are implemented
- `agentscan cache path`, `cache show`, and `cache validate` are implemented with XDG default plus override support
- `agentscan daemon status` reports daemon-backed cache health and now distinguishes daemon-backed, stale, and snapshot-only cache states with explicit provenance messaging
- title-first metadata classification is wired into snapshot ingestion
- `agentscan daemon run` writes a daemon-marked cache from tmux control mode
- the daemon currently fails fast when tmux disappears and leaves restart policy to an external supervisor
- daemon title and metadata updates now refresh only the affected pane, window and session rename/topology events can refresh the affected tmux scope when stable ids are present, and a periodic full reconcile remains as a safety net
- `list` and `inspect` now read cache-backed state by default
- `scan`, cache-backed pane commands, cache inspection, and popup commands now support `-f` / `--refresh` to take a fresh tmux snapshot and rewrite cache on demand without losing the last known daemon refresh timestamp
- cache reads now validate schema version before consumers use cached state
- pane diagnostics now distinguish direct snapshots, daemon snapshots, and daemon-updated panes
- raw tmux metadata now includes stable `session_id` and `window_id` values for debugging and narrower daemon refresh paths
- full snapshots and targeted daemon pane refreshes now keep pane ordering stable
- `agentscan popup` provides an interactive popup UI directly from the Rust binary
- popup rows preserve stable key assignments within the visible page, page overflow rows instead of rendering unselectable entries, and surface cache failures in-popup instead of crashing
- popup redraw now responds to terminal resize, keeps the current page anchor stable, and clamps invalidated pages after cache-driven pane removal
- popup integration tests now cover interactive selection, paging overflow, stale-cache pane fallback, Ctrl-B passthrough, and cache-error rendering
- popup argument handling now rejects `--format` with migration guidance toward `list --format json` and `cache show --format json`
- fixture-backed provider coverage now includes explicit status-source assertions for Gemini, OpenCode, Copilot, and Pi, while Cursor CLI remains intentionally metadata-first unless it presents an explicit Cursor title
- display normalization now strips noisy provider prefixes from title-driven Claude and OpenCode labels and collapses wrapper-heavy Codex titles down to task labels
- `display.activity_label` is now populated for title-driven panes and authoritative wrapper labels when they carry useful activity text, including non-generic Codex wrapper titles
- Cursor CLI detection now treats `pane_current_command=cursor-agent` as the reliable baseline and ignores generic tmux titles for display unless the title is explicitly Cursor-shaped, keeping labels conservative until wrapper metadata is available
- `agentscan focus` supports client-aware tmux switching and has been validated against the current pane workflow
- `agentscan focus` now falls back to the most recently active attached tmux client when no explicit tty is provided
- isolated focus integration tests now validate explicit `--client-tty` targeting, attached-client fallback behavior, and multi-client arbitration toward the most recent attached client
- scanner and daemon snapshot ingestion now consume pane-local `@agent.*` wrapper metadata when present
- `agentscan tmux set-metadata` and `tmux clear-metadata` provide repo-local helpers for managing pane-local `@agent.*` metadata
- repo-local metadata helper writes now rebuild or refresh the existing cache so wrapper-driven metadata remains visible to cache consumers for both daemon-backed and forced-snapshot cache states, even if the prior cache was invalid
- daemon control-mode subscriptions now watch pane-local `@agent.*` metadata fields in addition to pane titles
- targeted daemon writes now reconcile helper-published metadata from the current cache so unrelated daemon updates do not erase wrapper state

Phase 1 status:

- the command surface, popup automation boundary, daemon cache contract, and repo-local tmux integration are implemented in the current branch
- the remaining work is narrower than the original Phase 1 scope and should be treated as finish work rather than missing product shape

Remaining closeout work before treating the current architecture as fully settled:

- a narrow fallback strategy for concrete ambiguous panes where tmux metadata and titles do not provide a reliable answer
- remaining migration and wrapper guidance for broader real-world adoption

## Phase 1

Phase 1 delivers the minimum daemon-backed product that can replace popup-time
rescans without requiring host dotfile migration.

### Outcomes

- control-mode daemon maintains pane state in memory
- daemon writes a canonical JSON cache atomically
- `list`, `inspect`, and `focus` operate against the cache or daemon-managed state
- the popup workflow can run directly from this repo's CLI entrypoint
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
- exit clearly when tmux disappears and let an external supervisor own restart policy

Detection posture:

- use tmux metadata and control-mode state as the primary detection path
- make terminal-title analysis the default classification and status path for all providers
- treat pane-content inspection as a later fallback for concrete unresolved cases only

Acceptance criteria:

- daemon can start with a fresh tmux snapshot
- daemon updates cache after pane topology changes
- daemon exits with clear errors rather than retrying internally

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

- interactive popup command entrypoint
- map popup selection to `agentscan focus`
- render popup rows directly from cached pane state

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
- wrapper adoption and precedence hardening continue from the implemented baseline

### Workstreams

- expand wrapper usage on top of the existing metadata contract
- harden precedence and lifecycle behavior for published metadata
- document wrapper integration patterns and metadata clearing expectations

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

## Tooling Follow-Ups

These are good next-layer quality investments after the current fmt, lint, complexity,
and unit-test baseline:

- broader fixture-heavy tests for real tmux title samples and cache snapshots
- broader property coverage for parser and normalization behavior
- broader benchmark coverage and simple comparison workflows for key paths
- simple performance regression checks around title-first detection and daemon refresh behavior

Concrete sequencing for the remaining finish work now lives in `plans/remaining-work.md`.

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
- when tmux disappears, fail fast and rely on an external supervisor rather than retrying internally

### tmux Control-Mode Ownership

Need to choose:

- single tmux server assumption
- or explicit target selection for multiple tmux servers

Chosen direction:

- support the default current tmux environment first and defer multi-server handling

### Popup UX Shape

Chosen direction:

- dedicated interactive popup command so popup rendering does not depend on human list formatting

### Detection Strategy Validation

Need to choose:

- which provider cases can be covered from title and tmux metadata alone before any deeper inspection is justified
- how to capture failing examples from the legacy workflow as fixtures without inheriting legacy behavior wholesale

Chosen direction:

- start with fixture-driven analysis of real title samples across all supported providers
- prioritize Codex and Claude coverage first, with Gemini and OpenCode treated as secondary until they justify more depth
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
8. Add repo-local tmux popup integration that launches `agentscan popup`.
9. Add fixture-driven tests for parsing, classification, cache serialization, and focus behavior.

## Definition Of Done For V1

V1 is ready for controlled migration when:

- popup-time full rescans are no longer required for normal use
- daemon-backed cache updates are stable in day-to-day tmux usage
- the popup flow works using repo-local integration from this repo
- `list`, `inspect`, and `focus` cover the common daily workflows
- the schema and command surface are documented and stable enough for early adopters
