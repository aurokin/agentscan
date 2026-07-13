# Desktop Client Contract

Local and remote desktop clients use the same `agentscan` command contract. The
desktop shell owns process execution, SSH orchestration, window lifecycle,
global hotkeys, rendering, and retry policy. The host that owns tmux owns the
daemon, discovery, classification, picker rows, and focus actions.

There is no desktop-specific scanner path.

## Shared Command Surfaces

For a local target, the desktop command runner executes commands directly:

```sh
agentscan daemon status --format json
agentscan subscribe --format json
agentscan hotkeys --format json
agentscan hotkey <key>
agentscan focus %1
```

For a remote target, the desktop command runner executes the same commands
through the user's normal SSH configuration and authentication:

```sh
ssh workbox agentscan daemon status --format json
ssh workbox agentscan subscribe --format json
ssh workbox agentscan hotkeys --format json
ssh workbox agentscan hotkey <key>
ssh workbox agentscan focus %1
```

The desktop app treats SSH as transport around stdout, stderr, exit status, and
process cancellation. It must not tunnel the daemon Unix socket as the primary
remote design, parse remote tmux directly, or duplicate scanner logic in
desktop code.

Picker selection keys are part of the CLI picker model. Desktop clients must
render and activate the returned `row.key` values from
`agentscan hotkeys --format json` instead of assuming the built-in default order,
because users can customize `picker_keys` and `picker_group_by` in the host
`agentscan` config.

`agentscan hotkeys --format json` returns a versioned envelope,
`{ "schema_version": 1, "rows": [...] }` (like the snapshot's `panes` wrapper).
Clients validate `schema_version` and read rows from `rows`. The same row shape
also rides inline on each subscribe `snapshot` frame (see Live State), so a
client with a live subscription never needs a separate `hotkeys` call per update.

When picker rows include `workspace`, clients should treat `workspace.id` as the
grouping identity and `workspace.label` as display text. Labels are intentionally
short and may collide across unrelated repositories or folders; `workspace.id`
keeps those groups distinct without changing what users see.

## macOS Window Lifecycle

The macOS desktop app registers an app-global `CommandOrControl+Shift+A`
shortcut through Tauri's global shortcut plugin. This summons (raises and
focuses) the desktop picker window — the window is persistent and the shortcut
never hides it — then the picker consumes the command contract above.

Tmux-prefix-originated launch remains a separate future integration path for
terminal workflows and should not be conflated with the app-global shortcut.
If macOS privacy behavior changes around global keyboard shortcuts, treat
Accessibility/Input Monitoring prompts as desktop packaging and signing
concerns; do not move shortcut handling into tmux or scanner code.

On summon, the macOS desktop picker is snapped to its current orientation — a
narrow sidebar by default, or a full-width bottom bar when the layout is
horizontal — and placed on the work area of the display containing the cursor.
If cursor or monitor lookup fails, the app falls back to the primary display
and still shows/focuses the picker.

## Live State

The desktop app keeps one supervised `agentscan subscribe --format json`
process per configured (open) source for live state, local and remote
subscriptions streaming concurrently (the keyed multi-source model recorded in
`ROADMAP.md`). Each `snapshot` frame carries its picker `rows` inline — the same
rows `agentscan hotkeys --format json` returns, assembled on the tmux-owning host
with live focus and client resolution — so the client renders directly from the
frame. This preserves the shared CLI picker contract (no key assignment or
display shaping reimplemented in desktop code) while avoiding a second full tmux
scan and SSH round-trip per update. The desktop still calls
`agentscan hotkeys --format json` for the one-shot initial picker load before a
subscription is established.

The desktop consumes the `agentscan subscribe --format json` **event stream**,
not the daemon's raw Unix-socket wire frames. The daemon↔subscribe-client wire
was changed to broadcast incremental `snapshot_diff` frames after the bootstrap
full snapshot, but reconstruction happens host-side inside the `agentscan
subscribe` process: it applies each diff and re-emits a complete
`LiveClientEvent::Snapshot` (with `rows`) per update. Desktop consumers therefore
still see only full `snapshot` event frames and need no changes for the diff
protocol; the diffs are invisible above the subscribe process boundary.

