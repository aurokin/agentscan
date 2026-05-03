# Agentscan Daemon Socket Migration Plan

## Source Context

- Linear project: `Agentscan`
- Linear milestone: `Daemon Socket Migration`
- Repo boundary at plan time: `AUR-170` and `AUR-171` are done; current `master` ends at socket path resolution and strict IPC hello/frame validation.
- Current code boundary: `src/app/ipc/mod.rs` defines socket path resolution, frame bounds, hello validation, and protocol/schema mismatch shutdown frames. `daemon run`, one-shot commands, daemon status, TUI bootstrap, integration tests, and cache diagnostics still use the cache-backed transport.
- Relevant current modules:
  - `src/app/daemon.rs`: tmux control-mode daemon loop, currently writes snapshots to the cache.
  - `src/app/commands.rs`: command routing, currently reads snapshots through `cache::load_snapshot`.
  - `src/app/cache.rs`: cache transport plus snapshot helper behavior that will later need to move or be deleted.
  - `src/app/tui/*`: renamed TUI, still cache-backed during migration.
  - `tests/common/tmux_harness.rs` and cache integration tests: still depend on `AGENTSCAN_CACHE_PATH`.

## Execution Order

1. `AUR-172` - Serve one-shot snapshots over the daemon socket.
   - Build the daemon-side snapshot socket server for snapshot-mode clients.
   - Extend the daemon-to-client frame contract for snapshot delivery and precise unavailable/shutdown states before clients depend on it.
   - Keep one-shot clients short-lived and outside subscriber state.
   - Add readiness, startup-failed, closing-state, and frame-size behavior.
   - Add a minimal socket test fixture for synthetic clients so later daemon issues reuse the same protocol harness.
   - Leave singleton lock/log identity, stale socket cleanup, daemonized start, and stop ownership to `AUR-174`; `AUR-172` is a foreground `daemon run` socket-serving slice.

2. `AUR-173` - Add daemon subscription fan-out with bounded subscriber cleanup.
   - Add subscribe-mode registration after current snapshot availability and capacity checks.
   - Publish bootstrap plus latest-wins updates without blocking tmux control mode.
   - Add subscriber/pending-handshake limits and dead-client cleanup.
   - Define bounded mailbox sizes, short write behavior, slow-client retirement, and snapshot clone/drop behavior in the issue plan before implementation.

3. `AUR-174` - Add daemon lifecycle commands.
   - Add `daemon start`, socket-backed `daemon status`, safe `daemon stop`, and `daemon restart`.
   - Introduce sidecar lock/log identity around the resolved socket path.
   - Keep `daemon run` as the foreground entrypoint.
   - Cover stale/orphan socket cases: connect refused, wrong protocol/process, dead PID, shutting down daemon, tmux unavailable, and different resolved socket paths.
   - Define status text and JSON fields for socket path, pid/identity, protocol/schema, tmux state, subscriber count, last snapshot, last error, and shutdown state.
   - Define log sidecar path, startup truncation policy, and required startup/stop failure logging.

4. `AUR-175` - Add auto-start and daemon opt-out flow.
   - Add one shared connect/start/retry helper.
   - Add `--no-auto-start` and `AGENTSCAN_NO_AUTO_START=1` to daemon-backed consumers.
   - Preserve precise startup outcomes.
   - Treat opt-out with no daemon as a clear failure, not a direct tmux fallback; direct tmux bypass remains `--refresh` or `scan`.
   - Define connect timeout, startup readiness timeout, retry cadence, and user-visible failure messages in the issue plan.

5. `AUR-176` - Migrate one-shot commands to socket-backed snapshots.
   - Move bare `agentscan`, `list`, `inspect`, `focus`, and new `snapshot` to daemon snapshots by default.
   - Keep `--refresh` direct tmux bypass for supported one-shot commands.
   - Keep `scan` daemon-free and fresh.
   - Define `snapshot` as the raw `SnapshotEnvelope` surface with text and JSON output, `--refresh`, and `--no-auto-start`; it should not need `--all` because it is unfiltered envelope output.
   - Validate `focus` against daemon state by default before attempting tmux focus; if daemon state says present but tmux reports missing, surface the tmux missing-pane error instead of silently succeeding.
   - Decide whether `tmux set-metadata` / `clear-metadata` rely on daemon control-mode events or need an explicit daemon nudge during the socket migration.

6. `AUR-177` - Move TUI to live socket subscription.
   - Replace cache polling/bootstrap with subscribe-mode client events.
   - Keep input and subscription reads feeding one TUI event loop.
   - Show connecting/offline/shutdown states without falling back to tmux scans.

7. `AUR-178` - Rewrite cache-dependent integration harnesses.
   - Convert command and daemon-backed tests to socket or daemon fixtures.
   - Keep real tmux subprocess coverage only where end-to-end behavior matters.
   - Remove cache-write dependence before cache transport deletion.

8. `AUR-179` - Remove cache transport and obsolete cache surfaces.
   - Remove cache IPC writes, `agentscan cache`, cache freshness diagnostics, and `AGENTSCAN_CACHE_PATH`.
   - Move still-useful snapshot validation/filtering/sorting helpers out of cache ownership.
   - Confirm removed commands and env vars fail intentionally.

9. `AUR-181` - Human review: confirm breaking surface and rollout posture.
   - Confirm the intentional command/API breaks and direct tmux bypass boundaries.
   - Confirm provider-roadmap/support issues can resume only after review.
   - Because Linear places this after `AUR-179`, treat it as a final acceptance gate for the already-implemented breaking surface before docs finalization and push.

10. `AUR-180` - Finalize daemon socket docs and release notes.
    - Reconcile README, docs index, architecture, integration, roadmap, and release notes with shipped socket behavior.
    - Preserve only tmux `display-popup` references, not the removed app command name.
    - Run and record the full quality baseline.

## Per-Issue Workflow

- Before each issue, replace `ISSUE_PLAN.md` with that issue's plan and commit it with the issue's work.
- Each `ISSUE_PLAN.md` must include scope, non-goals, implementation outline, edge cases, test plan, documentation impact, and plan-review notes.
- Move the Linear issue from `Todo` to `In Progress` when implementation begins, then to `In Review` when local implementation is ready for code review, then to `Done` after review and gates pass.
- Add Linear comments when behavior, scope, risk, or verification results are important for future context.
- Use fresh subagents for plan review before implementation and for code review after implementation; close inactive subagents before opening new ones.
- Commit after milestone planning, after each completed issue, and after any review-driven follow-up commit.
- Do not push until `AUR-180`, final documentation refactor, final gates, and the milestone completion audit are done.

## Verification Baseline

For each issue, run the narrowest meaningful focused tests first, then expand based on blast radius. Before closing the milestone, run:

- `cargo fmt --all --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_arguments`
- `cargo test`

## Documentation Endgame

After all milestone issues are complete, perform a documentation refactor using progressive disclosure and harness engineering principles:

- Keep top-level docs short and route readers to task-specific detail.
- Promote durable socket, lifecycle, snapshot, test harness, and migration decisions from Linear/comments into repo docs.
- Add or update ADR-style records only for decisions an agent needs before modifying daemon socket, lifecycle, command surface, cache removal, or harness code.
- Remove stale migration scaffolding once the current-state docs own the behavior.
