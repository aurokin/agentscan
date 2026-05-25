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
identity.

A reconcile loop remains as a safety net until control-mode events have proven
sufficient across real workflows. When the control-mode broker is active, the
safety reconcile interval is 30 seconds; broker fallback keeps the shorter
interval so command-backed reads can recover promptly. Reconcile materiality
ignores timestamp-only differences and cache-origin churn so telemetry can show
whether the safety loop is finding actual missed state.

`AGENTSCAN_CONTROL_MODE_ACTIVE_RECONCILE_INTERVAL_MS` can override the active
broker safety interval for tests and diagnostics. Leave it unset in normal use.

Runtime telemetry counters in `daemon status --format json` include:

- `control_event_batch_count` for processed control-mode batches that contain a
  control event, including no-op batches; raw ignored output is counted only
  when `AGENTSCAN_DEEP_CONTROL_MODE_TELEMETRY=1` is set
- `control_event_refresh_count` for control-mode batches that refreshed the daemon
  snapshot
- `control_event_line_count` for the control-mode lines received across processed
  batches
- per-kind control-mode counters for pane, title, window, session, resnapshot,
  and ignored events
- `reconcile_attempt_count`
- `reconcile_noop_count`
- `reconcile_changed_snapshot_count`
- `targeted_title_update_count`
- `targeted_pane_refresh_count`
- `targeted_scope_refresh_count`
- `full_snapshot_refresh_count`
- `targeted_refresh_fallback_to_full_count`
- `broker_fallback_count`

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

For opt-in durable event tracing, run the daemon with:

```sh
AGENTSCAN_TRACE_EVENTS=1 AGENTSCAN_TRACE_EVENT_LIMIT=1000 agentscan daemon run
```

Trace events are written as bounded JSON Lines to the `event_log_path` reported
by `agentscan daemon status`. The file is truncated on daemon start and rotated
by line count, so tracing is intentionally not an unbounded append-only log.

Ignored control-mode lines are also telemetry-silent by default so high-volume
pane output does not churn daemon status state. Set
`AGENTSCAN_DEEP_CONTROL_MODE_TELEMETRY=1` when investigating raw control-mode
event volume and no-op daemon wakeups.

Diagnostic-only knobs can isolate expensive or safety-net behavior:

```sh
AGENTSCAN_DISABLE_RECONCILE=1 agentscan daemon run
AGENTSCAN_DISABLE_PROC_FALLBACK=1 agentscan daemon run
```

`AGENTSCAN_DISABLE_RECONCILE=1` disables the periodic/timeout reconcile safety
loop, but event-triggered full resnapshots can still occur when tmux emits a
broad structural event. `AGENTSCAN_DISABLE_PROC_FALLBACK=1` marks proc fallback
diagnostics as skipped with a clear reason. These are debugging controls, not
recommended defaults.


## Broker Fallback

Steady-state daemon `list-panes` reads use the brokered tmux control-mode
command path. The broker shares the daemon's long-lived control-mode client,
collects command responses, buffers unrelated events, and falls back to
short-lived tmux commands only when a broker command fails.

`daemon status` reports whether the broker is active or in fallback, the last
disabled reason, reconnect count, and fallback count. Broker fallback is a
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
