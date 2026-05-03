# agentscan Roadmap

## Project Goal

Replace the current shell-heavy tmux agent discovery stack with a fast Rust
scanner and indexer that can power aliases, interactive pickers, and later other tools
without rescanning tmux on every interaction.

## Product Boundary

This repository is the product source of truth.

It owns:

- the Rust scanner, daemon, socket transport, and structured snapshot model
- the user-facing `agentscan` CLI
- tmux-facing commands and minimal integration guidance
- the documentation for supported contracts and workflows

It does not require immediate migration of host-specific dotfiles. Those remain
reference environment material until the Rust-native workflow is mature enough
to replace them intentionally.

## Non-Goals

These stay outside the core scanner unless explicitly justified later:

- replacing shell aliases in user dotfiles immediately
- replacing tmux key bindings in user dotfiles immediately
- owning provider launch semantics such as `resume`
- turning the scanner into a generic tmux session manager
- adding a permanent fast versus full scan split

## Documentation Posture

This file records durable direction and architectural decisions.

Active milestone sequencing, blockers, and execution detail live in Linear until
they settle. Stable implementation detail should be promoted back into:

- `docs/index.md`
- `docs/architecture.md`
- `docs/integration.md`
- `docs/harness-engineering.md`

## Durable Decisions

### Implementation Language

Use Rust for the core scanner and indexer.

Reasons:

- typed data model
- straightforward testing around parsing and classification
- good fit for a long-lived daemon
- reduced shell process churn in the hot path

### Primary Identity

Use `pane_id` as the canonical runtime key for pane state.

Implications:

- in-memory state is keyed by `pane_id`
- snapshot records are keyed by `pane_id`
- inspect and focus workflows target `pane_id`

### Steady-State Architecture

The intended mature architecture is a daemon-required runtime with short-lived
socket clients.

Implications:

- the daemon is the single source of live pane state
- consumers connect to the daemon over a Unix socket and read
  `SnapshotEnvelope` frames
- daemon startup is automatic for normal desktop commands
- direct tmux snapshots remain available for debugging and recovery through
  `agentscan scan` and refresh-capable command flags
- `AGENTSCAN_NO_AUTO_START=1` and `--no-auto-start` exist for CI and scripts
  that must not leave a daemon running
- when tmux disappears, the daemon reports failure through lifecycle/status
  paths; restart policy remains explicit user or supervisor policy

### Snapshot Contract

Use a versioned JSON `SnapshotEnvelope` as the canonical structured state
contract. The transport is the daemon socket, not a canonical cache file.

Implications:

- the snapshot envelope shape is an API contract
- schema versioning must be explicit
- the daemon socket uses a separate wire protocol version; socket clients start
  with a strict hello frame that declares the wire version, snapshot schema
  version, and requested mode
- compatible socket clients receive `hello_ack`; protocol or schema mismatches
  receive an explicit shutdown frame without acknowledgment
- one-shot commands read a full snapshot frame and disconnect
- TSV is an output adapter only, not the canonical store
- persisted cache JSON is a migration artifact, not the target IPC boundary

### Detection Policy

The default detection path is:

1. explicit wrapper-published tmux metadata
2. tmux pane metadata and terminal titles
3. targeted process-tree fallback for concrete ambiguous panes
4. tightly scoped provider-specific pane output parsing for status only, after
   provider identity has already been established

Implications:

- prefer tmux metadata and control-mode events over process scans
- keep labels conservative when evidence is weak
- treat pane inspection as fallback rather than the normal path
- pane output is not a provider-identity signal. When used, it must be
  provider-scoped, anchored to current prompt/footer/status shapes, and reported
  through `status.source="pane_output"`.
- process fallback is targeted live process inspection, not broad system
  scanning. It is limited to concrete ambiguous panes, checks the foreground
  process group for shell or wrapper panes, and checks root/descendant process
  command, argv, and selected environment markers for known launcher panes.
- provider logs, transcript files, session databases, and other historical
  state stores are not core detection inputs. They may be useful during
  research, but baseline detection must rely on live tmux, process, title, and
  tightly scoped pane evidence.
- `inspect` reports provider source, status source, classification reasons, and
  targeted process fallback outcomes so classification problems can be debugged
  from the CLI and snapshot JSON without reading implementation code