The subscribe-frame contract is intentionally **tolerant of additive frame
types**: a frame whose `type` is unknown to the client is ignored (a no-op), so a
newer daemon can introduce frame types without breaking the live view on an older
desktop build. Only a *known* `type` with a malformed payload, or a line that is
not valid JSON, is a protocol error that tears the subscription down. Clients
should follow the same rule rather than adding a dedicated handler per frame type.
Because `rows` is a required field of the `snapshot` frame, a host too old to
emit it produces a malformed known frame that reconnects rather than rendering an
empty picker; the desktop and host `agentscan` are expected to move together.

When the subscription exits unexpectedly, the UI keeps the last successful
snapshot visible while reconnecting and uses:

```sh
agentscan daemon status --format json
```

for lightweight diagnostics.

### Active-pane and focused-pane indicators (schema 5+)

Each picker row carries `is_active` and `is_focused`, and each snapshot pane
carries `tmux.pane_active` and `tmux.window_active`.

`is_active` is the derived `pane_active && window_active`: the active pane of the
active window. Because tmux tracks this per session, `is_active` can hold for one
pane per attached session — it is *not* a single global signal. The backend
reports tmux's raw layered state without collapsing it.

`is_focused` is the collapsed signal: at most one row across the whole picker is
`is_focused`, identifying the pane the user is actually in. It is the active pane
(`is_active`) of the session that the most-recently-active attached tmux client
is viewing, resolved live by `hotkeys` from `list-clients` (`#{client_activity}`,
`#{client_session}`). Clients should highlight / default selection to the
`is_focused` row, falling back to first-row or last selection when none is set.

Client resolution counts *interactive* tmux clients only. The agentscan daemon
attaches one control-mode client per session for snapshotting, which tmux reports
with an empty `#{client_tty}`; these are server plumbing, not humans watching
panes, so `hotkeys` ignores tty-less clients when resolving focus and counting
clients. A human client always has a tty — including a human control-mode client
such as iTerm2 `tmux -CC`, which keeps its pty and is still counted and eligible
to anchor focus.

Multiple-client semantics (we optimize for one attached client):

- 0 clients attached → no row is `is_focused`.
- 1 client → the active pane of its session.
- N clients with a clear most-recently-active winner → that client's session.
- N clients tied at the most-recent activity, all viewing the same session →
  that session.
- N clients tied across *different* sessions → ambiguous; no row is `is_focused`,
  so clients should not move the cursor rather than guess.

`is_focused` resolution is best-effort: any tmux error degrades to "no focused
pane" and never fails the picker. It re-resolves whenever picker rows are built —
on every `hotkeys` call and on every subscribe `snapshot` frame, since the host
rebuilds the frame's `rows` from live `list-clients` state — so it follows focus
live. Successful focus actions emit a best-effort daemon client event after tmux
confirms the switch; the daemon re-reads tmux and republishes a snapshot so this
refresh does not have to wait for the next tmux notification.

Each row also carries `attached_client_count`: the count of *interactive* clients
attached to the tmux server, echoed on every row because the picker output is a
flat array with no envelope. It deliberately excludes the daemon's tty-less
clients (per the resolution rule above), so it reflects the number of real views
a human could be looking at rather than internal plumbing — otherwise the count
would never be 1 on a daemon-backed server. `>1` means
focus-following is best-effort (the highlight tracks the most-recently-active
client); clients may surface a hint, as the macOS desktop does with a "multiple
clients attached" banner.

## Profiles And Environment

The local profile can override the `agentscan` binary path and provide
additional environment variables for every local command and live subscription
process.

SSH profiles add a host field and wrap the same `agentscan` command arguments
in the user's normal SSH configuration. Remote environment variables are
exported inside the remote shell command before `agentscan` starts; they are
intended for scoped values such as `AGENTSCAN_TMUX_SOCKET` and
`AGENTSCAN_SOCKET_PATH`.

The desktop debug log records command names, outcomes, errors, and stream
events, but it should show environment variable names/counts rather than
dumping values into routine diagnostics.

## Non-Default Remote Tmux Servers

Remote commands target the remote default tmux server and daemon socket unless
the runner supplies an isolated context. For non-default tmux servers, desktop
profiles should propagate both `AGENTSCAN_TMUX_SOCKET` and a matching
`AGENTSCAN_SOCKET_PATH` into each remote command rather than parsing tmux
directly:

```sh
ssh workbox 'mkdir -p "$HOME/.local/state/agentscan" && env \
  AGENTSCAN_TMUX_SOCKET=/tmp/tmux-501/custom \
  AGENTSCAN_SOCKET_PATH="$HOME/.local/state/agentscan/custom.sock" \
  agentscan subscribe --format json'
```

