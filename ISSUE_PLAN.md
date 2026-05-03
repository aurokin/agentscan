# AUR-180 Issue Plan: Finalize Daemon Socket Docs And Release Notes

## Goal

Reconcile the durable repo documentation with the completed daemon socket
migration. The docs should describe shipped behavior, not in-flight migration
state: daemon-backed consumers use the Unix socket, direct tmux snapshots are
explicit recovery paths, the cache command/file transport is gone, and
`agentscan tui` is the interactive surface.

## Scope

This issue owns:

- `AGENTS.md`
- `README.md`
- `docs/index.md`
- `docs/architecture.md`
- `docs/integration.md`
- `ROADMAP.md`
- release notes for the daemon socket migration
- stale migration-plan language that would mislead future agents after the
  migration is complete

This issue does not own:

- daemon socket protocol changes
- snapshot schema changes
- command behavior changes
- resurrecting compatibility aliases or removed cache commands
- host dotfiles migration

## Documentation Contract To Preserve

- Normal `agentscan`, `list`, `inspect`, `focus`, `snapshot`, and `tui` flows
  are daemon-backed and use socket snapshots.
- Normal consumers auto-start the daemon unless `--no-auto-start` or
  `AGENTSCAN_NO_AUTO_START=1` opts out.
- `agentscan scan` and refresh-capable `--refresh` flows are direct tmux
  recovery paths and do not require or start the daemon.
- `agentscan snapshot --format json` is the raw snapshot envelope surface.
- `agentscan list --format json` is the supported normal automation surface.
- `agentscan tui` is interactive-only, socket-subscription-backed, and has no
  cache or direct tmux fallback.
- `agentscan popup` is removed with no compatibility alias.
- The `agentscan cache` command family and persisted cache file transport are
  removed. Any remaining `diagnostics.cache_origin` vocabulary is snapshot
  schema compatibility, not an active cache transport.
- Shell wrappers stay thin and may publish explicit tmux metadata; provider
  hooks/extensions remain future enrichment, not baseline requirements.
- Tmux `display-popup` remains only as a tmux invocation mechanism, not as the
  removed `agentscan popup` command.

## Steps

1. Audit durable docs for stale migration language.
   - Check `AGENTS.md`, `README.md`, `ROADMAP.md`, `docs/index.md`,
     `docs/architecture.md`, `docs/integration.md`,
     `docs/harness-engineering.md`, `docs/notes/interactive-tui.md`, and
     `MILESTONE_PLAN.md`.
   - Distinguish historical planning references from active user/operator
     contracts.
   - Classify each legacy-wording hit as allowed historical context,
     removed-command warning, tmux `display-popup` usage, schema compatibility,
     or must-rewrite current-state wording.

2. Convert primary docs from target/migration phrasing to shipped-state
   phrasing.
   - Make `README.md` and `docs/architecture.md` say the socket-backed daemon
     model is current behavior.
   - Keep Linear as the place for active sequencing, but remove wording that
     implies this migration is still underway.
   - Keep command lists consistent across docs.

3. Add release notes.
   - Add `CHANGELOG.md` with an `Unreleased` section because the repo has no
     existing release-note convention.
   - List the breaking surfaces precisely: human picker launch paths move from
     `popup` to `tui`; automation migrates to `list --format json`; raw
     envelope consumers use `snapshot --format json`; cache commands and cache
     file IPC are removed; normal consumers are daemon-backed by default; the
     TUI is socket-only; direct tmux recovery is through `scan`/`--refresh`.
   - Note the operator migration targets for machine-readable consumers.

4. Reconcile roadmap and milestone artifacts.
   - Update `ROADMAP.md` so the daemon socket migration reads as delivered
     current posture, not adopted-next architecture.
   - Convert `MILESTONE_PLAN.md` into a completed historical artifact with a
     short current-state pointer, or replace its stale pre-migration context
     with an explicit completed-milestone note.

5. Audit vocabulary after edits.
   - `popup` should only appear when describing the removed command or tmux
     `display-popup`.
   - `cache` should only appear when describing removed surfaces, compatibility
     schema vocabulary, harness guards, or historical context.
   - Docs should not tell users or agents to use `agentscan cache`,
     `AGENTSCAN_CACHE_PATH`, or cache files as an IPC boundary.

## Risks

- Overcorrecting docs could erase useful migration guidance for downstream
  scripts. Mitigation: keep explicit migration target tables and release notes.
- Release notes may imply a published package version that does not exist.
  Mitigation: use an "Unreleased" section unless the repo already has a release
  convention.
- Broad cache-word removal could hide the intentional
  `diagnostics.cache_origin` compatibility field. Mitigation: document it as
  schema compatibility where relevant instead of pretending the field vanished.

## Verification

- `cargo fmt --all --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_arguments`
- `cargo test`
- targeted docs vocabulary audit with `rg` for:
  - `agentscan cache`
  - `AGENTSCAN_CACHE_PATH`
  - `cache file`
  - `cache transport`
  - `cached`
  - `cache-backed`
  - `cache polling`
  - `cache-v1.json`
  - `persisted cache JSON`
  - `agentscan popup`
  - `moving to`
  - `target architecture`
  - `adopted next architecture`
  - `intended steady-state`
  - `target transport`

## Plan Review Notes

Plan review subagent: Boole the 16th.

- Added `AGENTS.md` to scope because it is agent-facing instruction surface and
  still had stale cached-flow wording at review time.
- Expanded the vocabulary audit beyond exact cache command strings so
  `cached`, `cache-backed`, `cache-v1.json`, and future/target phrasing cannot
  slip through.
- Made the release-note target explicit as `CHANGELOG.md` with `Unreleased`.
- Split release-note migration wording so `tui` is the human picker target,
  `list --format json` is the normal automation target, and `snapshot --format
  json` is only the raw envelope target.
- Tightened `MILESTONE_PLAN.md` handling so it becomes clearly historical or
  points readers to current-state docs.
