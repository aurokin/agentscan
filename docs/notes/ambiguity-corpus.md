# Ambiguity Corpus

This note records the ambiguous pane evidence introduced for AUR-36. The
fixture lives in `tests/fixtures/tmux_snapshot_ambiguous.txt`; tests in
`src/app/tests.rs` lock the current unresolved behavior.

The purpose is to identify where the current metadata, command, and title
classification path runs out of trustworthy evidence. This note does not define
new fallback behavior by itself.

## Decision Matrix

| Pane | Evidence | Current Result | Decision | Rationale | Follow-up |
|------|----------|----------------|----------|-----------|-----------|
| `%600` | `zsh` command, wrapper-shaped title `(bront) ~/code/agent-wrapper`, `ai` window | provider `unknown`, status `unknown`, conservative title label | require wrapper metadata | The title looks like wrapper context, not a provider signal. Inferring a provider from path or window name would invent evidence. | AUR-37 should define wrapper-published metadata expectations. |
| `%601` | `node` command, generic `Working` title, `ai` window | provider `unknown`, status `unknown`, label `Working` | targeted `/proc` fallback | The title may be a provider status, but without a known provider it is not trustworthy. A narrow process-tree fallback could identify a child provider launched under Node. | AUR-39 can use this as a `/proc` candidate. |
| `%602` | `python3` command, generic `agent bootstrap` title, `ai` window | provider `unknown`, status `unknown`, label `agent bootstrap` | targeted `/proc` fallback | The command is a launcher/runtime, not the agent. Linux process ancestry may reveal a concrete provider after tmux evidence fails. | AUR-39 can use this as a `/proc` candidate. |
| `%603` | `zsh` command, ASCII `pi - agentscan` title | provider `unknown`, status `unknown`, label `pi - agentscan` | keep `unknown` | ASCII `pi` task titles are intentionally weak without the `pi` command, wrapper metadata, Greek pi, or a strong status glyph. | No fallback required unless stronger evidence appears. |
| `%604` | `sh` command, task-like title `review_auth_flow`, `ai` window | provider `unknown`, status `unknown`, label `review_auth_flow` | later incremental output path | The title may be user task text rather than provider identity. Resolving it would require content from the running pane, which is outside the current fallback slice. | Defer until incremental output parsing is justified. |

## Guardrails

- Metadata remains the first and strongest signal.
- Command and title classification should stay conservative.
- `/proc` fallback candidates must run only after tmux metadata, command, and
  title evidence fail.
- Broad `ps` scans, repeated `capture-pane` loops, and popup-time scraping stay
  out of bounds.
- Cases that cannot be resolved with specific evidence should remain
  `unknown`.
