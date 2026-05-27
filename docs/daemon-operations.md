# Daemon Operations

The daemon is required for normal `agentscan` consumers. Users should not have
to install it as a service: normal daemon-backed commands auto-start it unless
explicitly opted out.

Use this document for operational behavior and troubleshooting. Use
`docs/architecture.md` for the internal model and `docs/integration.md` for
machine-readable contracts.

## Start Policy

Normal consumers connect to the daemon socket and auto-start the daemon when it
is not already running. On macOS, detached auto-start is allowed only after the
parent command verifies that the executable is validly signed/trusted. This
keeps detached starts on the signed-binary path recorded in
`docs/adr/macos-daemon-autostart-and-executable-assessment.md`.

Useful commands:

```sh
agentscan daemon status
agentscan daemon status --format json
agentscan daemon start
agentscan daemon run
agentscan daemon stop
agentscan daemon restart
```

Use foreground `agentscan daemon run` for local ad-hoc development builds,
debugging, or any workflow where detached auto-start is intentionally avoided.

## Opt Out Of Auto-Start

Scripts and CI that must not leave a long-lived process running can use:

```sh
agentscan list --no-auto-start
AGENTSCAN_NO_AUTO_START=1 agentscan subscribe --format json
```

Direct tmux recovery paths do not require the daemon:

```sh
agentscan scan
agentscan list --refresh
agentscan hotkeys --refresh
```

## Status JSON

`agentscan daemon status --format json` is always available. It reports daemon
lifecycle, socket, readiness, protocol/schema compatibility, broker health, and
runtime telemetry where available.

Use it when:

- a desktop or TUI subscription is offline;
- a daemon-backed command fails to connect;
- a remote SSH profile needs a cheap compatibility check;
- a developer wants to see whether the reconcile loop is correcting missed
  event-driven updates.

## Event-Driven Behavior And Reconcile Telemetry

The daemon is event-driven first. It subscribes to tmux control-mode events and
refreshes targeted pane, window, or session scopes when tmux gives enough
identity. The control-mode subscription format uses single-brace `#{...}`
directives sent to tmux verbatim — doubling them silently breaks the
subscription (every field renders as a literal `}`, so `%subscription-changed`
never fires on real field changes), which is guarded by a unit test.

**Control mode is scoped to the attached session.** A single control client
receives `%output` and `%subscription-changed` only for panes in its attached
session, never for panes in other sessions. The daemon therefore runs **one
control client per session**: a primary client (command channel + events for its
session) plus an event-only subscriber client for every other session. All
control clients attach with `-f ignore-size,no-output`: `ignore-size` keeps a
client out of the session window-size calculation (so a daemon client never
shrinks a pane), and `no-output` pauses the per-pane `%output` terminal firehose
entirely. Status/title/command/metadata are driven from the throttled (~1s)
`refresh-client -B` subscription, not from scraping `%output`, so the daemon stays
flat under heavy terminal output (e.g. 20 busy agents) while remaining responsive.
Subscribers also never issue commands. All reader threads feed a single shared
event channel. The subscriber set is reconciled at startup and on every
`%sessions-changed`, so sessions created or destroyed at runtime get event
coverage immediately; subscribers whose client process died are pruned and
re-attached on the next reconcile. The number of subscriber clients is capped
(64) so a pathological session count cannot spawn unbounded `tmux -C` processes;
when there are more sessions than the cap, subscriber coverage is incomplete, so
the daemon keeps the reconcile poll at its **active** interval (30s) instead of
relaxing to the 300s self-heal backstop, ensuring the un-subscribed sessions are
not starved. The lowest-numbered sessions keep their event clients. `list-panes` is
server-wide, so it always runs on the primary regardless of how many sessions
exist. Result: every session is event-driven for status/title/command/metadata
(~1s), not just the attached one.

