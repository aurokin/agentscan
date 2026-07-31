# Amp Support

Status: baseline support verified with an isolated real-agent lifecycle E2E

## Local Probing

Probed on macOS across three same-day native Amp builds. The user-provided pane
started as `0.0.1785184098-gdd0f3f`; an earlier isolated E2E ran
`0.0.1785488326-gdfd462`; and the final lifecycle/corpus capture ran
`0.0.1785515475-g65101d`. The executable was `~/.amp/bin/amp`; both the
foreground command and `pane_current_command` were exactly `amp`. Amp published
no tmux metadata. Because the unrelated Amp terminal editor uses the same
executable name, the bare command is treated as ambiguous and resolved from
canonical executable provenance.

The initial authenticated screen probe used the user-provided pane
`agentscan:6.1`. After command-collision hardening, isolated harness run
`1785517267_58540` reached `detected -> ready -> busy -> ready`. Identity was
high-confidence `proc_process_tree` evidence from
`/Users/auro/.amp/bin/amp`; both terminal states came from `pane_output`.
Startup-only run `1785517513_64250` also passed through an isolated official
`@ampcode/cli` npm install. tmux reported `amp.exe`, while classification used
the canonical `@ampcode/cli/bin/amp.exe` path as high-confidence process
evidence.

Startup-only run `1785517871_80628` used a custom `AMP_HOME` whose `/tmp`
spelling appears as `/private/tmp` in macOS process evidence; it resolved
through the canonical foreground executable and reached pane-output idle. An
earlier exploratory run (`1785517621_27232`) staged the native binary under a
temporary `Cellar/ampcode`-shaped path. The final collision-hardened matcher
intentionally rejects that nonstandard prefix; standard macOS, Intel macOS,
and Linuxbrew prefixes have source and unit coverage but were not live-installed.

Observed startup and idle state:

- the fresh title was empty or generic
- after a turn, the title became `<thread label> - amp - <cwd>`
- the persistent composer top border ended in a built-in mode: `low`,
  `medium`, `high`, or `ultra`; its body rows had the shape `│...│`
- the idle bottom border began directly with `╰──` and ended with the cwd

Observed busy state:

- a braille spinner prefixed the title while Amp was working
- the current composer bottom began with `╰ ≈ Connecting`, `╰ ≋ Sending`, or
  `╰ ∼ Streaming [N tok]`; animation could transiently omit letters from the
  label

Observed completion:

- the current composer returned to the idle `╰──` bottom shape
- completed busy composers can remain in scrollback, so the composer nearest
  the pane tail must win

## Evidence Matrix

| Signal | Strength | Baseline use | False-positive posture |
| --- | --- | --- | --- |
| Exact `amp` pane command | Ambiguous | Trigger targeted foreground/process-tree inspection | Shared with the maintained Amp terminal editor; never identity by itself |
| Metadata aliases `amp`, `amp-code`, `amp code`, `ampcode` | Strong | Provider identity when wrapper-published | Explicit tmux metadata only; Amp did not publish it during probing |
| Canonical executable under process-home `~/.amp/bin`, inherited `$AMP_HOME/bin`, a standard Homebrew prefix, or npm `@ampcode/cli` packages | Strong | Targeted process-tree identity | Allow-lists official installation layouts; arbitrary relocated `amp` binaries remain unknown |
| Empty or generic fresh title | Rejected | None | Contains no provider identity |
| `<thread label> - amp - <cwd>` title | Supporting only after identity | Normalize the thread label for display | Thread text is user-controlled; exact title text never establishes identity |
| Current composer with a recognized mode top, `│...│` body, and `╰──` bottom | Strong after identity | `status.source="pane_output"` idle fallback | Requires known Amp identity and a complete composer near the pane tail |
| Current composer bottom starting `╰ ≈`, `╰ ≋`, or `╰ ∼` | Strong after identity | Busy fallback | Uses observed activity glyphs rather than exact animated label text |
| Braille title spinner | Rejected for status | None | Title animation is supporting observation only; the current composer is the status anchor |
| Stale completed composers in scrollback | Rejected | None | The current composer nearest the tail wins |
| Unrecognized mode, activity glyph, or incomplete composer | Ambiguous | Status withheld (`unknown`) | A restyle must go quiet, never invert busy and idle |

## Unprobed States

Custom approval and policy dialogs remain `unknown`. Amp's official
documentation says tools do not ask approval by default, but those alternate
screens were not observed and are not inferred from that policy. Non-default
launch modes and future composer modes or activity glyphs also remain unknown
until probed. A real Homebrew package installation and native Windows startup
were not run; their official layouts are source- and unit-validated. Symlinked
`HOME` or `AMP_HOME` roots beyond the stable macOS `/tmp`, `/var`, and `/etc`
aliases stay unknown so scans never traverse process-controlled filesystems;
roots containing `..` stay unknown for the same reason.

## Icons

- Emoji: `⚡` (`U+26A1`)
- Stock Nerd Font: `nf-fa-bolt` (`U+F0E7`)
- Patched Nerd Font: the existing `agent-icons-v9` Amp mark at `U+100046`; no
  font-patcher manifest change is needed
- Desktop: the official red full-color Amp mark, shared across light and dark
  themes; upstream source: <https://ampcode.com/amp-mark-color.svg>
