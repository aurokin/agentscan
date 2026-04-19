# Remaining Work Plan

Status: active

## Purpose

This document tracks the work still needed after the current branch's daemon,
cache, popup, focus, and wrapper-metadata baseline.

It is intentionally narrower than `IMPLEMENTATION_PLAN.md`. The large product
shape decisions are already made. The remaining work is now mostly about adding
only the fallbacks that are actually justified and finishing the migration and
wrapper guidance around the current daemon-backed workflow.

## Current Status

Already shipped in the current branch:

- daemon-backed cache updates from tmux control mode
- cache-backed `list`, `inspect`, `focus`, and `popup`
- interactive-only popup with documented JSON automation alternatives
- wrapper metadata publishing and consumption via pane-local `@agent.*` options
- client-aware focus and daemon integration coverage for common topology changes
- daemon/cache diagnostics now distinguish daemon-backed, stale, and snapshot-only cache states and surface daemon refresh provenance in text output
- fixture-backed status coverage now includes explicit status-source assertions for Gemini, OpenCode, Copilot, and Pi without weakening the conservative display-label policy

Still unfinished:

- targeted fallback inspection for concrete ambiguous panes
- migration and wrapper guidance for broader real-world adoption

## Finish Criteria

The remaining work is done when:

- ambiguous panes that cannot be resolved from tmux metadata and titles have a documented, narrow fallback path with tests and explicit cost boundaries
- the repo docs describe the actual supported workflow without stale "pending" notes for already-shipped behavior

## Workstreams

### 1. Targeted Fallbacks

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

### 2. Migration And Wrapper Guidance

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

1. Capture ambiguous examples and add only the fallback mechanisms those cases require.
2. Refresh migration and wrapper docs so the repo docs stay aligned with the shipped behavior.
