# Copilot and Cursor Closed-Source Probing

Status: completed for current baseline

## Goal

AUR-155 asked for empirical evidence before adding or hardening support for
closed-source GitHub Copilot and Cursor agent panes. The durable baseline is:

- classify from exact commands, tmux metadata, or foreground process evidence
- treat branding-only strings as weak unless the provider is already known
- use pane-output parsing only as a provider-scoped status fallback
- record false-positive risk before changing classifier behavior

## Repeatable Probing Checklist

Use an isolated tmux server so local sessions, titles, and sockets do not leak
into the evidence:

1. Create a temporary `TMUX_TMPDIR` and tmux socket name.
2. Start the provider CLI in a new tmux session on that socket.
3. Capture tmux metadata:
   - `pane_current_command`
   - `pane_title`
   - `pane_tty`
   - any provider-published tmux user options
4. Resolve the foreground process group for the pane TTY and record:
   - foreground pid and command
   - root and descendant argv
   - selected environment variables, redacting secrets
5. Capture current pane output at each state transition:
   - fresh idle prompt
   - active working state
   - approval or trust prompt
   - return to idle after completion
6. Repeat with title updates disabled or customized when the CLI supports it.
7. Record each signal as strong, supporting-only, or rejected before changing
   classifiers.
8. Add regression coverage for both the accepted signal and the nearest false
   positive.

For `agentscan` subprocess checks against the isolated tmux server, pass
`AGENTSCAN_TMUX_SOCKET` and keep the harness socket and `TMUX_TMPDIR` explicit.

## Evidence Matrix

| Provider | Signal | Strength | Baseline use | False-positive posture |
| --- | --- | --- | --- | --- |
| GitHub Copilot | Exact live foreground command `copilot` or `github-copilot` | Strong | Provider identity | Exact command only; no suffix matching |
| GitHub Copilot | Foreground argv path under `@github/copilot` or platform package `@github/copilot-*/copilot` | Strong | Provider identity when tmux reports `node` | Requires known package path, not arbitrary `copilot` text |
| GitHub Copilot | Default tmux title `GitHub Copilot` | Supporting only | Generic display label after provider identity | Live probing showed the default title, but title branding alone should not establish provider identity |
| GitHub Copilot | Custom `--name` tmux title | Supporting only | Display label after provider identity | Never establishes provider by itself |
| GitHub Copilot | `Thinking (Esc to cancel)` near the current prompt | Strong after identity | `status.source="pane_output"` busy fallback | Scoped to known Copilot panes and current prompt context |
| GitHub Copilot | Folder trust modal text | Strong after identity | Busy fallback | Ignored after a normal prompt appears below the modal |
| GitHub Copilot | Current `❯` prompt plus `/ commands · ? help` footer | Strong after identity | Idle fallback | Requires current prompt/footer anchoring so stale scrollback does not win |
| GitHub Copilot | `COPILOT_HOME`, `COPILOT_MODEL`, similar env | Supporting only | Research context | Never establishes provider alone |
| Cursor CLI | Exact live foreground command `cursor-agent` | Strong | Provider identity | Exact command only |
| Cursor CLI | Exact live foreground command `cursor-cli` | Strong | Provider identity | Exact command only |
| Cursor CLI | Bare command `agent` | Rejected | None | Too generic without future Cursor-specific argv/path evidence |
| Cursor CLI | Foreground process resolves through `cursor-agent` while tmux reports `node` | Strong | Provider identity | Requires foreground process evidence, not title-only branding |
| Cursor CLI | Default title `Cursor Agent` | Supporting only | Generic display label when provider identity exists | Treated conservatively because titles can be generic or user-controlled |
| Cursor CLI | `→ Plan, search, build anything` or `→ Add a follow-up` in the current footer | Strong after identity | Idle fallback | Requires current Cursor footer border and known provider |
| Cursor CLI | Current footer containing `ctrl+c to stop` | Strong after identity | Busy fallback | Scoped to known Cursor panes and current footer |
| Cursor CLI | Braille spinner plus `Running` above the current footer | Strong after identity | Busy fallback | Ordinary response text containing `Running` is ignored |
| Cursor CLI | `CURSOR_AGENT`, `CURSOR_CLI` env | Supporting only | Research context | Never establishes provider alone |

## Captured Baseline

GitHub Copilot CLI probing used version 1.0.39 in an isolated tmux session.
tmux reported `pane_current_command=node`, the default title was
`GitHub Copilot`, and foreground process evidence resolved the native Copilot
package binary. The npm loader path delegated through `@github/copilot` into a
platform package such as `@github/copilot-darwin-arm64/copilot`. During work,
the pane rendered `Thinking (Esc to cancel)` while the tmux title stayed stable.

Cursor CLI probing used the local `cursor-agent` binary in an isolated tmux
session. The default idle pane could expose a generic `Cursor Agent` title while
the foreground process evidence resolved `cursor-agent`. Local CLI inspection
also confirmed `about --format json`, `status --format json`, and `create-chat`
support, with `create-chat` returning a UUID-backed local chat id suitable for
future `@agent.session_id` metadata.

## Shipped Coverage

The runtime baseline is now covered by provider aliases, process evidence, title
normalization, display fallback tests, and provider-scoped pane-output status
tests:

- provider aliases live in `src/app/provider.rs`
- Copilot package/process fallback lives in `src/app/classify/proc_evidence.rs`
- Copilot and Cursor pane-output status fallback lives in
  `src/app/classify/pane_output.rs`
- regression coverage lives in `src/app/tests/classification.rs`,
  `src/app/tests/title_status_display.rs`,
  `src/app/tests/provider_classification.rs`, and `src/app/tests/support.rs`

Relevant commits:

- `956df69 Harden Cursor CLI provider detection`
- `9dd04de Detect Copilot busy pane output`
- `0ec32f9 Infer Copilot and Cursor idle pane output`
- `b72e178 Detect Cursor CLI busy pane output`
- `d503175 Recognize Copilot absolute path idle prompt`
- `aa0fe93 Harden Copilot process fallback`

## Remaining Boundaries

Do not read Copilot or Cursor session stores, logs, or local state files in
baseline detection. Hooks, statusline customization, and wrapper-published
metadata remain optional enrichment paths. If future CLI versions change prompt
or footer shapes, repeat the checklist above and add a fixture for the old false
positive class before broadening heuristics.
