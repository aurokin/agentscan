# Progressively Disclosed Harness Engineering

`agentscan` should keep stable contracts and engineering harnesses in the repo
while using Linear for active milestone execution. This document explains that
documentation posture.

## Disclosure Layers

The repo should disclose information in layers:

1. `README.md`
   Operator-facing overview, current scope, key commands, and entrypoints.
2. `docs/index.md`
   Map of the documentation set and the repo's disclosure model.
3. `ROADMAP.md`
   Durable product direction, boundaries, and architectural decisions.
4. `docs/architecture.md`
   Runtime model, daemon/socket contract, command families, and design guardrails.
5. `docs/integration.md`
   Wrapper metadata contract, daemon-backed automation surfaces, shell boundary, and migration posture.
6. `docs/harness-engineering.md`
   Validation posture, harness categories, and rules for what belongs in repo docs versus Linear.
7. `docs/notes/`
   Narrow follow-up or historical notes that are too specific for the primary docs.
8. Linear
   Active milestones, blockers, sequencing, and execution detail.

When a Linear milestone completes, its stable outcomes should be folded back
into the appropriate repo docs instead of leaving the repo full of stale plan
documents.

## Harness Categories

The repo already uses multiple harnesses to validate behavior:

- fixture harnesses for tmux rows, snapshot envelopes, and normalization behavior
- unit harnesses for parser, classification, status, and output logic
- daemon integration harnesses for tmux topology changes, metadata updates, and socket publication
- TUI and focus interaction harnesses for real tmux client behavior
- snapshot validation harnesses for schema and daemon health behavior
- benchmark harnesses for hot-path regressions

These harnesses should be documented in terms of the contract they protect, not
as checklists for an active milestone.

## Tmux Server Isolation

Live tmux integration tests must run against their own isolated tmux server, not
the user's default server. Direct harness tmux commands should target the
temporary server with `tmux -S <socket>`, remove inherited `TMUX`, and keep
`TMUX_TMPDIR` inside the harness temp directory.

`agentscan` subprocesses launched by the harness must also receive
`AGENTSCAN_TMUX_SOCKET=<socket>`. When this variable is set, `agentscan` runs its
own tmux subprocesses through `tmux -S <socket>` and removes inherited `TMUX`
from those child commands. This protects destructive fixture operations such as
`kill-server`, `kill-session`, and `kill-window` from drifting onto the default
tmux server.

The integration suite should keep a guard test that poisons the default
`TMUX_TMPDIR` and asserts `agentscan scan --all --format json` still reads from
the harness server. If that test starts reading the poisoned/default server, the
suite should fail before any destructive tmux fixture can run.

## Quality Baseline

The current baseline remains:

- `cargo fmt --all --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_arguments`
- `cargo test`

Performance spot checks currently use:

- `cargo bench --bench core_paths -- --noplot`

## Documentation Rules

- Keep active task sequencing out of the repo once it moves to Linear.
- Keep stable contracts, guardrails, and operator guidance in repo docs.
- Prefer narrow follow-up notes over large catch-all plan documents.
- Promote behavior into the docs only after it is implemented or intentionally adopted as a durable boundary.
- If a document exists only to mirror active milestone status, replace it with a redirect to the deeper stable docs and Linear.

## Practical Rule Of Thumb

If a reader needs to know what the tool guarantees, the answer belongs in repo
docs. If a reader needs to know what is next, blocked, or in progress, the
answer belongs in Linear until it settles.
