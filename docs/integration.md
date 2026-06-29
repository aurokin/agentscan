# Integration

This document covers the stable integration boundary for `agentscan`: wrapper
metadata, machine-readable outputs, shell ownership, and migration posture.
Active rollout sequencing lives in Linear.

## Automation Contract

Machine-readable consumers should use:

- `agentscan list --format json` for the supported pane listing surface in normal automation flows
- `agentscan list --all --format json` when non-agent panes are intentionally needed
- `agentscan snapshot --format json` only when a consumer explicitly needs the raw snapshot envelope
- `agentscan subscribe --format json` for a live JSON Lines stream of daemon
  subscription events
- `agentscan daemon status --format json` for daemon lifecycle, socket, and readiness checks
- `agentscan providers --format json` for supported provider names, display
  markers for all icon modes, marker codepoints, and aliases
- `agentscan hotkeys --format json` for the shared picker display model used by
  tmux binds, the terminal TUI, and desktop picker surfaces

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
| Subscribe to live daemon updates locally or over SSH | `agentscan subscribe --format json` |
| Check daemon lifecycle or readiness | `agentscan daemon status --format json` |
| Inspect supported providers, icon modes, and aliases | `agentscan providers --format json` |
| Render a pane picker with stable selection keys | `agentscan hotkeys --format json` |
| Activate a picker selection from automation or desktop code | `agentscan hotkey <key>` |
| Activate a picker selection from a tmux key binding | `agentscan tmux hotkey <key>` |
| Open a human pane picker from a tmux bind | `agentscan tui` |
| Force a direct tmux read for recovery or debugging | `agentscan scan` or a supported `--refresh` flag |

Removed surfaces do not have compatibility aliases:

- `agentscan popup` is replaced only for human picker launch paths by
  `agentscan tui`.
- `agentscan cache`, `cache path`, and `cache validate` are removed. Normal
  automation should use `list --format json`; consumers that need raw envelope
  fields should use `snapshot --format json`.
- `AGENTSCAN_CACHE_PATH` is removed with the cache file IPC transport.

There is no cache-file IPC replacement. Socket-isolated tests and harnesses
should use `AGENTSCAN_SOCKET_PATH` when they need a non-default daemon socket.

## Configuration

Provider icon rendering is presentation-only. It does not change provider
classification, daemon socket snapshots, or machine-readable pane records.

Supported modes are `emoji`, `nerd-font`, and `nerd-font-patched`. The default is
`emoji`. The patched Nerd Font mode emits provider glyphs from the
`agent-icons-v9` manifest and requires a terminal font patched with those
private-use codepoints.

Resolution precedence is:

1. `--icons <mode>` on human-facing commands
2. `AGENTSCAN_ICONS=<mode>`
3. `${XDG_CONFIG_HOME:-~/.config}/agentscan/config.toml`
4. the built-in `emoji` default

The config file shape is:

```toml
icons = "emoji"
picker_group_by = "session"
picker_keys = [
  "1", "2", "3", "4", "5",
  "Q", "E", "R", "F", "G", "T",
  "Z", "X", "C", "V", "B",
]
disable_reconcile = true
disable_proc_fallback = false
```

`picker_keys` customizes the shared picker key order. It is config-file only:
there is no CLI or environment override. The list remaps the 16 selection slots
and must contain exactly 16 unique single ASCII letters or digits. Letters are
normalized case-insensitively. `N` and `P` are reserved for TUI paging.

`picker_group_by` customizes the picker grouping and row order. It is
config-file only: there is no CLI or environment override. Valid values are
`session`, `git-repo`, and `cwd`. `session` is the default and preserves the
current tmux-location order. `git-repo` and `cwd` order rows by workspace group,
then tmux location; picker hotkeys are assigned after that ordering.

