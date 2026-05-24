# Desktop App

The desktop app is a macOS-first Tauri shell over the installed `agentscan`
CLI. It does not link scanner internals, parse tmux output, classify providers,
or connect to the daemon socket directly.

Use this document for desktop operation and troubleshooting. Use
`docs/desktop-client-contract.md` for the command/SSH contract and
`docs/desktop-release-smoke.md` for signed local builds.

## Local Profile

The built-in local profile runs `agentscan` directly on the Mac running the app.
It preflights the configured binary with:

```sh
agentscan --version
```

The default binary path is `agentscan`. If the GUI environment cannot find the
intended CLI, set the local profile binary path in desktop settings.

Local profile commands use the shared CLI contract:

```sh
agentscan daemon status --format json
agentscan subscribe --format json
agentscan hotkeys --format json
agentscan focus <pane-id>
```

The desktop app keeps one supervised `agentscan subscribe --format json`
process for live state. Snapshot frames trigger a picker-row refresh through
`agentscan hotkeys --format json` so key assignment and row shaping remain
owned by the CLI.

## SSH Profiles

SSH profiles run the same command arguments through the user's normal SSH
configuration and authentication:

```sh
ssh workbox agentscan subscribe --format json
ssh workbox agentscan hotkeys --format json
ssh workbox agentscan focus <pane-id>
```

The remote host owns tmux, the daemon, provider classification, picker rows,
and focus semantics. The desktop app owns process supervision, stdout/stderr,
exit status, cancellation, rendering, and error presentation.

For non-default remote tmux servers, set both environment variables in the SSH
profile:

```sh
AGENTSCAN_TMUX_SOCKET=/tmp/tmux-501/custom
AGENTSCAN_SOCKET_PATH=$HOME/.local/state/agentscan/custom.sock
```

Those values keep the remote daemon socket paired with the intended remote tmux
server.

## Picker Behavior

The picker renders rows from `agentscan hotkeys --format json`. Search/filter
is client-side only: it filters returned rows without changing the shared
picker JSON contract, provider inference, status inference, or key assignment.

Selection and activation delegate to the CLI:

```sh
agentscan focus <pane-id>
```

When a remote SSH profile knows the intended tmux client, it can pass an
explicit client tty:

```sh
agentscan focus <pane-id> --client-tty /dev/pts/7
```

Without a known client tty, focus is best effort through the CLI's attached
client fallback.

## Window And Hotkey

The macOS desktop app registers `CommandOrControl+Shift+A` as an app-global
shortcut through Tauri's global shortcut plugin.

On summon, the picker is sized as a narrow sidebar and placed on the work area
of the display containing the cursor. If cursor or monitor lookup fails, it
falls back to the primary display and still shows/focuses the picker.

This is desktop window lifecycle behavior. Picker data and focus actions still
flow through `agentscan` command surfaces.

## Debug Log

The desktop debug log records command names, outcomes, errors, and stream
events. It should show environment variable names/counts rather than dumping
values into routine diagnostics.

Use the debug log to distinguish:

- local binary path or GUI environment problems;
- daemon readiness or auto-start refusal;
- invalid JSON or incompatible CLI output;
- SSH auth/network failures;
- missing remote `agentscan` binaries;
- remote tmux availability problems;
- stale pane or client-tty focus failures.

## Related Docs

- `docs/desktop-client-contract.md`: local/SSH command contract and failure
  surfaces.
- `docs/desktop-platform-posture.md`: macOS-first platform posture and future
  Linux/Windows seams.
- `docs/desktop-release-smoke.md`: signed local app builds and smoke checklist.
- `docs/integration.md`: stable automation and wrapper metadata contracts.
