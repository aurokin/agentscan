# Integration

This document covers the stable integration boundary for `agentscan`: wrapper
metadata, machine-readable outputs, shell ownership, and migration posture.
Active rollout sequencing lives in Linear.

## Automation Contract

Machine-readable consumers should use:

- `agentscan list --format json` for the supported pane listing surface in normal automation flows
- `agentscan list --all --format json` when non-agent panes are intentionally needed
- `agentscan snapshot --format json` only when a consumer explicitly needs the raw snapshot envelope

`agentscan tui` is interactive-only. It must not become a TUI-shaped JSON or TSV
surface, and unsupported formatting requests must not become compatibility
shims. The legacy `agentscan popup` command is removed rather than kept as an
alias. Root-level `--format` routed to the interactive command should continue
to fail with migration guidance rather than rendering machine-readable UI
output.

Migration targets:

| Existing consumer need | Supported target |
|------------------------|------------------|
| Parse agent panes for automation | `agentscan list --format json` |
| Parse all tmux panes, including non-agent panes | `agentscan list --all --format json` |
| Inspect schema version or the unfiltered snapshot envelope | `agentscan snapshot --format json` |
| Open a human pane picker from a tmux bind | `agentscan tui` |

If a script needs data that is missing from the documented JSON surfaces, treat
that as an API gap in `list` or snapshot JSON. Do not add hidden `tui --format`
paths, TUI-shaped TSV, or parser compatibility branches to preserve legacy
stdout parsing.

Normal consumers are daemon-backed. They auto-start the daemon by default so
desktop workflows do not need service setup. Scripts and CI that must avoid
spawning a long-lived process should use `--no-auto-start`,
`AGENTSCAN_NO_AUTO_START=1`, or direct tmux recovery paths such as
`agentscan scan` and refresh-capable command flags.

Status provenance is part of the JSON contract. `status.source="pane_output"`
means the provider was already identified by stronger evidence, and agentscan
then used a provider-specific current prompt/footer/status pattern to infer
`busy` or `idle`. Pane output must not be used as provider identity evidence,
and stale historical lines in the pane tail should not drive current status.

## Wrapper Metadata Contract

Launch wrappers may publish explicit pane-local tmux user options:

- `@agent.provider`
- `@agent.label`
- `@agent.cwd`
- `@agent.state`
- `@agent.session_id`

Field semantics:

- `provider`: normalized provider identity. Canonical values are `codex`,
  `claude`, `gemini`, `opencode`, `copilot`, `cursor_cli`, and `pi`. The
  metadata helper also accepts useful aliases such as `github-copilot`,
  `cursor-cli`, `cursor-agent`, and `pi-coding-agent`, then writes the
  canonical value.
- `label`: short user-facing display text for list and TUI surfaces. It
  should describe the task or conversation only when the wrapper has that
  information directly. Do not derive richer labels from paths, generic tmux
  titles, or weak provider guesses.
- `cwd`: task root or meaningful working directory for the agent workflow. This
  may differ from tmux `pane_current_path` when the wrapper launches from a
  bootstrap directory and then attaches an agent to another project root.
- `state`: optional explicit state. Valid values are `busy`, `idle`, and
  `unknown`. Publish `busy` or `idle` only from direct provider state or a
  wrapper-controlled lifecycle event; otherwise omit the field or publish
  `unknown`.
- `session_id`: provider-specific resume, chat, or conversation identifier when
  one exists and is useful for later wrapper behavior. It is not a tmux session
  id.

Wrappers can publish metadata with:

```sh
agentscan tmux set-metadata \
  --provider codex \
  --label "Review auth flow" \
  --cwd "$PWD" \
  --state busy \
  --session-id "$AGENT_SESSION_ID"
```

All fields are optional, but at least one field must be provided to
`set-metadata`. Empty string values for `label`, `cwd`, and `session_id` are
ignored by the helper rather than written as meaningful metadata.

## Wrapper Rules

- Publish metadata as early as possible after launch.
- Update only fields the wrapper actually knows.
- Do not invent activity state without strong evidence.
- Missing metadata must not block discovery.
- Explicit metadata overrides heuristic title parsing when present.
- Cursor CLI wrappers should prefer explicit labels and session ids over hoping tmux titles stay rich.
- Provider hooks and extensions are deep-roadmap enrichment only. They may
  eventually improve labels, session ids, or activity state, but baseline
  scanner work should first exhaust source analysis, local probing, and
  plug-and-play detection.