`disable_reconcile` and `disable_proc_fallback` are diagnostic runtime knobs.
Their environment variables, `AGENTSCAN_DISABLE_RECONCILE` and
`AGENTSCAN_DISABLE_PROC_FALLBACK`, override config file values.
`disable_reconcile` defaults to `true`: every session is driven by control-mode
events (the daemon runs one event subscriber client per session, since control
mode is session-scoped), so the periodic redundancy reconcile is off. The poll is
not disabled entirely — it is reduced to an infrequent self-heal/drift backstop
(default 300s). Broker fallback (no events) always keeps the fast reconcile. Set
`disable_reconcile = false` to restore the full 30s redundancy reconcile (useful
for populating the redundancy meter). See `docs/daemon-operations.md` for the full
interval matrix.

If a script needs data that is missing from the documented JSON surfaces, treat
that as an API gap in `list` or snapshot JSON. Do not add hidden `tui --format`
paths, TUI-shaped TSV, or parser compatibility branches to preserve legacy
stdout parsing.

## Picker Hotkey Contract

Picker selection keys are assigned by `agentscan`, not by tmux shell glue or
desktop UI code. The default shared key order is:

```text
1 2 3 4 5 Q E R F G T Z X C V B
```

Use `agentscan hotkeys --format json` to render a picker outside the terminal
TUI. Each row includes the assigned key, pane id, provider, status, display
metadata, display label, structured location, location tag, and workspace
context. Desktop surfaces should consume these rows directly, or use the
returned `pane_id` with
`agentscan focus <pane-id>` when acting on a row they already rendered.
Consumers must render the returned `key` field rather than assuming the default
order because users may configure `picker_keys` and `picker_group_by`.

`location_tag` remains the tmux address (`session:window.pane`) in every
grouping mode. `workspace.label` is the human grouping label, `workspace.id` is
the machine grouping identity for clients, and `workspace.source` reports
whether it came from `session`, `git_repo`, or `cwd`.

Use `agentscan hotkey <key>` as the strict action path for automation and
desktop callers. It normalizes key case, resolves the key against the current
picker model, delegates focus through the same pane validation and tmux focus
behavior as `agentscan focus`, and exits non-zero when the key is invalid,
unassigned, stale, or cannot be focused.

After a successful focus switch, `agentscan focus`, `agentscan hotkey`, and the
terminal TUI send a best-effort event to the daemon. This event does not set
state directly; it asks the daemon to re-read tmux and publish a fresh snapshot
so `subscribe` consumers can update focused-pane UI without waiting for the next
tmux control-mode notification or reconcile tick.

Use `agentscan tmux hotkey <key>` from tmux key bindings. It uses the same
picker and focus path, but reports action failures through `tmux
display-message` and exits successfully so tmux does not open command output
view for expected picker misses.

These commands are daemon-backed by default and support `--refresh` for direct
tmux recovery. `hotkeys` also supports `--all`; `hotkey` and `tmux hotkey`
accept `--all` when a binding intentionally targets a picker model that includes
non-agent panes.

## Live Subscription Stream

Use `agentscan subscribe --format json` when a long-lived consumer needs live
daemon updates. The command emits newline-delimited JSON frames and flushes each
frame as it is written. The stream starts with connection lifecycle frames such
as `connecting`, then emits `snapshot` frames for the bootstrap and later daemon
updates. Terminal-adjacent tools and the desktop app should consume this stream
instead of connecting to the daemon Unix socket directly.

The stream is designed to be transport-neutral. Local consumers can spawn
`agentscan subscribe --format json`; remote consumers can run the same command
through SSH, for example:

```bash
ssh workbox agentscan subscribe --format json
```

Fatal setup or compatibility failures are emitted as a `fatal` frame before the
process exits non-zero. Daemon shutdown is emitted as a `shutdown` frame before
the stream exits successfully. Closing the consumer side of stdout stops the
subscription without requiring an explicit daemon command.

Normal consumers are daemon-backed and auto-start the daemon by default so
desktop workflows do not need service setup. On macOS, detached auto-start is
allowed only after parent-side executable trust preflight succeeds. Ad-hoc or
local development builds should run `agentscan daemon run` in a long-lived tmux
pane, or use direct tmux recovery paths such as `agentscan scan` and
refresh-capable command flags. Scripts and CI that must avoid spawning a
long-lived process should use `--no-auto-start` or `AGENTSCAN_NO_AUTO_START=1`.

