# Kimi Code (Moonshot Kimi CLI) Support

Status: completed for current baseline

## Goal

Add conservative plug-and-play support for Moonshot's Kimi CLI ("Kimi Code").
Kimi is closed enough that support started from empirical local probing plus
upstream inspection, then encoded only low-risk live signals.

## Local Probing

Probed version: `kimi 0.27.0` (`~/.kimi-code/bin/kimi`), default model alias
`kimi-code/kimi-for-coding` (display name `K2.7 Coding`).

macOS probing used an isolated tmux server with a temporary `TMUX_TMPDIR` and a
single `kimi` pane. Linux probing (CachyOS host, same version) used a live
authenticated pane and confirmed identical command evidence.

Observed idle startup:

- `pane_current_command=kimi` on macOS and Linux
- kernel process comm / argv[0] is `kimi-code` on macOS; `kimi` on Linux —
  tmux never surfaces `kimi-code` as `pane_current_command` on either platform
- `pane_title=Kimi Code` at launch, set via terminal escape
- process tree: shell pane pid with a single `kimi` child; no tmux `@` user
  options published
- current idle output includes the `Welcome to Kimi Code!` banner, a bordered
  input box line `│ >`, and a footer of
  `<model display name> thinking  <cwd>  <git branch>` plus
  `context: N% (x/256k)`

Observed busy state after a short prompt:

- the pane title switched to session text derived from the first prompt and
  stayed fixed; it does not toggle between idle and working
- a moon-phase spinner line (`🌑`–`🌘` cycling, followed by ` · ` and rotating
  tip text, e.g. `🌓 · Tip: ! to run a shell command`) rendered directly above
  the input box
- the right footer rotates hint text (`ctrl+c: cancel`, `/yolo …`) in both
  states, so footer hints are not a busy/idle discriminator

Observed completed state:

- the moon-phase spinner line disappeared; assistant output lines are prefixed
  with `● `
- the current prompt returned to the boxed `│ >` shape
- stale spinner text can remain in scrollback, so busy detection must anchor to
  the current input box region rather than historical lines

## Evidence Matrix

| Signal | Strength | Baseline use | False-positive posture |
| --- | --- | --- | --- |
| Exact `kimi` pane command | Strong | Provider identity | Exact command only; no suffix matching (`kimi` is a short generic word) |
| Metadata aliases `kimi_code`, `kimi-code`, `kimi code`, `kimi` | Strong | Provider identity when wrapper-published | Explicit tmux metadata only |
| Startup title `Kimi Code` | Supporting only | Generic display label suppression | Never establishes provider alone; post-prompt titles are arbitrary session text |
| Current boxed `│ >` input box | Strong after identity | `status.source="pane_output"` idle fallback | Requires known Kimi provider; box is model-independent |
| Moon-phase spinner line (`U+1F311`–`U+1F318` glyph plus ` · ` separator) above the current input box | Strong after identity | Busy fallback | Bounded window above the current box only; stale scrollback spinners are ignored |
| Bare moon glyph near the current box without the ` · ` separator | Ambiguous | Status withheld (`unknown`) | Could be echoed output or a future spinner restyle; never flips to idle or busy |
| Frames without a current input box (approval dialogs, alternate UIs) | Rejected | None | Status stays unknown rather than guessing; these states were not probed |
| Footer model/context lines (`K2.7 Coding …`, `context: N% (x/256k)`) | Rejected | None | Model-dependent text (`256k` varies by model); not used as anchors |
| Kernel comm `kimi-code` | Rejected for baseline | None | Never surfaces via tmux; a future process-tree enrichment could use it |

## Unprobed States

The conservative fallback leaves these `unknown` by design; probe before
encoding richer signals:

- tool-approval prompts, plan mode, and long tool executions
- non-default models (`K3`, `K2.7 Coding Highspeed`) footer shapes
- `kimi acp` and `kimi -p` non-interactive modes
- npm-only installs where the pane command could surface as `node` (would need
  process-tree or `~/.kimi-code/workspaces.json` corroboration; deep-roadmap
  enrichment only)

## Icons

- Emoji `🌙`; stock Nerd Font glyph `U+F0594` (`nf-md-weather_night`)
- Patched glyph reuses the `agent-icons-v9` manifest's existing `kimi` entry at
  `U+100057` — Kimi CLI has no distinct upstream mark, so a separate glyph
  would duplicate the brand moon. The upstream manifest gained `kimi-code` /
  `kimi_code` aliases on that entry.
- Desktop uses a monochrome light/dark SVG pair built from the same vendored
  Kimi mark.