The within-session redundancy reconcile is **disabled by default**
(`disable_reconcile` defaults to `true`). Because all sessions are now event-driven
via subscriber clients, the periodic poll is no longer responsible for
cross-session coverage; with reconcile disabled it is reduced to an infrequent
**self-heal/drift backstop** (default 300s) that exists only to recover from rare
event drift (a missed notification or a subscriber that failed to attach). Broker
fallback (no event stream at all) always keeps the fast 1s reconcile regardless of
`disable_reconcile`, because then the poll is the sole update path. The
connect/reconnect bootstrap reconcile also runs unconditionally as initial-truth
and gap-recovery sync. Interval summary (broker active):

- `disable_reconcile = true` (default): self-heal/drift backstop every 300s
- `disable_reconcile = false`: full redundancy reconcile every 30s
- broker fallback: 1s, regardless of `disable_reconcile`

Reconcile materiality ignores timestamp-only differences and cache-origin churn
so telemetry can show whether the poll is finding actual missed state. Run with
`disable_reconcile = false` to populate `reconcile_changed_snapshot_count` as the
redundancy meter.

`AGENTSCAN_CONTROL_MODE_ACTIVE_RECONCILE_INTERVAL_MS` overrides the full
redundancy interval and `AGENTSCAN_CONTROL_MODE_SELF_HEAL_INTERVAL_MS` overrides
the self-heal backstop interval, for tests and diagnostics. Leave them unset in
normal use.

Runtime telemetry counters in `daemon status --format json` include:

Volume counters are always recorded for every control-mode batch, including
ignored-only `%output` firehose bursts. They are integer-only and add no
allocation to the hot path, so the firehose stays measurable without enabling
deep telemetry:

- `control_event_batch_count` for every processed control-mode batch
- `control_event_line_count` for the control-mode lines received across all
  batches
- `control_event_output_line_count` for `%output` lines specifically (the
  per-pane terminal-output firehose). Subtract the title count to size the
  *wasted* firehose: `%output` lines that did not carry a terminal-title escape
- `control_event_output_byte_count` for the total bytes of those `%output` lines
- `control_event_ignored_count` for lines that produced no actionable event

The remaining counters are recorded only for batches with an actionable event
(or for every batch when `AGENTSCAN_DEEP_CONTROL_MODE_TELEMETRY=1`):

- `control_event_refresh_count` for control-mode batches that refreshed the daemon
  snapshot
- per-kind control-mode counters for pane, title, window, session, and
  resnapshot events
- `reconcile_attempt_count`
- `reconcile_noop_count`
- `reconcile_changed_snapshot_count`
- `targeted_title_update_count`
- `targeted_pane_refresh_count`
- `targeted_scope_refresh_count`
- `full_snapshot_refresh_count`
- `targeted_refresh_fallback_to_full_count`
- `broker_fallback_count`

Pane-output `capture-pane` counters track the daemon's status-fallback path, the
relatively expensive per-pane capture used to read provider status when tmux
metadata and titles do not carry it:

- `pane_output_capture_attempt_count` for capture-pane calls actually issued
- `pane_output_capture_hit_count` for status reuses served from the TTL cache
  without a capture-pane call; cache effectiveness is
  `hit / (hit + attempt)`
- `pane_output_capture_error_count` for capture-pane calls that failed

When telemetry is unavailable, not initialized, or not published by an older
compatible daemon, these fields are `null`.

No-op reconcile passes are intentionally silent on the subscription stream. Use
daemon status counters for observability; do not treat periodic snapshot frames
as heartbeats.

`agentscan daemon status --events --format json` includes the daemon's bounded
in-memory recent event ring. These records summarize recent control-event and
reconcile work, whether the snapshot materially changed, whether a frame was
published, duration, and a compact pane-level diff when available. The ring is
bounded and is not part of normal list, snapshot, subscribe, TUI, or desktop
consumer payloads.

Status JSON also includes `latest_snapshot_observability`, a cheap summary of
the latest snapshot's provider-known/unknown counts, status-source counts, and
proc-fallback outcomes. Use this to see whether proc fallback is resolving
current panes without scraping individual pane diagnostics.