- provider-side hooks and extensions are deep-roadmap enrichment only. They may
  eventually publish better labels, session ids, or direct activity state, but
  they sit behind source analysis, local probing, and plug-and-play detection
  hardening. The core scanner must remain plug-and-play for common agent
  launches.

### TUI Contract

The interactive pane picker is a TUI, not an automation surface. The
user-facing command name is `agentscan tui`; `agentscan popup` is removed, not
aliased.

Implications:

- the TUI does not support `--format`
- terminal rendering is not a stable machine-readable contract
- automation consumers should use `agentscan list --format json` for normal pane data
- raw snapshot-envelope consumers should use `agentscan snapshot --format json`
- compatibility formatting paths must not be added back to the TUI

### Integration Boundary

Keep shell as the integration layer and Rust as the discovery and state layer.

Implications:

- shell may launch panes and bind keys
- shell may keep aliases, provider wrappers, and TUI entrypoints
- shell should not classify panes or infer activity state
- shell should not shape machine-readable pane output
- wrapper behavior is integration context, not a reason to move launch logic into Rust

### Platform Priority

Linux and macOS are the primary targets for early fallback logic.

Implications:

- Linux fallback may read selected `/proc` argv/env fields for unresolved
  launcher panes.
- macOS fallback should stay targeted to descendant processes of unresolved
  launcher panes rather than broad `ps` scans.

## Reference Baseline

The current shell stack in `~/.dotfiles` is useful as reference material, not as
the specification for `agentscan`.

It is helpful for understanding:

- which panes users currently expect to see
- which providers and wrappers show up in practice
- which display labels and status cues feel useful
- which interactive pane-picker and navigation flows exist today

It is not a requirement to preserve:

- the existing shell implementation strategy
- repeated `capture-pane` or broad `ps` usage
- TUI-shaped TSV output
- every legacy heuristic or regex

`agentscan` should learn from that workflow, not clone it.

## Migration Posture

Delivered baseline:

- snapshot scanner from `tmux list-panes`
- provider inference from tmux metadata and titles
- text and JSON output
- interactive `agentscan tui`
- versioned JSON cache snapshot
- pane metadata model for explicit tmux user options
- daemon-backed cache maintenance from tmux control mode
- targeted process-tree fallback for unresolved `node`, `bun`, and `python3`
  launcher panes, including Claude Code binary-path and teammate-spawn evidence
- provider-specific plug-and-play hardening for Gemini CLI, GitHub Copilot CLI,
  Pi, and opencode from upstream source evidence and local probing, while
  keeping weak status inference conservative
- provider-specific pane-output status fallback for already-identified GitHub
  Copilot and Cursor CLI panes, including current idle and busy prompt/footer
  shapes while ignoring stale output
- inspect provenance for provider, status, classification, and fallback
  decisions

Adopted next architecture:

- daemon is required for normal `list`, `inspect`, `focus`, `tui`, and
  `snapshot` flows
- normal consumers auto-start the daemon unless explicitly opted out
- live state moves from cache-file IPC to a Unix-socket JSON-Lines protocol
- the cache file and `agentscan cache` surface are removed after socket
  consumers are migrated
- `agentscan tui` is the interactive command; `agentscan popup` is removed

Definition of done for the current finish pass:

- the release-quality gates in `README.md` pass locally
- docs describe shipped fallback behavior, wrapper metadata, automation
  surfaces, and shell boundaries consistently
- unresolved panes stay conservative unless wrapper metadata, tmux evidence, or
  targeted process fallback provides specific provider evidence
- deferred work is limited to future migration sequencing and additional
  provider-scoped output parsing only if justified by concrete unresolved panes

Further migration sequencing belongs in Linear until it becomes stable enough to
document as a contract in the repo docs.

## Future Direction

Likely next classes of durable improvement:

- continued hardening of tmux client interaction flows
- provider-specific plug-and-play detection hardening, starting with evidence
  gathered from upstream source or explicit local probing
- optional hook support for Codex and Claude Code only as a final enrichment
  layer after plug-and-play support is broadly settled
- optional Pi extension support only as a final enrichment layer after default
  Pi detection works without integration
- incremental output parsing only if later justified by concrete unresolved panes

Those should move from Linear into the repo docs only when they become settled
behavior or durable engineering policy.
