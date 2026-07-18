# Agent Metadata Contract

This is the canonical specification for how a coding-agent CLI (or a thin
launch wrapper around one) publishes explicit pane metadata to `agentscan`.
It is implementable in an afternoon: three core fields, a handful of optional
enrichment fields, and a staleness guard.

Publishing metadata is always optional. agentscan's zero-config inference
(titles, process commands, provider-scoped pane output) remains the baseline
for every pane; plug-and-play detection is a product invariant and hooks or
wrappers must never become a prerequisite for discovery. Published metadata
exists to replace inference with ground truth where the agent has it —
including the one state no heuristic can reliably produce: `waiting`.

## Transport

Metadata travels as pane-local tmux user options. agentscan's daemon already
subscribes to every `@agent.*` field over tmux control mode, so a write fires
an event and lands in the next snapshot without any polling or `capture-pane`
work.

```sh
tmux set-option -p @agent.state busy
```

Alternatively, use the bundled helper, which validates values and resolves the
current pane:

```sh
agentscan tmux set-metadata --state busy
```

Rules for emitters:

- Publish on startup and on state transitions. Never publish on a timer.
- Guard every publish with `[ -n "$TMUX" ]` and swallow failures; metadata
  publication must never break the agent when tmux is absent or the write
  fails.
- Clear your fields on clean exit (trap/atexit). Pane disappearance is
  authoritative cleanup, so this only matters for long-lived panes where the
  shell outlives the agent; `@agent.pid` covers crashes either way.
- Update only fields you actually know. Partial updates are part of the
  contract: one layer may publish `provider` early and another layer add
  `session_id` or `state` later.
- An empty string value means "absent". agentscan treats `""` exactly like an
  unset option, so clearing by writing `""` is equivalent to unsetting.

## Fields

### Core (publish these three)

| Field | Value | What it eliminates |
|---|---|---|
| `@agent.provider` | canonical provider slug (`claude`, `codex`, `aider`, `gemini`, `antigravity`, `opencode`, `copilot`, `cursor_cli`, `pi`, `grok`, `hermes`, `droid`, `kimi_code`) | all title/command/process-tree identity inference |
| `@agent.state` | `busy` \| `idle` \| `waiting` \| `unknown` | title glyph parsing and pane-output capture for this pane |
| `@agent.pid` | PID of the publishing agent process | the stale-metadata/ghost-pane bug class (see Trust rule) |

Plus the contract version:

| Field | Value | Semantics |
|---|---|---|
| `@agent.v` | integer, currently `1` | Version of this contract the emitter implements. Absence means v0: the pre-`waiting`, pre-pid contract documented historically in `docs/integration.md`. agentscan uses this to gate future semantics changes; emitters should publish `1`. |

### Enrichment (each independently optional)

| Field | Value |
|---|---|
| `@agent.label` | short user-facing task/conversation label for list, TUI, and dock surfaces; only publish when the agent knows it directly |
| `@agent.session_id` | provider-specific resume/chat/conversation identifier (not a tmux session id) |
| `@agent.cwd` | task root when it differs from tmux `pane_current_path` (bootstrap-dir launches) |
| `@agent.model` | model slug in the provider's own vocabulary, e.g. `claude-opus-4-1`; carried into the JSON snapshot (`agent_metadata.model`, schema 7+) and shown in inspect output |

## State semantics

- `busy` — the agent is actively generating or executing.
- `idle` — the agent is at its prompt with nothing pending.
- `waiting` — the agent is blocked on human input: a permission/approval
  prompt, a question, a confirmation. This is the state users care most about
  and the one heuristics cannot reliably produce. Publish it the moment the
  agent blocks; publish the follow-up `busy`/`idle` the moment it unblocks.
- `unknown` — the pane is agent-owned but the emitter cannot honestly report
  activity. agentscan treats a published `unknown` as "no answer" and falls
  through to its own inference, which may still resolve a status.

Never publish `busy`/`idle`/`waiting` from guesses; omit `state` (or publish
`unknown`) when there is no direct signal.

## Trust and staleness

tmux user options outlive the process that wrote them, so a crashed agent's
metadata could otherwise mislabel whatever runs in the pane next. `@agent.pid`
is the guard:

- If `@agent.pid` is absent, the block is used as published (v0
  compatibility).
- If `@agent.pid` is present, agentscan trusts the *entire* `@agent.*` block
  only when that pid parses as a positive integer, is alive, and is a
  descendant of (or equal to) the pane's process-tree root. The check runs
  against the same lazy process snapshot the classifier already holds, and
  only for panes that publish a pid.
- If the check fails — dead pid, pid outside the pane's process tree,
  non-numeric junk — the whole block is ignored (provider, state, label,
  everything) and the pane falls through to normal inference. There is no
  partial trust: an emitter that opts into the staleness guard is trusted
  all-or-nothing.

## Precedence

Trusted explicit metadata beats inference, symmetrically for identity and
status:

- `@agent.provider` wins over command matching, title heuristics, and
  process-tree evidence (`classification.matched_by="pane_metadata"`).
- `@agent.state` (`busy`/`idle`/`waiting`) wins over title-derived status and
  suppresses the pane-output capture fallback entirely for that pane
  (`status.source="pane_metadata"`). A published `unknown` falls through to
  heuristics as described above.

When metadata is absent or untrusted, the full inference ladder applies
unchanged. Inference is the permanent zero-config fallback, not a deprecated
path.

## Emitter recipes

Startup:

```sh
if [ -n "$TMUX" ]; then
  tmux set-option -p @agent.provider claude 2>/dev/null || true
  tmux set-option -p @agent.pid "$$"        2>/dev/null || true
  tmux set-option -p @agent.v 1             2>/dev/null || true
  tmux set-option -p @agent.state idle      2>/dev/null || true
fi
```

State transition (call at each edge — turn start, blocking prompt, turn end):

```sh
publish_state() {
  [ -n "$TMUX" ] || return 0
  tmux set-option -p @agent.state "$1" 2>/dev/null || true
}

publish_state busy      # turn started
publish_state waiting   # blocked on an approval prompt / question
publish_state idle      # back at the prompt
```

Clean-exit cleanup:

```sh
cleanup_metadata() {
  [ -n "$TMUX" ] || return 0
  for f in provider state pid v label cwd session_id model; do
    tmux set-option -p -u "@agent.$f" 2>/dev/null || true
  done
}
trap cleanup_metadata EXIT
```

Or with the helper:

```sh
agentscan tmux set-metadata --provider claude --pid "$$" --v 1 --state idle
agentscan tmux set-metadata --state waiting
agentscan tmux clear-metadata            # clears all @agent.* fields
agentscan tmux clear-metadata --field state --field session-id
```

The helper accepts `--pane-id` to target a pane other than the current one and
accepts provider aliases (`aider-chat`, `cursor-agent`, `kimi-code`, ...),
writing the canonical slug.

## Versioning notes

- The JSON snapshot envelope carrying these fields is `schema_version` 7:
  schema 7 added the `waiting` status wire value and the
  `agent_metadata.pid`/`v`/`model` passthrough.
- `@agent.v` versions the *emitter* contract independently of the snapshot
  schema. New emitters publish `1`.
