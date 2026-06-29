# Provider Evidence Ledger

This ledger summarizes accepted provider evidence at a high level. It is not a
replacement for tests or source/probing notes; it is the quick map for what
kind of evidence currently supports each provider.

Baseline rule: common agent panes should work without provider hooks,
extensions, launch wrappers, or shell integration. Hooks and metadata remain
optional enrichment, not prerequisites.

## Evidence Classes

- `metadata`: explicit `@agent.*` tmux user options or aliases accepted by
  `agentscan tmux set-metadata`.
- `command`: exact command or known package/shim path evidence.
- `title`: observed terminal title or window name shape.
- `process`: targeted native process-tree evidence for unresolved ambiguous
  panes.
- `pane_output`: provider-scoped status fallback after provider identity is
  already known.

## Current Providers

| Provider | Identity evidence | Status evidence | Notes |
|----------|-------------------|-----------------|-------|
| Claude Code | metadata, title, command/path, targeted process evidence | title/metadata where available; provider-scoped current prompt/footer, interrupt hint, and permission-wait pane-output fallback | Targeted process evidence covers launcher and teammate-spawn shapes; pane-output status is only used after identity is known. |
| Codex | metadata, command/title shapes | current prompt/footer/status pane-output fallback for supported layouts | Model/path and plan/goal footer shapes are accepted only as current status context. |
| Gemini CLI | metadata, command/title/source-observed shapes | provider-scoped current prompt/action-required fallback | Deprecated maintenance status: support remains for existing and enterprise users, but Gemini CLI drift is not an active update target. Generic Gemini mentions in unrelated titles are not provider identity. |
| Antigravity | metadata, exact `agy` command | unknown unless explicit metadata or future provider-specific fallback | Keeps status conservative. |
| opencode | metadata, upstream-observed `OpenCode` / `OC | ...` titles, package/shim paths, Linux `OPENCODE` env evidence | provider-scoped current prompt/status fallback where supported | Session titles alone do not invent state. |
| GitHub Copilot CLI | metadata, command/package path/title shapes | current prompt/footer/thinking/trust-prompt pane-output fallback | Stale thinking lines are ignored. |
| Cursor CLI | metadata-first, command/path aliases, specific task/status title shapes | current Cursor footer/status pane-output fallback | Generic Cursor titles remain conservative display labels. |
| Pi | metadata, upstream-observed Greek titles, package/shim paths, Linux `PI_CODING_AGENT=true` | current editor footer/retry/working loader fallback | Bare `pi` commands are not enough. |
| Grok | metadata, command/title shapes from local probing | running body marker and current footer fallback | Approval footer remains conservative unless directly supported. |
| Hermes | metadata, command/path aliases | current prompt fallback and wrapper-published labels | Title text alone is not provider identity. |
| Aider | metadata, exact `aider` command, `python -m aider`, known `aider-chat` package paths, Python console-script invocations | unknown unless explicit metadata publishes state | Upstream prompt is a generic `> ` shape, so pane output is not a durable status source. |
| Factory Droid | metadata, exact `droid` command | current Droid prompt/footer fallback | `⛬ ...` titles are display labels only after identity is known. |

## Rejected Or Supporting-Only Signals

- Pane output is never provider identity.
- Generic title mentions without a known provider shape are not identity.
- Bare short commands that collide with common tools stay conservative unless
  supported by stronger evidence.
- Historical logs, transcripts, provider databases, and session stores are not
  core detection inputs.
- Wrapper metadata can publish richer labels or explicit state, but baseline
  detection must still work without it for common launches.

## Where To Add Detail

- Add source-analysis or probing notes under `docs/notes/` when a provider
  needs durable evidence details.
- Add tests for every accepted signal and the closest likely false positive.
- Update this ledger when a provider gains or loses an evidence class.