## Lifecycle Guidance

Wrappers should publish stable identity metadata as soon as they know it. A
minimal early update with `provider` and `cwd` is useful even before a provider
session id or task label exists. Later partial updates can add `label`,
`state`, or `session_id` without rewriting the whole record.

Partial updates are part of the contract. A wrapper should update only the
fields it knows and leave the rest untouched. This lets one layer publish
provider identity while another layer later publishes the provider session id or
state.

State publication should be conservative:

- Use `busy` when the wrapper has direct evidence that the agent is actively
  working.
- Use `idle` when the wrapper has direct evidence that the agent is ready for
  input or otherwise quiescent.
- Use `unknown` when the wrapper knows the pane is agent-owned but cannot
  honestly report activity.
- Omit `state` when the wrapper has no state signal at all.

Pane disappearance is authoritative cleanup. Wrappers do not need to clear
metadata on normal pane exit because the pane record disappears with the pane.
Explicit clearing is useful for long-lived panes, wrapper handoff, provider
restart inside the same pane, or failed launches where stale metadata would
otherwise remain visible:

```sh
agentscan tmux clear-metadata --field state --field session-id
```

Calling `clear-metadata` with no `--field` clears all `@agent.*` fields.

## Provider Notes

Provider-specific guidance should stay narrow and correctness-driven:

- Claude Code panes are strongest when wrappers publish explicit metadata. The
  scanner can also resolve unresolved launcher panes from Claude Code process
  evidence, including the `@anthropic-ai/claude-code` CLI path and tmux teammate
  spawns that carry Claude Code agent flags plus `CLAUDECODE=1`.
- Codex and Claude Code are candidates for eventual hook-based metadata
  publishing, but hook support is deferred to the end of the provider roadmap.
  Hooks should publish explicit tmux metadata rather than becoming a required
  detection dependency.
- Pi should remain plug-and-play from its default `pi` process, package paths,
  `PI_CODING_AGENT=true` environment marker, and Greek terminal title shape.
  Pi extension support is a deep-roadmap additive path for richer metadata;
  default Pi titles should not be used to invent busy or idle state.
- Cursor CLI should be treated as metadata-first for labels and session ids.
  The live `cursor-agent` command is enough to identify the provider, but tmux
  titles are often generic. Wrappers should publish `@agent.label` and
  `@agent.session_id` when they can obtain those values. Baseline status can
  fall back to provider-scoped pane output after identity is known: current
  Cursor footers can report idle, and current running footer/status-line shapes
  can report busy with `status.source="pane_output"`.
- opencode should remain plug-and-play from its upstream `OpenCode` and
  `OC | ...` terminal title shapes, targeted package or shim path evidence, and
  Linux `OPENCODE` environment marker. Its default terminal title does not
  publish busy or idle state; keep status unknown unless explicit tmux metadata
  supplies state.
- GitHub Copilot and Cursor are closed-source enough that support should be
  based on empirical probing. Copilot baseline status can fall back to
  provider-scoped pane output after identity is known: current prompt/footer
  shapes can report idle, and current thinking or folder-trust prompts can
  report busy with `status.source="pane_output"`. Record command, title, argv,
  env, and state snapshots before adding or changing heuristics.
- Wrapper-shaped panes with generic shell commands should publish metadata
  rather than relying on path, window name, or title inference. The ambiguity
  corpus in `docs/notes/ambiguity-corpus.md` records examples where weak tmux
  evidence intentionally remains `unknown`.

## Shell Boundary

Shell remains responsible for:

- aliases and ergonomics in user dotfiles
- provider launch wrappers
- tmux key bindings and TUI entrypoints
- choosing when to invoke `agentscan list`, `agentscan focus`, or `agentscan tui` in a user workflow

Shell should not remain responsible for:

- pane discovery
- provider classification
- process scanning strategy
- activity-state inference
- daemon lifecycle policy
- shaping machine-readable pane output

## Migration Posture

The repo should document only settled integration contracts. Active milestone
work, rollout sequencing, and open execution detail live in Linear until they
are stable enough to promote back into the docs.

Host-specific dotfiles can migrate incrementally. During migration, shell code
should switch parsing consumers to `agentscan list --format json` or `agentscan
snapshot --format json` and keep `tui` limited to interactive tmux flows.

The repo-local tmux `display-popup` invocation for local testing is:

```sh
tmux display-popup -E "$PWD/target/debug/agentscan" tui
```
