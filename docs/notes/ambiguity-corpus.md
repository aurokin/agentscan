# Ambiguity Corpus

This note records the ambiguous pane evidence introduced for AUR-36. The
fixture lives in `tests/fixtures/tmux_snapshot_ambiguous.txt`; tests in
`src/app/tests.rs` lock both the tmux-only unresolved behavior and the targeted
fallback behavior for AUR-39.

The purpose is to identify where the current metadata, command, and title
classification path runs out of trustworthy evidence. Snapshot construction may
now use the targeted Linux `/proc` fallback for the explicit runtime-launcher
cases below after the tmux-only path fails. Wrapper metadata expectations for
cases like `%600` are defined in `docs/integration.md`.

## Decision Matrix

| Pane | Evidence | Current Result | Decision | Rationale | Current Handling |
|------|----------|----------------|----------|-----------|------------------|
| `%600` | `zsh` command, wrapper-shaped title `(bront) ~/code/agent-wrapper`, `ai` window | provider `unknown`, status `unknown`, conservative title label | require wrapper metadata | The title looks like wrapper context, not a provider signal. Inferring a provider from path or window name would invent evidence. | Wrapper metadata expectations are documented in `docs/integration.md`. |
| `%601` | `node` command, generic `Working` title, `ai` window | provider `unknown`, status `unknown`, label `Working` | targeted `/proc` fallback | The title may be a provider status, but without a known provider it is not trustworthy. A narrow process-tree fallback can identify a child provider launched under Node. | Targeted `/proc` fallback is implemented for this launcher class. |
| `%602` | `python3` command, generic `agent bootstrap` title, `ai` window | provider `unknown`, status `unknown`, label `agent bootstrap` | targeted `/proc` fallback | The command is a launcher/runtime, not the agent. Linux process ancestry can reveal a concrete provider after tmux evidence fails. | Targeted `/proc` fallback is implemented for this launcher class. |
| `%603` | `zsh` command, ASCII `pi - agentscan` title | provider `unknown`, status `unknown`, label `pi - agentscan` | keep `unknown` | ASCII `pi` task titles are intentionally weak without the `pi` command, wrapper metadata, Greek pi, or a strong status glyph. | No fallback is required unless stronger evidence appears. |
| `%604` | `sh` command, task-like title `review_auth_flow`, `ai` window | provider `unknown`, status `unknown`, label `review_auth_flow` | later incremental output path | The title may be user task text rather than provider identity. Resolving it would require content from the running pane, which is outside the current fallback slice. | Deferred until incremental output parsing is justified. |

## Guardrails

- Metadata remains the first and strongest signal.
- Command and title classification should stay conservative.
- `/proc` fallback candidates must run only after tmux metadata, command, and
  title evidence fail.
- Broad `ps` scans, repeated `capture-pane` loops, and popup-time scraping stay
  out of bounds.
- Cases that cannot be resolved with specific evidence should remain
  `unknown`.
- AUR-39 fallback is scoped to unresolved `node` and `python3` launcher panes
  and records `proc_process_tree` provenance when it finds a known descendant
  provider command.