Status provenance is part of the JSON contract. `status.source="pane_output"`
means the provider was already identified by stronger evidence, and agentscan
then used a provider-specific current prompt/footer/status pattern to infer
`busy` or `idle`. Pane output must not be used as provider identity evidence,
and stale historical lines in the pane tail should not drive current status.

`agentscan daemon status --format json` includes runtime telemetry counters for
clients and developers that need to evaluate event-driven behavior:

- `control_event_batch_count`
- `control_event_refresh_count`
- `control_event_line_count`
- `reconcile_attempt_count`
- `reconcile_noop_count`
- `reconcile_changed_snapshot_count`
- `targeted_refresh_fallback_to_full_count`
- `broker_fallback_count`
- `control_mode_broker_subscriber_count` — per-session event-only subscriber
  clients currently attached (one per non-primary session)
- `control_mode_broker_subscriber_coverage_complete`
- `control_mode_broker_missing_subscriber_session_ids`
- `control_mode_broker_subscribers` — per-subscriber session id, pid,
  start time, last line/event timestamps, restart count, and dead flag
- `control_mode_broker_next_subscriber_monitor_in_ms`
- `subscriber_monitor_count`
- `subscriber_start_count`
- `subscriber_reattach_count`
- `subscriber_attach_failure_count`
- `subscriber_exit_count`

When the daemon is not running, has not finished initializing runtime
telemetry, or is an older compatible daemon that does not publish these
counters, these fields are `null`. Once runtime telemetry is available they are
numeric counters for the current daemon process. These are diagnostic signals,
not subscription heartbeats; live consumers should still react to
`agentscan subscribe --format json` frames.

No-op reconcile passes are intentionally silent on the subscription stream. Use
the daemon status counters to observe reconcile activity; do not rely on
periodic snapshot frames as heartbeats.

## SSH Desktop Transport Contract

Local and remote desktop clients use the same `agentscan` command contract.
The desktop shell owns process execution, SSH orchestration, window lifecycle,
global hotkeys, rendering, and retry policy. The host that owns tmux owns the
daemon, discovery, classification, picker rows, and focus actions.

The detailed local/SSH command contract, remote compatibility checks, expected
failure surfaces, and remote smoke plan live in
`docs/desktop-client-contract.md`.

## Wrapper Metadata Contract

Launch wrappers may publish explicit pane-local tmux user options:

- `@agent.provider`
- `@agent.label`
- `@agent.cwd`
- `@agent.state`
- `@agent.session_id`

Field semantics:

- `provider`: normalized provider identity. Canonical values are `codex`,
  `claude`, `aider`, `gemini`, `antigravity`, `opencode`, `copilot`,
  `cursor_cli`, `pi`, `grok`, `hermes`, and `droid`. The metadata helper also
  accepts useful aliases such as `aider-chat`, `agy`, `github-copilot`,
  `cursor-cli`, `cursor-agent`, `pi-coding-agent`, `hermes-agent`, and
  `factory-droid`, then writes the
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
- Gemini CLI support is deprecated but retained. Existing and enterprise users
  can keep using the supported evidence paths, but Gemini CLI drift is not an
  active maintenance target for new status or UI updates.
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
- Aider should remain plug-and-play from the exact `aider` command,
  `python -m aider`, explicit metadata aliases, known `aider-chat` package paths,
  and Python console-script invocations. Its upstream prompt is a generic `> `
  prompt and should not drive pane-output status; keep status unknown unless
  explicit tmux metadata publishes state.
- GitHub Copilot and Cursor are closed-source enough that support should be
  based on empirical probing. Copilot baseline status can fall back to
  provider-scoped pane output after identity is known: current prompt/footer
  shapes can report idle, and current thinking or folder-trust prompts can
  report busy with `status.source="pane_output"`. Record command, title, argv,
  env, and state snapshots before adding or changing heuristics.
- Factory Droid CLI support is based on local empirical probing. The exact
  `droid` command is provider identity; `⛬ ...` titles are display labels only
  after identity is known. Baseline status can fall back to provider-scoped
  pane output after identity is known: the current boxed prompt can report
  idle, and current streaming/steering prompt shapes can report busy with
  `status.source="pane_output"`.
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
