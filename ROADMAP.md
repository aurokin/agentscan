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

The current architecture is a daemon-required runtime with short-lived socket
clients.

Implications:

- the daemon is the single source of live pane state
- consumers connect to the daemon over a Unix socket and read
  `SnapshotEnvelope` frames
- daemon startup is automatic for normal desktop commands; macOS only allows
  detached auto-start after parent-side executable trust preflight succeeds
- direct tmux snapshots remain available for debugging and recovery through
  `agentscan scan` and refresh-capable command flags
- `AGENTSCAN_NO_AUTO_START=1` and `--no-auto-start` exist for CI and scripts
  that must not leave a daemon running
- macOS release binaries are Developer ID signed, hardened-runtime enabled,
  timestamped, and notarized before release packaging so detached daemon
  startup can run through a signed-binary path
- macOS ad-hoc or locally built binaries should use `agentscan scan`,
  `--refresh`, or foreground `agentscan daemon run`
- when tmux disappears, the daemon reports failure through lifecycle/status
  paths; restart policy remains explicit user or supervisor policy
- the control-mode subscription format is sent to tmux verbatim and therefore
  uses single-brace `#{...}` directives; doubling them silently breaks the
  subscription (every field renders as a literal `}`, so `%subscription-changed`
  never fires on real changes). A unit test guards against regressing to `#{{`.
- **tmux control mode is scoped to the attached session**, so the daemon runs one
  control client per session: a primary (command channel + its own session's
  events) plus an event-only subscriber client for every other session. All
  clients attach with `-f ignore-size,no-output` and subscribe via
  `refresh-client -B`, feeding one shared event channel. `ignore-size` keeps daemon
  clients out of window-size calculation; `no-output` **pauses the per-pane
  `%output` terminal firehose globally**, so status/title/command/metadata come
  from the throttled (~1s) subscription rather than from scraping terminal output.
  This keeps the daemon flat under heavy load (e.g. ~20 busy agents emitting tens
  of thousands of output lines/sec) while staying responsive. The subscriber set is
  reconciled at startup and on every `%sessions-changed`, so sessions
  created/destroyed at runtime get event coverage immediately; dead subscriber
  clients are also checked by a lightweight subscriber-health monitor (~250ms)
  that runs independently of the event stream, so continuous events from one
  session cannot delay detection that another session's subscriber went stale.
  Subscriber health is reported through `agentscan daemon status` (desired/active
  counts, missing/dead sessions, per-subscriber line/event timestamps, monitor and
  reconcile deadlines, and cumulative monitor/start/reattach/failure/exit
  counters). The subscriber count is capped (64) for pathological session counts;
  when the count exceeds the cap the lowest-numbered sessions keep event clients
  and the reconcile poll stays at its active interval (rather than the self-heal
  backstop) so the un-subscribed sessions are not starved.
  This makes every session — not just the attached one — event-driven for
  status/title/command/metadata, which is the product-critical requirement
  (responsive cross-session agent appear/disappear/status).
- the within-session **redundancy** reconcile defaults to **off**
  (`disable_reconcile` defaults to `true`); all sessions are event-driven via the
  per-session subscriber clients. Disabling it does not stop the poll entirely — it
  reduces it to an infrequent **self-heal/drift backstop** (`list-panes -a`,
  default 300s) that only recovers from rare event drift or a failed subscriber
  attach; it is no longer responsible for cross-session latency. Broker fallback
  (no event stream) always keeps the fast 1s reconcile as the sole update path,
  and the connect/reconnect bootstrap reconcile (initial truth + gap recovery)
  runs unconditionally. Setting `disable_reconcile = false` restores the full 30s
  redundancy reconcile and its meter (`reconcile_changed_snapshot_count`).
- control-event observability is always source-aware: recent daemon events record
  whether a batch came from the primary control client or from a per-session
  subscriber, with per-source line and parsed-event counts. Raw control-mode
  lines remain debug-only and are included in recent events / the JSONL event
  trace only when `AGENTSCAN_TRACE_CONTROL_LINES=1` is set.