That keeps discovery, subscription, hotkeys, and focus scoped to the same
remote tmux target.

## Focus And Client TTY

Focus actions target a tmux client, not only a pane. Terminal-launched commands
can usually infer the current client from tmux, then fall back to the most
recent attached client.

A desktop SSH exec often has no current tmux client, so when the desktop owns
or knows the intended remote tmux view it should pass that client explicitly:

```sh
ssh workbox agentscan hotkey <key> --client-tty /dev/pts/7
ssh workbox agentscan focus %1 --client-tty /dev/pts/7
```

If the desktop does not know a client tty, the bare action commands remain
valid best-effort commands, but failures or surprising focus targets should be
treated as remote tmux client-targeting problems rather than desktop-side
scanner problems.

## Remote Compatibility

Remote discovery should start with a cheap command that proves the binary is
present and speaks JSON:

```sh
ssh workbox agentscan daemon status --format json
```

If that succeeds and reports non-null `protocol_version` and
`snapshot_schema_version` values, validate exact compatibility before starting
the long-lived subscription process. The current compatible values are
`protocol_version=1` and `snapshot_schema_version=5`.

If the daemon is not running, this command reports the normal not-running JSON
shape without a live daemon protocol/schema to validate. Normal remote
consumers may then let `agentscan subscribe --format json` auto-start the
daemon according to the remote host's platform policy.

The subscribe command validates the socket protocol internally: a successful
bootstrap `snapshot` frame implies wire protocol compatibility, and consumers
should validate the exposed `snapshot.schema_version`. Incompatible handshakes
surface as a `fatal` frame or non-zero command failure.

When a scripted or preview-only flow must not start a daemon, pass
`--no-auto-start` or set `AGENTSCAN_NO_AUTO_START=1` inside the remote command
environment. The desktop app itself runs latch-only: it passes
`--no-auto-start` on every supervised subscribe re-arm and every hotkeys
invocation, and only the explicit user "Start agentscan" action auto-starts a
daemon (see `docs/adr/desktop-latch-only-daemon-launch.md`).

## Expected Failures

| Failure | Source | Desktop handling |
|---------|--------|------------------|
| SSH host, network, or auth failure | SSH process exit/stderr | Show connection failure and keep local UI state unchanged |
| Missing remote binary | SSH exit status/stderr such as `agentscan: command not found` | Show install/configuration guidance for that host |
| Incompatible protocol or snapshot schema | non-zero `daemon status` stderr, returned status JSON, or `fatal` subscribe frame | Ask the user to upgrade `agentscan` on the target host |
| Invalid JSON or unexpected stdout | command stdout/stderr | Treat the remote command as incompatible or misconfigured and surface a short output sample |
| Daemon auto-start refusal | `fatal`/offline subscribe frame or daemon status message | Show the remote `agentscan daemon run` / signing / opt-out guidance from the payload |
| tmux missing or gone | daemon status, subscribe offline/fatal frame, or command stderr | Show remote tmux availability guidance; do not fall back to desktop-side scanning |
| Focus or hotkey target gone or client tty unavailable | non-zero action exit or command error text | Refresh picker state from the shared commands and report the stale target or client-targeting failure |

## Remote Smoke Plan

1. Run `ssh workbox agentscan daemon status --format json` and confirm JSON
   parses. If protocol/schema versions are present, confirm they are
   compatible; if the daemon is not running, confirm the not-running state/error
   guidance is visible and defer compatibility validation to subscription
   startup.
2. Run `ssh workbox agentscan hotkeys --format json` and render picker rows
   from the returned command output without local tmux access.
3. Start `ssh workbox agentscan subscribe --format json`, read the bootstrap
   `snapshot` frame, then change a remote agent pane title or metadata and
   confirm a later `snapshot` frame arrives.
4. Invoke `ssh workbox agentscan hotkey <key> --client-tty <tty>` or
   `ssh workbox agentscan focus <pane_id> --client-tty <tty>` when a target
   remote client tty is known. Otherwise invoke the bare command as a
   best-effort fallback. Verify failures are reported from the command result
   rather than inferred by the desktop app.

Remote install/bootstrap UX is a follow-up product concern. The stable contract
for this project remains command execution over SSH using the documented JSON
and action surfaces.
