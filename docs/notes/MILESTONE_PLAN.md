# Agentscan Daemon Socket Migration Record

This file is a historical milestone record, not the current architecture
reference. Current daemon, socket, command, integration, and harness contracts
live in:

- `README.md`
- `CHANGELOG.md`
- `ROADMAP.md`
- `docs/architecture.md`
- `docs/integration.md`
- `docs/harness-engineering.md`

The milestone is complete; issue keys below are history from the retired
tracker. The original PRD is condensed under "Original PRD" at the end.

## Delivered Scope

The daemon socket migration delivered these durable changes:

- one-shot daemon socket snapshots for normal consumers
- live daemon socket subscriptions for the TUI
- daemon lifecycle commands: `start`, `run`, `status`, `stop`, and `restart`
- automatic daemon startup for normal consumers, with `--no-auto-start` and
  `AGENTSCAN_NO_AUTO_START=1` opt-outs
- daemon-backed `agentscan`, `list`, `inspect`, `focus`, `snapshot`, and `tui`
  flows
- direct tmux recovery through `agentscan scan` and supported `--refresh` flags
- socket-first integration harnesses with isolated tmux servers
- removal of the cache command family and cache file IPC transport
- `agentscan tui` as the interactive command, with no `agentscan popup`
  compatibility alias

## Issue Sequence

- `AUR-172` served one-shot snapshots over the daemon socket.
- `AUR-173` added daemon subscription fan-out with bounded subscriber cleanup.
- `AUR-174` added daemon lifecycle commands.
- `AUR-175` added automatic daemon startup and opt-out behavior.
- `AUR-176` migrated one-shot commands to socket-backed snapshots.
- `AUR-177` moved the TUI to a live socket subscription.
- `AUR-178` rewrote daemon integration harnesses around socket fixtures.
- `AUR-179` removed obsolete cache transport surfaces.
- `AUR-180` finalized current-state docs and release notes.
- `AUR-181` was the human review gate for the breaking surface and rollout
  posture (see `docs/notes/ISSUE_PLAN.md`).

## Closure Rules (as applied)

- The milestone branch was not pushed until AUR-180 docs, AUR-181 human review,
  and the final quality baseline were complete.
- Provider-roadmap and support work stayed blocked until the AUR-181 review.
- Only durable behavior and contracts were promoted into repo docs; future
  sequencing goes in GitHub issues.

## Original PRD (condensed)

### Problem

A persisted cache file served as both storage and IPC between the daemon,
one-shot commands, and the interactive picker. Interactive updates were
polling-based, daemon state was ambiguous, and commands reasoned about cache
freshness instead of live scanner health. Users needed current pane state
without manual service setup, hooks, wrappers, or shell integration;
automation needed stable JSON and a daemon-free recovery path; maintainers
needed reviewable slices without keeping temporary cache fallbacks as
permanent surface.

### Solution

A daemon-required Unix-domain socket model. The daemon is the single live state
owner. Clients speak a byte-bounded JSON-Lines protocol, negotiate frame and
snapshot schema compatibility, and receive one snapshot or a live
subscription. Normal consumers auto-start the daemon unless opted out; one-shot
commands keep a direct tmux `--refresh` bypass. The interactive command became
`agentscan tui` (tmux popup is only one possible host). The cache file and
cache diagnostics were removed once every consumer and test was socket-backed.

### Technical decisions

- One socket frame protocol version for the wire envelope and a separate
  snapshot schema version for `SnapshotEnvelope` compatibility.
- Two client modes: `snapshot` (one-shot) and `subscribe` (TUI).
- Compatible clients get `hello_ack`; protocol or schema mismatches get an
  explicit shutdown with no acknowledgment.
- `daemon_not_ready` only during the bounded initial-snapshot window;
  `startup_failed` is terminal for the current daemon process.
- One-shot clients are never registered as subscribers. Subscribers register
  only after a current snapshot exists and capacity is available.
- Subscriber fan-out is latest-wins and never blocks the tmux control-mode
  loop. Paired EOF and protocol monitoring retires dead clients even when tmux
  is quiet.
- Client hello frames and daemon-to-client frames are byte-bounded. An
  oversized initial snapshot fails startup; an oversized later snapshot keeps
  the last good snapshot and logs the skipped publish.
- Socket path resolution enforces per-user private runtime directories and
  path-length limits; lock and log sidecars derive from the socket path; the
  file lock is the single-instance authority.
- `stop` sends SIGTERM with best-effort PID validation and escalates to SIGKILL
  only after a fresh compatible identity handshake.

### Testing strategy

Frame parsing and strict validation in isolation (unknown fields, malformed
versions, unknown modes, byte limits, truncated frames); socket path resolution
(override, runtime dir, macOS and Unix fallbacks, permissions, ownership,
length); synthetic socket clients for snapshot, subscribe, not-ready, startup
failure, subscriber limit, EOF cleanup, and post-handshake violations;
subprocess lifecycle tests (start readiness, status states, idempotent stop,
stale socket cleanup, non-socket refusal, identity mismatch, incompatible
daemon and startup-failure guidance); TUI transitions with injected
subscription sources plus real tmux end-to-end tests. Cache-fixture tests were
retired before the cache transport was removed.

### Out of scope

Delta or per-pane frames, idle daemon shutdown, peer-credential checks,
Windows, log rotation beyond startup truncation, `daemon stop --force`,
subscriber introspection, telemetry, hooks or wrapper metadata as baseline
prerequisites, compatibility aliases for removed commands, and keeping the
cache file as a supported transport.

### Slice order

AUR-170 (rename `popup` to `tui`) and AUR-171 (socket path and strict
handshake) first; then snapshots (172), fan-out (173), lifecycle commands
(174), auto-start and opt-out (175), one-shot migration (176), TUI subscription
(177), harness rewrite (178), cache removal (179), the human review gate (181),
and final docs (180).
