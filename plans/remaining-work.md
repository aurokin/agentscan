# Remaining Work Plan

Status: active

## Purpose

This document tracks the work still needed after the current branch's daemon,
cache, popup, focus, and wrapper-metadata baseline.

It is intentionally narrower than `IMPLEMENTATION_PLAN.md`. The large product
shape decisions are already made. The remaining work is about finishing provider
coverage, adding only the fallbacks that are actually justified, and hardening
the operational edges around the current daemon-backed workflow.

## Current Status

Already shipped in the current branch:

- daemon-backed cache updates from tmux control mode
- cache-backed `list`, `inspect`, `focus`, and `popup`
- interactive-only popup with documented JSON automation alternatives
- wrapper metadata publishing and consumption via pane-local `@agent.*` options
- client-aware focus and daemon integration coverage for common topology changes

Still unfinished:

- fixture-backed status and label depth across the remaining providers
- targeted fallback inspection for concrete ambiguous panes
- daemon lifecycle and stale-cache ergonomics beyond the current fail-fast model
- migration and wrapper guidance for broader real-world adoption

## Finish Criteria

The remaining work is done when:

- all supported providers have fixture-backed title and status coverage at the same confidence level currently reserved for the strongest Codex and Claude paths, except where metadata-first behavior is an explicit product choice
- ambiguous panes that cannot be resolved from tmux metadata and titles have a documented, narrow fallback path with tests and explicit cost boundaries
- daemon/cache commands surface actionable health and staleness diagnostics without changing the explicit-daemon operating model by accident
- the repo docs describe the actual supported workflow without stale "pending" notes for already-shipped behavior

## Workstreams

### 1. Provider Coverage

Goal:

- raise Gemini, OpenCode, Copilot, and Pi coverage to fixture-backed, reviewable status without inventing richer labels from weak signals

Tasks:

- collect representative real tmux title samples for each remaining provider
- add fixture rows and focused tests for provider detection, status inference, and label normalization
- tighten heuristics only when the sample set shows a stable signal
- keep Cursor CLI metadata-first unless real Cursor-shaped titles justify richer title handling

Acceptance criteria:

- new fixtures land for each remaining provider
- classifier tests explain why a provider matched and why weak titles still fall back
- README and implementation docs describe the broadened coverage accurately

### 2. Targeted Fallbacks

Goal:

- handle real ambiguous panes without introducing broad steady-state cost or reviving popup-time pane scraping

Tasks:

- capture concrete ambiguous examples that title-first detection cannot classify correctly
- decide whether `/proc` inspection, incremental `%output` parsing, or explicit wrapper metadata is the right fallback for each case
- implement the smallest fallback needed for the confirmed cases
- add diagnostics that make fallback usage visible in `inspect` output and tests

Acceptance criteria:

- each fallback is tied to at least one committed failing example
- no broad `ps` scan or repeated `capture-pane` loop is introduced into the steady-state path
- fallback decisions and limits are documented in `ROADMAP.md` and `IMPLEMENTATION_PLAN.md` if the architectural boundary changes

### 3. Daemon And Cache Hardening

Goal:

- improve operational behavior around the existing explicit-daemon model without changing the core ownership model accidentally

Tasks:

- define clearer daemon health semantics for stale cache, missing daemon refresh time, and non-daemon snapshots
- expand `agentscan cache` and `agentscan daemon status` diagnostics only where they help users debug the real runtime state
- add tests around stale cache reporting and any new cache or daemon subcommand behavior
- keep restart policy external unless a concrete need justifies changing that decision

Acceptance criteria:

- stale or unavailable daemon state produces actionable output
- cache diagnostics distinguish snapshot, daemon, and forced-refresh states clearly
- command behavior remains consistent with the explicit daemon startup model

### 4. Migration And Wrapper Guidance

Goal:

- make it easier to adopt authoritative pane metadata and the documented JSON/cache interfaces without reintroducing shell-owned detection

Tasks:

- document wrapper expectations for publishing and clearing `@agent.*` metadata
- document the recommended machine-readable surfaces for downstream consumers
- record any provider-specific metadata guidance that improves accuracy without adding runtime inference cost

Acceptance criteria:

- the docs point automation consumers to supported JSON/cache surfaces only
- wrapper authors have a clear contract for provider, label, cwd, state, and session id publication
- no docs imply that `agentscan popup` is a machine-readable interface

## Suggested Order

1. Finish provider fixtures and heuristics for Gemini, OpenCode, Copilot, and Pi.
2. Capture ambiguous examples and add only the fallback mechanisms those cases require.
3. Tighten daemon/cache diagnostics once the classifier behavior is stable enough to support clearer operational messaging.
4. Refresh migration and wrapper docs after the technical work lands so the repo docs stay aligned with the shipped behavior.
