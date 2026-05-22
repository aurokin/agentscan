# Factory Droid CLI Support

Status: completed for current baseline

## Goal

AUR-370 adds conservative plug-and-play support for Factory's Droid CLI. Droid
is productized enough that support should start from empirical local probing and
public documentation, then encode only low-risk live signals.

## Local Probing

Local version: `droid 0.131.0`

Probing used an isolated tmux server with a temporary `TMUX_TMPDIR` and a
single `droid` pane in `/Users/auro/code/agentscan`.

Observed idle startup:

- `pane_current_command=droid`
- `pane_title=⛬ New Session`
- `pane_tty=/dev/ttys019`
- foreground process argv: `droid`
- child process argv:
  `/Users/auro/.local/bin/droid exec --input-format stream-jsonrpc --output-format stream-jsonrpc`
- current idle output includes the Droid banner, version, model/mode row, boxed
  prompt line `│ >`, and footer text containing `? for help` plus `IDE`.

Observed busy state after a short prompt:

- tmux title stayed Droid-owned and later changed to `⛬ Basic Math Question`
- current output included `Streaming...  (Press ESC to stop)`
- current prompt line changed to `│ > Enter to steer`
- footer still contained `? for help` plus `IDE`

Observed completed state:

- title remained `⛬ Basic Math Question`
- response line used the Droid glyph prefix
- current prompt returned to boxed `│ >`
- stale `Streaming...` text remained above the current prompt, so busy
  detection must prefer the current prompt/footer region over historical lines.

## Evidence Matrix

| Signal | Strength | Baseline use | False-positive posture |
| --- | --- | --- | --- |
| Exact `droid` pane command | Strong | Provider identity | Exact command only; no suffix matching |
| Metadata aliases `droid`, `factory-droid`, `factory droid` | Strong | Provider identity when wrapper-published | Explicit tmux metadata only |
| Foreground argv basename `droid` | Strong | Process fallback identity for shell/interpreter panes | Uses existing process-tree fallback rules |
| Title prefix `⛬ ` | Supporting only | Display label after provider identity | Does not classify generic shell panes by itself |
| Current boxed `│ >` prompt plus `? for help` / `IDE` footer | Strong after identity | `status.source="pane_output"` idle fallback | Requires known Droid provider and current footer context |
| Current `Streaming... (Press ESC to stop)` or `│ > Enter to steer` prompt | Strong after identity | Busy fallback | Scoped to known Droid panes and current prompt/footer region |
| Stale `Streaming...` above a returned idle prompt | Rejected | None | Current idle prompt wins over historical busy text |
| Generic `Droid` / `Factory` text | Rejected | None | Never establishes provider alone |

## Shipped Behavior

- `Provider::Droid` serializes as `droid`.
- `droid` is an exact command alias with high confidence.
- `factory-droid` and `factory droid` are metadata aliases.
- `⛬ <title>` is normalized as a display label only after provider identity is
  established by command, metadata, or process fallback.
- Droid pane-output status fallback is provider-scoped and current-footer
  anchored. It reports `idle` from the boxed prompt and `busy` from the current
  streaming/steering prompt.

## Boundaries

Do not read Factory/Droid local session stores, logs, or daemon state files for
baseline detection. Droid plugins, MCP configuration, and daemon integration are
not required for discovery.
