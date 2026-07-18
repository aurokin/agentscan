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

Active issue status and final human approval live in Linear.

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
- `AUR-180` finalizes current-state docs and release notes.
- `AUR-181` is the human review gate for the breaking surface and rollout
  posture.

## Closure Rules

- Do not push the milestone branch until AUR-180 docs, AUR-181 human review,
  and the final quality baseline are complete.
- Keep future execution sequencing in Linear.
- Promote only durable behavior and contracts into repo docs.