`latest_snapshot_observability.per_provider` breaks those signals down by
provider (keyed by canonical provider name; unclassified panes bucket under
`unknown`). For each provider it reports `pane_count`, the identity match-kind
counts (`matched_pane_metadata`, `matched_pane_current_command`,
`matched_pane_title`, `matched_proc_process_tree`), and the status-source counts
(`status_source_pane_metadata`, `tmux_title`, `pane_output`, `not_checked`).
This is the live per-provider tuning matrix: a non-zero
`matched_proc_process_tree_count` means that provider currently depends on proc
inspection for identity, and a high `status_source_pane_output_count` means it
depends on capture-pane for status. The buckets are computed once per snapshot
publish, not per control-mode event.

For opt-in durable event tracing, run the daemon with:

```sh
AGENTSCAN_TRACE_EVENTS=1 AGENTSCAN_TRACE_EVENT_LIMIT=1000 agentscan daemon run
```

Trace events are written as bounded JSON Lines to the `event_log_path` reported
by `agentscan daemon status`. The file is truncated on daemon start and rotated
by line count, so tracing is intentionally not an unbounded append-only log.

Ignored control-mode lines no longer churn the recorded event ring or snapshot
diffs by default, but their volume is always tallied in the always-on counters
above (batch, line, `%output` line/byte, and ignored counts). Because the
telemetry frame is published lazily — on the next snapshot-changing event or
reconcile pass rather than per firehose batch — a pure `%output` burst under
`disable_reconcile` may show slightly stale volume counters until the next
flush. Set `AGENTSCAN_DEEP_CONTROL_MODE_TELEMETRY=1` to also record the per-kind
counters and event ring for otherwise-silent batches, or enable
`AGENTSCAN_TRACE_EVENTS=1` for per-event detail.

Diagnostic-only knobs can isolate expensive or safety-net behavior:

```toml
# ${XDG_CONFIG_HOME:-~/.config}/agentscan/config.toml
disable_reconcile = true
disable_proc_fallback = true
```

```sh
AGENTSCAN_DISABLE_RECONCILE=1 agentscan daemon run
AGENTSCAN_DISABLE_PROC_FALLBACK=1 agentscan daemon run
```

The environment variables override config file values. `disable_reconcile`
disables the periodic/timeout reconcile safety loop, but event-triggered full
resnapshots can still occur when tmux emits a broad structural event.
`disable_proc_fallback` marks proc fallback diagnostics as skipped with a clear
reason. The daemon reads these runtime options on startup, so restart the daemon
after changing config. These are debugging controls, not recommended defaults.


## Broker Fallback

Steady-state daemon `list-panes` reads use the brokered tmux control-mode
command path. The broker shares the daemon's long-lived control-mode client,
collects command responses, buffers unrelated events, and falls back to
short-lived tmux commands only when a broker command fails.

`daemon status` reports whether the broker is active or in fallback, the last
disabled reason, reconnect count, fallback count, and
`control_mode_broker_subscriber_count` — the number of per-session event-only
subscriber clients (one per non-primary session; `unavailable`/`null` from a
daemon that predates the per-session-client architecture). Broker fallback is a
degraded but intentional continuity path; repeated fallback should be treated as
a daemon/tmux integration issue.

## Common Failure Modes

| Symptom | Likely cause | Next step |
|---------|--------------|-----------|
| Not running JSON shape | daemon socket missing and auto-start disabled or refused | remove `--no-auto-start`, unset `AGENTSCAN_NO_AUTO_START`, or run `agentscan daemon run` |
| macOS detached start refused | executable is ad-hoc, invalidly signed, quarantined, or otherwise untrusted | use a signed/notarized build or run foreground `agentscan daemon run` |
| Protocol/schema incompatible | old daemon still running after upgrade | `agentscan daemon restart` |
| tmux gone or unavailable | tmux server exited or is not reachable from daemon environment | restart tmux or point `AGENTSCAN_TMUX_SOCKET` at the intended server |
| Repeated broker fallback | control-mode command path is failing | inspect daemon status JSON and logs, then restart daemon if needed |
| Remote desktop profile cannot focus | SSH exec has no current tmux client | configure/pass `--client-tty` when known |