- **pane-output providers** (status read only from captured pane output, never from
  tmux metadata — e.g. pi without the `titlebar-spinner` extension, droid) are kept
  responsive without the `%output` pty firehose. A separate activity subscription includes
  `window_activity`, so pane output fires a distinct `%subscription-changed` event. The daemon
  turns that into a cache-throttled re-capture only when the pane's current status path can require
  captured output; noisy unknown panes and metadata-driven providers do not trigger a targeted
  tmux read. Identity and metadata remain in their own all-pane subscription, so new
  agents are still discovered immediately. Because an idle transition emits no activity, the
  daemon also arms a single **settle re-check** deadline (~2.2s) while any pane-output pane
  reads busy and re-reads those panes when it fires (busy→idle); the deadline is armed once and
  not pushed out by unrelated panes' activity, so the re-check is not starved. This preserves
  the `no-output` flat-cost invariant (no per-byte pty stream) while making non-metadata
  providers event-responsive. Where a provider *does* publish state in tmux metadata
  (claude/codex/grok titles; pi's `titlebar-spinner` extension), that cheaper title-driven path
  is preferred automatically by the layered detection — both paths are supported, selected by
  cost.

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
- lifecycle `stop` is the exception to the data-contract boundary: it may
  terminate a protocol/schema-mismatched daemon from the persisted identity
  sidecar only after the sidecar socket path, socket peer PID, lifecycle lock,
  live PID, and executable checks pass; this path parses only stable sidecar
  identity fields so older sidecar shapes can still be stopped safely
- one-shot commands read a full snapshot frame and disconnect
- TSV is an output adapter only, not the canonical store
- persisted cache JSON is not a supported IPC boundary
- schema 5 added per-pane tmux active flags (`tmux.pane_active`,
  `tmux.window_active`) and the derived picker `is_active`
  (`pane_active && window_active`), so clients can highlight the focused pane.
  The daemon control-mode subscription also tracks `#{pane_active}`/
  `#{window_active}` so focus changes trigger a re-snapshot. The backend reports
  tmux's raw layered state; collapsing multiple attached sessions to one global
  active pane is a client concern, not a backend one.
- focus actions from `agentscan focus`, `agentscan hotkey`, and the terminal TUI
  also emit a best-effort daemon client event after tmux confirms the switch. The
  daemon still re-reads tmux as the source of truth, but the client event forces
  a fresh snapshot publication even when pane metadata is materially unchanged,
  so live consumers can recompute the focused pane immediately.

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
- pane-output matchers anchor to the last *rendered* line: trailing blank rows are
  trimmed once centrally before any provider matcher runs, so a top-rendered or
  freshly started agent (whose prompt/footer is far above the pane's blank padding)
  is still read from its current UI rather than the pane's physical bottom.
- process fallback is targeted live process inspection, not broad system
  scanning. It is limited to concrete ambiguous panes, checks the foreground
  process group for shell or wrapper panes, and checks root/descendant process
  command, argv, and selected environment markers for known launcher panes.
- the daemon runs this targeted fallback on its live event path too, not only on
  full snapshots: when a control-event refresh (re)builds a pane that is still
  unidentified but agent-shaped (version-like command, spinner/idle glyph, or a
  shell/launcher foreground), it inspects that one process tree. This keeps
  metadata-invisible agents detected without waiting for a full reconcile — most
  importantly Claude Code, whose command is its version string and whose title is
  the current task, so it is identifiable only from the `claude` process. It stays
  bounded: only unidentified agent-shaped panes are inspected, and a resolved pane
  is no longer a candidate.
- process fallback should use native platform inspection only. Linux reads
  procfs directly; macOS reads `libproc` / `sysctl` data directly. Avoidable
  helper process launches such as `ps`, `pgrep`, and `grep` are not part of the
  scanner hot path.
- tmux subprocesses remain an intentional boundary for direct snapshots,
  initial daemon bootstrap, focus, metadata helpers, and pane-output fallback.
  Steady-state daemon `list-panes` refreshes use the brokered control-mode
  command path, with short-lived tmux reads retained as fallback when a broker
  command fails.
- daemon pane-output status fallback may use a short-lived in-memory cache to
  throttle repeated `capture-pane` reads during refresh bursts. The cache is
  local to the daemon, keyed by pane identity and classification inputs, and is
  not canonical state. Direct `scan` snapshots remain uncached.
- daemon refresh `list-panes` reads may use the control-mode event client only
  through the brokered command path that owns response collection, event
  buffering, timeouts, fallback, reconnect attempts, and lifecycle status
  reporting. See
  `docs/notes/daemon-redesign-decisions.md` for the implementation decisions
  and `docs/notes/daemon-redesign-brief.md` for the original migration slices.
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
- tmux client/server version splits are handled plug-and-play: when a fresh
  client is dropped mid-handshake ("server exited unexpectedly", "lost server",
  "protocol version mismatch" — empirically a healthy newer server dropping an
  older client, e.g. a linuxbrew 3.6b server vs the apt 3.4 tmux that
  non-interactive SSH resolves), agentscan probes well-known installs
  (linuxbrew, Homebrew, MacPorts, system) for one that completes a real
  handshake against the same socket, then reroutes all tmux execs through it
  for the rest of the process. Validation-by-handshake keeps false positives
  out; `AGENTSCAN_TMUX_BIN` pins an explicit binary and disables the
  auto-resolution. A missing-tmux PATH (binary not found at all) is a possible
  future extension of the same candidate list.

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

### Desktop And Remote Client Boundary

Terminal and desktop clients should converge on the same command/event
contract. A local desktop runner executes `agentscan` directly. A remote
desktop runner executes the same commands through SSH using the user's normal
SSH configuration and authentication.

Implications:

- desktop code owns host/profile selection, process supervision, stdout/stderr
  handling, reconnect policy, rendering, global hotkeys, and error presentation
- the desktop reconnect policy is **latch-only**: the dock attaches to an
  existing daemon (`subscribe --no-auto-start`) and auto-reconnects with backoff
  when a recoverable close (daemon restart / socket superseded) ends the stream,
  but never starts a daemon on its own. A missing daemon surfaces a `noDaemon`
  state with an explicit "Start agentscan" action — the only path that passes
  `auto_start: true`. This also makes app launch latch-only (no auto-start at
  startup). The policy and its connection state machine live in the Effect
  `LiveConnection` service (`desktop/src/effect/`), which is the single owner of
  the live connection lifecycle the dock observes. The Rust live worker is
  single-shot per epoch (no in-worker retry loop, AUR-517): transient mid-stream
  drops self-heal inside the `agentscan subscribe` CLI, and any session-ending
  close — clean daemon loss or abnormal subscribe-child death alike — surfaces as a
  terminal frame the service re-arms with a fresh, latch-only epoch, so all
  re-arm/backoff ownership sits in `LiveConnection`. While in `noDaemon`, the
  service cheap-polls `agentscan daemon status --format json` (AUR-518) instead of
  re-arming a full `subscribe` each backoff tick — expensive over SSH — and only
  escalates to a full re-arm once a daemon is reachable (or the probe can't tell).
- the desktop live pipeline is keyed multi-source, not single-source: the Rust
  side holds one supervisor per source key (the frontend's runnerKey) with
  per-key epoch gating, every live event envelope carries its source key, and
  `LiveConnection` runs one supervised subscription fiber per configured source
  over a per-key state map (`configure` diffs a target list: start added keys,
  interrupt removed keys, leave unchanged keys running). The latch-only policy,
  epoch fencing, and explicit-Start latch all apply per key. The dock configures
  one target per OPEN host folder (below), so multiple sources hold live
  subscriptions concurrently.
- the dock's vertical strip is a list of host folders: one collapsible section
  per enabled source, in the user's persisted source order (drag-reorder in
  Settings). Open folder = live subscription armed + that source's
  workspace-grouped rows and per-source recovery strip; closed folder = header
  only, no subscription. Open state persists per profile id
  (`openProfileIds`; migration opens the previously-active profile). Row
  keybinds (Ctrl+<key>) are owned by exactly ONE source — the topmost open
  folder in source order (`keybindOwnerId`) — and resolve only against the
  owner's rows; other folders render their key labels dimmed as information,
  and every activation runs with the row's OWN source's runner settings.
  Ownership follows reorder and passes to the next open folder when the owner
  closes. The horizontal bar keeps the single-source presentation, showing the
  keybind owner. Preflight still probes only the settings-selected active
  source; other open sources arm on committed-profile validity and surface
  failures through their keyed live state. The full-screen boot/recovery
  takeover is scoped to when no other open folder could render; with another
  folder open, the active source's preflight failure surfaces inside its own
  folder (error dot + Open settings strip). The horizontal bar still shows
  only the owner's keyed live state, so when the takeover is suppressed the
  active source's preflight failure has no dedicated surface there — a known
  gap of the single-source bar, not of the folder model.
- the machine that owns tmux also owns `agentscan` daemon lifecycle,
  classification, picker rows, hotkey actions, and focus actions
- remote desktop support is SSH command execution around documented JSON/JSONL
  CLI surfaces, not daemon socket forwarding, remote tmux parsing, or a
  desktop-specific scanner protocol
- the remote command runs in a non-interactive `ssh host "cmd"` shell, which
  skips rc files, so version-manager (mise/asdf), `~/.cargo/bin`, and
  `~/.local/bin` installs are absent from its PATH. To keep remote agentscan
  plug-and-play, the SSH runner appends those dirs to PATH inside a POSIX
  `sh -c 'PATH="$PATH:…"; export PATH; exec env "$@"'` wrapper (binary/args/env
  forwarded as `"$@"`) so a bare-name `agentscan` resolves without the user
  configuring an explicit binary path. The `sh -c` wrapper keeps the command
  shell-agnostic — correct on fish (where `$PATH` is a list) and csh, and safe
  when a PATH entry contains spaces — preserving the property the bare `exec env`
  form had; the dir list is a fixed constant, so there is no injection surface.
  Dirs are appended *after* `$PATH` — so the remote's own resolution always wins
  and a stale shim can't shadow a binary already on PATH — and version-manager
  shims sit last in the fallback list for the same reason. An explicit binary
  path or a PATH set in profile env still wins. The same precedence-ordered dir
  set seeds the local runner's auto-detect.
- remote diagnosis stays explicit and bounded, not eager: only when a preflight
  fails as binary-not-found does the desktop run one best-effort SSH probe (the
  remote's login + interactive shell — `-l` for `.profile`/`.zprofile`, `-i` for
  `.zshrc`/`.bashrc` — to mirror the SSH login shell the desktop's commands run
  under) to locate agentscan and turn the dead-end into an actionable hint. It
  probes the profile's *configured* binary name (not a hard-coded `agentscan`, so
  a custom name/wrapper isn't overwritten) and reports only an absolute,
  executable path (an alias/function is never offered as a binary), is gated to
  the not-found case, fails fast (`BatchMode`/`ConnectTimeout`), and
  degrades silently — including for non-POSIX (fish/csh) login shells — so it
  never slows or muddies local-only debugging when no host is reachable. The CLI
  `doctor` stays local-only; SSH-client logic lives in the desktop, which owns the
  transport boundary.
- non-default remote tmux targets must propagate both `AGENTSCAN_TMUX_SOCKET`
  and an isolated `AGENTSCAN_SOCKET_PATH` so the remote daemon and tmux server
  stay paired
- remote install/bootstrap UX is a desktop product follow-up, not part of the
  scanner contract

### Platform Priority

Linux and macOS are the primary targets for early fallback logic.

Implications:

- Linux fallback reads selected `/proc` command, argv, env, tty, and descendant
  fields for unresolved launcher panes.
- macOS fallback reads native `libproc` / `sysctl` command, argv, tty, and
  descendant fields for unresolved launcher panes. `KERN_PROCARGS2` env strings
  are parsed when present, but live macOS env visibility is not guaranteed and
  must not be the only required signal for baseline detection.

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
- shared picker rows and actions through `agentscan hotkeys --format json`,
  strict `agentscan hotkey <key>`, and tmux-safe `agentscan tmux hotkey <key>`
- live client events through `agentscan subscribe --format json`
- versioned JSON snapshot envelope
- pane metadata model for explicit tmux user options
- daemon-backed socket snapshots from tmux control mode
- targeted process-tree fallback for unresolved `node`, `bun`, and `python3`
  launcher panes, including Claude Code binary-path and teammate-spawn evidence
- provider-specific plug-and-play hardening for Gemini CLI, GitHub Copilot CLI,
  Cursor CLI, Pi, opencode, Antigravity, Grok, Hermes, Aider, and Factory Droid
  from upstream source evidence or local probing, while keeping weak status
  inference conservative
- provider-specific pane-output status fallback for already-identified
  supported providers, including current idle and busy prompt/footer shapes
  while ignoring stale output
- inspect provenance for provider, status, classification, and fallback
  decisions
- provider icon modes through CLI/env/config, with `emoji` as default and
  `agentscan providers --format json` exposing all marker data, including the
  custom patched Nerd Font `agent-icons-v9` codepoints
- configurable 16-slot picker key order through `picker_keys` in the core config
  file, with the previous `1 2 3 4 5 Q E R F G T Z X C V B` order preserved as
  the default shared contract

Delivered daemon architecture:

- daemon is required for normal `list`, `inspect`, `focus`, `tui`, and
  `snapshot` flows
- normal consumers auto-start the daemon unless explicitly opted out; macOS
  starts only when executable trust preflight succeeds
- live state uses a Unix-socket JSON-Lines protocol
- the daemon is event-driven first, with a reconcile safety loop and telemetry
  counters for event/reconcile behavior
- diagnostic config can disable the reconcile safety loop or proc fallback for
  observability investigations, with environment variables as runtime overrides
- the cache file and `agentscan cache` surface are removed
- `agentscan tui` is the interactive command; `agentscan popup` is removed

Delivered desktop baseline:

- macOS-first Tauri desktop app in `desktop/`
- local and SSH profiles over the shared CLI command contract
- supervised desktop subscription over `agentscan subscribe --format json`
- picker rendering over `agentscan hotkeys --format json`
- focus actions delegated to `agentscan focus <pane-id>`
- app-global `CommandOrControl+Shift+A` summon shortcut
- cursor-display sidebar placement with primary-display fallback
- search/filter as client-side UI over returned picker rows
- Apple Silicon macOS desktop release artifact, local desktop release/smoke
  workflow, and strict version metadata check

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
