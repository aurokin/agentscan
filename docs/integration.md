# Integration

This document covers the stable integration boundary for `agentscan`: wrapper
metadata, machine-readable outputs, shell ownership, and migration posture.
Active rollout sequencing lives in Linear.

## Automation Contract

Machine-readable consumers should use:

- `agentscan list --format json` for the supported pane listing surface
- `agentscan list --all --format json` when non-agent panes are intentionally needed
- `agentscan cache show --format json` only when a consumer explicitly needs the raw cache envelope

`agentscan popup` is interactive-only. It must not become a popup-shaped JSON or
TSV surface, and unsupported flags should remain normal parse errors rather than
compatibility shims.

## Wrapper Metadata Contract

Launch wrappers may publish explicit pane-local tmux user options:

- `@agent.provider`
- `@agent.label`
- `@agent.cwd`
- `@agent.state`
- `@agent.session_id`

Field semantics:

- `provider`: normalized provider name such as `codex`
- `label`: short user-facing task label for list and popup display
- `cwd`: task root or meaningful working directory
- `state`: optional explicit state such as `busy` or `idle`
- `session_id`: provider-specific resume or chat identifier when useful

## Wrapper Rules

- Publish metadata as early as possible after launch.
- Update only fields the wrapper actually knows.
- Do not invent activity state without strong evidence.
- Missing metadata must not block discovery.
- Explicit metadata overrides heuristic title parsing when present.
- Cursor CLI wrappers should prefer explicit labels and session ids over hoping tmux titles stay rich.

Whether wrappers proactively clear stale metadata on exit remains an integration
choice. Pane disappearance is authoritative for removal, so wrappers do not need
to treat clearing as mandatory to interoperate safely.

## Shell Boundary

Shell remains responsible for:

- aliases and ergonomics in user dotfiles
- provider launch wrappers
- tmux key bindings and popup entrypoints

Shell should not remain responsible for:

- pane discovery
- provider classification
- process scanning strategy
- activity-state inference
- cache management

## Migration Posture

The repo should document only settled integration contracts. Active milestone
work, rollout sequencing, and open execution detail live in Linear until they
are stable enough to promote back into the docs.

The repo-local popup invocation for local testing remains:

```sh
tmux display-popup -E "$PWD/target/debug/agentscan" popup
```
