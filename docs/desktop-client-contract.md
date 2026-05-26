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
agentscan hotkey q
agentscan focus %1
```

For a remote target, the desktop command runner executes the same commands
through the user's normal SSH configuration and authentication:

```sh
ssh workbox agentscan daemon status --format json
ssh workbox agentscan subscribe --format json
ssh workbox agentscan hotkeys --format json
ssh workbox agentscan hotkey q
ssh workbox agentscan focus %1
```

The desktop app treats SSH as transport around stdout, stderr, exit status, and
process cancellation. It must not tunnel the daemon Unix socket as the primary
remote design, parse remote tmux directly, or duplicate scanner logic in
desktop code.

## macOS Window Lifecycle

The macOS desktop app registers an app-global `CommandOrControl+Shift+A`
shortcut through Tauri's global shortcut plugin. This toggles the desktop
picker window, then the picker consumes the command contract above.

Tmux-prefix-originated launch remains a separate future integration path for
terminal workflows and should not be conflated with the app-global shortcut.
If macOS privacy behavior changes around global keyboard shortcuts, treat
Accessibility/Input Monitoring prompts as desktop packaging and signing
concerns; do not move shortcut handling into tmux or scanner code.

On summon, the macOS desktop picker is sized as a narrow sidebar and placed on
the work area of the display containing the cursor. If cursor or monitor lookup
fails, the app falls back to the primary display and still shows/focuses the
picker.

## Live State

The local desktop app keeps one supervised `agentscan subscribe --format json`
process for live state. Snapshot frames trigger a picker-row refresh through
`agentscan hotkeys --format json`, preserving the shared CLI picker contract
instead of reimplementing key assignment or display shaping in desktop code.

When the subscription exits unexpectedly, the UI keeps the last successful
snapshot visible while reconnecting and uses:

```sh
agentscan daemon status --format json
```

for lightweight diagnostics.

### Active-pane indicator (schema 5+)

Each picker row carries `is_active`, and each snapshot pane carries
`tmux.pane_active` and `tmux.window_active`. `is_active` is the derived
`pane_active && window_active`: the active pane of the active window — i.e. the
currently-focused pane. Clients should highlight the row whose `is_active` is
true.

Caveat: with multiple attached tmux sessions, `is_active` can hold for one pane
per attached session. Disambiguating "the pane the most-recently-active client
is looking at" is left to the client (e.g. via the focus client-tty it already
tracks); the backend reports tmux's raw layered state without collapsing it to a
single global active pane.

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
ssh workbox agentscan hotkey q --client-tty /dev/pts/7
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
environment.

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
