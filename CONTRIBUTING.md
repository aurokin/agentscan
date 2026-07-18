# Contributing

- Start at `docs/index.md` — the docs are progressively disclosed and it maps
  where everything lives.
- The most common contribution is adding or fixing a provider; follow
  `docs/adding-a-provider.md` end to end. Evidence first, matchers that
  degrade to `unknown`, never silent busy/idle flips.
- Quality baseline (must pass before review): `cargo fmt --all --check`,
  `cargo clippy --all-targets --all-features -- -D warnings` (plus the
  complexity variant with `-W clippy::cognitive_complexity -W clippy::too_many_arguments`),
  and `cargo test`. Desktop changes additionally need `pnpm build` and
  `pnpm test` in `desktop/` (pnpm only, never npm).
- Tests that spawn `tmux` or `agentscan` must use the isolated harness socket
  and temp `TMUX_TMPDIR` — see `docs/harness-engineering.md`.
- Record user-facing changes under `## Unreleased` in `CHANGELOG.md`.
