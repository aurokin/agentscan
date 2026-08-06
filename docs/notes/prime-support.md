# Prime Agent (Prime Intellect) Support

Status: completed for current baseline

## Goal

Add plug-and-play support for Prime Agent, Prime Intellect's fork of the Pi
coding agent (pi-mono), with hard guarantees that Pi and Prime never claim each
other's panes in either direction. Support started from upstream source
analysis of the installed distribution plus empirical probing of a
harness-owned session.

## Upstream Source Analysis

Analyzed version: `prime-agent 0.7.0`, installed via mise at
`~/.local/share/mise/installs/github-prime-intellect-ai-prime-agent/0.7.0/`.

Prime is a rebrand-by-config fork: a `piConfig` package field drives
`APP_NAME = "prime-agent"`, `APP_TITLE = "prime-agent"`, config dir
`~/.prime/agent`, and env prefix `PRIME_AGENT` (`dist/config.js`). Pi upstream
uses `pi`, `π`, `.pi`. Extensions load only from `~/.prime/agent/extensions/`,
never `~/.pi`; skills share the `~/.agents/skills` namespace with Pi.

Load-bearing leftover: `dist/cli-main.js` still sets
`process.env.PI_CODING_AGENT = "true"` — exactly agentscan's Pi env marker —
and every tool child inherits it. Prime sets no Prime-specific marker on
pane-side processes (`PRIME_AGENT_*` vars are installer/daemon-scoped), so env
alone can never distinguish a Prime child from a Pi child. Reported upstream as
PrimeIntellect-ai/prime-agent#750. agentscan handles it with a scoped
suppression: Prime identity evidence anywhere in the pane's process tree beats
the Pi env rung for that pane (see `ROADMAP.md`, Detection Policy).

## Local Probing

macOS probing used an isolated tmux server (temporary `TMUX_TMPDIR`, dedicated
socket) running a harness-owned `prime-agent 0.7.0` session, plus read-only
inspection of a live session.

Observed process shape:

- `dist/cli-main.js` rewrites `process.title` to `prime-agent`, which clobbers
  argv: KERN_PROCARGS2 yields argv `["prime-agent", ""]` (note the trailing
  empty arg — harmless to argv0 matching) and the executable path is the node
  binary, not a prime path.
- `ps comm` shows the argv-derived `prime-agent`, while the kernel `pbi_comm`
  (agentscan's `ProcessEvidence.command`) is `node`. Identity therefore anchors
  on argv0/comm `prime-agent` through the normal command-alias ladder; no
  Prime-specific arg patterns exist because a path-suffix pattern could
  essentially never fire.
- tmux `pane_current_command` surfaces as `node` or `prime-agent` depending on
  launch shape; both are handled (`prime-agent` via the command alias, `node`
  via proc fallback and the title spec).
- Prime daemonizes workers/kernels under ppid 1; they are outside every pane
  tree and must never be matched globally.
- No tmux `@agent.*` user options are published.

Observed pane title: `prime-agent - <session> - <repo>` (idle form
`prime-agent - <cwd basename>` at startup). Like Pi's `π - ` titles, the OSC
title outlives the session, so the title hint only classifies over a live
Prime runtime foreground (`prime-agent`/`node`/`bun`) or a spinner glyph.

Observed idle frame (harness capture):

- prompt row ` >   Try "explain how @<filepath> works"` (rendered during busy
  turns too, so it corroborates a live frame rather than deciding status)
- footer `← agents/resume  GPT-5.6 Sol • medium  ? for shortcuts  …  0 (0%)`;
  the context pair renders as `0 (0%)`, `34 (0%)`, `6.0k (2%)` — count with
  optional `k`, then parenthesized percentage

Observed busy frames:

- a braille-spinner loader above the prompt/footer:
  `⠇ Waiting · 0s`, `⠼ Thinking · 2s`, `⠸ Executing · 3s · ↑ 34 tokens`;
  label set from `AGENT_ACTIVITY_LABELS` in the interactive mode source
  ({Waiting, Thinking, Writing, Writing code, Executing})
- tool summary rows (`◆ bash · sleep 15 · ↑ 1 lines · Ctrl+O to expand`) and a
  hint row may sit between the loader and the footer (up to seven rows
  observed)
- `Waiting` is model latency, not a human-blocking question; Prime's built-in
  toolset has no confirmed blocking tool, so no `waiting` status is reported

## Evidence Matrix

| Signal | Strength | Baseline use | False-positive posture |
| --- | --- | --- | --- |
| Exact `prime-agent` command/argv0 | Strong | Provider identity | Exact only, no suffix matching; `prime` and `prime-*` never match |
| Metadata aliases `prime`, `prime-agent`, `prime agent` | Strong | Provider identity when wrapper-published | Explicit tmux metadata only |
| `prime-agent - <session>` title over a live runtime foreground | Strong | Provider identity + normalized label | Gated by the stale-title guard (mirrors Pi's): non-runtime foregrounds defer to process evidence; empty remainder never matches |
| Footer `•` separator + right-aligned `<count>[k] (<pct>%)` pair | Strong after identity | `status.source="pane_output"` frame gate | Mutually exclusive with Pi's `%/` / `?/N` footer tokens by construction |
| Braille spinner + activity label + `· <elapsed>` loader within a bounded window above the current footer | Strong after identity | Busy fallback | Activity words without the spinner glyph never match; a spinner-led row the matcher cannot confirm degrades to unknown, never idle |
| Prompt row `> …` near the current footer with no spinner-led row | Strong after identity | Idle fallback | Prompt renders during busy turns, so it only reads idle when no spinner-led row exists |
| `PI_CODING_AGENT=true` env on pane processes | Rejected for Prime identity | Pi env rung, suppressed when Prime identity is present in the tree | Inherited from Pi upstream (leftover in the fork); can never distinguish the fork from upstream |
| `PRIME_ARG_PATTERNS`-style argv path evidence | Rejected | None | The title rewrite clobbers argv on macOS and Linux, so a path pattern would be dead code |
| Prime daemon/worker processes (ppid 1) | Rejected | None | Outside pane trees by design; matching comm globally would claim unrelated panes |

## Unprobed States

Left `unknown` by design; probe before encoding richer signals:

- approval/permission dialogs, `/login`, and alternate full-screen UIs
- non-default models' footer shapes and long `1m 5s`-style elapsed formats
  (accepted loosely by the elapsed check but not observed)
- Linux process shape (argv/comm after the title rewrite) — assumed to mirror
  macOS via Node's process.title semantics, not empirically confirmed
- whether future Prime releases keep the `PI_CODING_AGENT` leftover (the
  suppression stays as belt-and-suspenders either way)

## Icons

- Emoji `🦋`; stock Nerd Font glyph `U+F1589` (`nf-md-butterfly`), verified
  present in shipped Nerd Fonts
- Patched glyph `U+10005B`, the next free codepoint in the agent-icons block
  (Prime butterfly mark from the upstream repo's `assets/brand/`)
