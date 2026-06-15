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
- frontend vitest harnesses for the desktop app: node-environment view-model and
  transcript tests (Effect services against a mock Tauri IPC layer, queued-op
  bodies against recorded fake sinks), plus per-window jsdom mount smoke tests
  for the dock and settings windows (node is the vitest default; the mount
  suites opt into jsdom per file via `// @vitest-environment jsdom`)
- Rust unit harnesses inside the standalone desktop crate
  (`desktop/src-tauri`), which the root cargo commands never touch

These harnesses should be documented in terms of the contract they protect, not
as checklists for an active milestone.

The tmux pane fixtures (`tests/fixtures/tmux_snapshot_*.txt`) encode a fixed
`\037`-delimited field layout. The two trailing fields are the active flags
(`#{pane_active}`, `#{window_active}`), so every fixture line and every
hand-built `TmuxPaneRow` literal carries them; the parser accepts field counts
{12, 14, 17, 19} (core + active, optionally plus session/window ids and the
`@agent` block). When the snapshot envelope shape changes, bump
`CACHE_SCHEMA_VERSION` and the matching fixtures/wire literals together.

## Subprocess Boundary

Production subprocesses should be limited to explicit product boundaries and
lifecycle work:

- tmux commands and the daemon's long-lived tmux control-mode client
- detached `agentscan daemon run` when daemon lifecycle policy allows it
- macOS `codesign` checks for explicit detached daemon start preflight

Process-inspection fallback should remain in-process through platform APIs:
procfs on Linux and `libproc` / `sysctl` on macOS. Tests may spawn helper
processes to build fixtures or validate lifecycle behavior, but those helpers
should not leak into production scanning paths.

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

Harness commands that exercise implicit daemon auto-start on macOS must use a
signed binary. Use foreground `agentscan daemon run`, direct `agentscan scan`,
or refresh-capable command flags for local ad-hoc macOS harness work. Any
detached daemon start on macOS requires a signed binary.

The integration suite should keep a guard test that poisons the default
`TMUX_TMPDIR` and asserts `agentscan scan --all --format json` still reads from
the harness server. If that test starts reading the poisoned/default server, the
suite should fail before any destructive tmux fixture can run.

## Socket-First Daemon Harnesses

Daemon integration tests treat the daemon socket as the transport for daemon
state. Tests that assert daemon readiness, live pane state, topology changes,
one-shot daemon routing, or TUI subscription success should use the socket
snapshot helpers.

Socket helpers must:

- connect to the harness `AGENTSCAN_SOCKET_PATH` with bounded IO
- send `hello` frames using the crate's shared protocol and snapshot schema
  versions
- reject incompatible `hello_ack` responses
- validate returned snapshots through the same typed snapshot validation used by
  the crate
- include the last socket error, last snapshot summary, and daemon logs in
  timeout diagnostics

Fake daemon socket fixtures must bind the harness socket, use bounded accepts and
read/write timeouts, parse and assert the incoming request, serve typed-valid
snapshot frames, and fail on unexpected connection patterns when the test owns a
single-request fixture.

The cache file transport is removed. Integration harnesses still sandbox
`XDG_CACHE_HOME` and `HOME` to a temp directory and fail if the old
`agentscan/cache-v1.json` path is created, so any accidental fallback write is
caught without touching a user cache directory.

## Quality Baseline

The current baseline remains:

- `cargo fmt --all --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_arguments`
- `cargo test`

Desktop changes carry their own half of the CI baseline. The same four cargo
commands run inside `desktop/src-tauri` (a standalone crate the root commands
never touch), plus, in `desktop/`:

- `pnpm build` (the Tauri crate embeds `../dist` via `tauri::generate_context!`,
  so the frontend must build before the crate compiles)
- `pnpm test` (the vitest suite)

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
