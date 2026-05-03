# AUR-177 Issue Plan: Move TUI to Live Socket Subscription

## Scope

Move `agentscan tui` from cache bootstrap plus cache mtime polling to a daemon
socket subscription:

- bootstrap from the first daemon subscription snapshot
- keep rendering live snapshot updates from the socket
- route keyboard, resize, and subscription events through one event loop
- show connection state in the TUI footer
- preserve the last known snapshot when the socket goes offline
- reconnect with socket-only backoff after ordinary post-bootstrap read errors

## Non-Goals

- Do not change the daemon wire protocol unless implementation exposes a gap.
- Do not add direct tmux or cache fallback for the final TUI.
- Do not make `agentscan tui` machine-readable or add TUI JSON/TSV output.
- Do not change provider classification, pane labels, or status heuristics.
- Do not remove the remaining cache commands; cache cleanup belongs to later
  milestone work.
- Do not push changes; the milestone remains local until complete.

## Implementation Outline

1. Replace TUI cache loading with a subscription client.
   - Add a TUI-facing daemon subscription helper, likely in `daemon` or a small
     `tui` submodule, that connects to `ipc::ClientMode::Subscribe`.
   - The final TUI bootstrap/reconnect path must never use
     `ipc::ClientMode::Snapshot`, `cache::load_snapshot`, or
     `scanner::snapshot_from_tmux`.
   - Send the normal hello frame and validate `hello_ack` protocol/schema before
     trusting any snapshot, matching the one-shot socket validation behavior.
   - Treat the first valid snapshot as bootstrap.
   - Validate every received snapshot with `cache::validate_snapshot(&snapshot, None)`.
   - Apply `cache::filter_snapshot(&mut snapshot, include_all)` inside the TUI
     update path after receiving a snapshot.
   - Classify pre-bootstrap responses deliberately:
     - missing/refused socket can auto-start and continue connecting
     - `DaemonNotReady`, `ServerBusy`, and `SubscriberLimitReached` are retryable
       connecting states
     - startup failure, protocol/schema mismatch, and invalid bootstrap snapshot
       are fatal unavailable states
     - `ServerClosing` before bootstrap is a shutdown/offline state and should
       not start a replacement daemon in that TUI process

2. Split input and subscription reads into producers feeding one event loop.
   - Keep terminal setup and frame rendering in `tui::mod`.
   - Startup ordering must be: enter terminal, initialize `Connecting` state,
     draw the connecting frame, write the ready marker, then let the
     subscription worker perform any blocking auto-start/connect/readiness work.
   - Move blocking crossterm input reads to an input thread that sends typed
     events such as key, resize, and input error through an `mpsc` channel.
   - Move socket subscription reads to a subscription thread that sends typed
     events such as connecting, connected/bootstrap snapshot, snapshot update,
     offline, explicit shutdown, and fatal setup error.
   - The main TUI loop should own `TuiState`, process those events, and redraw
     only after state-affecting events.
   - Add explicit cancellation through an atomic flag or channel close. Close
     keys should stop reconnect attempts, drop the socket, and prevent worker
     threads from producing events into a dead loop.
   - Use latest-snapshot-wins behavior for subscription events so slow rendering
     does not accumulate an unbounded queue of stale snapshots.

3. Model connection state explicitly in `TuiState`.
   - Add states such as `Connecting`, `Connected`, `Offline`, and
     `Shutdown`.
   - On startup, render a connecting frame before bootstrap and write the ready
     marker only after the first draw.
   - After bootstrap, preserve current panes across ordinary EOF/read errors and
     show an offline indicator/message with the reason class.
   - Treat `Unavailable { reason: ServerClosing, .. }` from the active
     subscription as the explicit daemon shutdown signal. Also treat a
     `Shutdown` frame as terminal if a future server emits one.
   - For explicit daemon shutdown, disable reconnect for this TUI process and
     keep the last known snapshot visible with shutdown/offline wording.
   - For pre-bootstrap incompatible/fatal setup errors, render an unavailable
     frame without suggesting `tui --refresh`.
   - Empty, connecting, unavailable, and normal pane-list frames should all
     reserve footer/indicator width consistently.

4. Implement socket-only reconnect backoff.
   - Ordinary post-bootstrap EOF/read errors should schedule reconnect attempts
     through the subscription worker or a small reconnect controller.
   - Reconnect must use daemon socket subscription only; it must not read cache,
     call `cache::load_snapshot`, call `scanner::snapshot_from_tmux`, or honor
     interactive refresh.
   - Post-bootstrap reconnect may auto-start a crashed/missing daemon using the
     same default auto-start policy, except when shutdown was explicit or
     `AGENTSCAN_NO_AUTO_START=1` disables auto-start.
   - Use a bounded/simple backoff suitable for tests, with injectable timings or
     small constants behind test helpers where needed.
   - Reset connection state to connected when a reconnect succeeds and a valid
     snapshot arrives.
   - Invalid post-bootstrap snapshots should preserve the last valid panes and
     surface an `invalid daemon snapshot` offline reason with capped retry/backoff
     instead of silently looping forever.
   - Expose enough backoff state in `TuiState` or footer text for deterministic
     tests, such as reconnecting/next retry pending, without relying only on
     wall-clock sleeps.

5. Remove interactive refresh from the TUI surface.
   - Remove `RefreshArgs` from `TuiArgs`.
   - Reject root `--refresh` routed to `tui` with existing unsupported-root-arg
     guidance.
   - Update error/help strings that currently recommend `agentscan tui --refresh`.
   - Keep root `--format` rejection and root `--no-auto-start` rejection.

6. Update rendering for connection indicators and narrow widths.
   - Keep the existing row rendering, paging, resize handling, stable key
     assignment, Ctrl-B passthrough, and missing-pane focus fallback.
   - Add footer status text for connecting, connected, offline/reconnecting, and
     shutdown.
   - Reserve width for the connection indicator and truncate the command/help
     text around it so footer content never overlaps at narrow terminal widths.
   - Use the existing display-width helpers for footer layout, including widths
     `0`, `1`, indicator-only, and long status text.
   - Update empty-state copy from cache terminology to socket/current snapshot
     terminology.

7. Align tests and docs narrowly.
   - Replace cache-polling TUI integration tests with socket subscription tests.
   - Update `docs/notes/interactive-tui.md` or delete cache-backed sections so
     it no longer preserves the old TUI behavior as current.
   - Update README/ROADMAP lines that still describe TUI cache polling or
     remaining TUI migration.
   - Leave full release/migration docs for the later docs issue.

## Edge Cases

- Missing daemon with TUI startup should use the same auto-start policy as normal
  daemon-backed consumers, unless `AGENTSCAN_NO_AUTO_START=1` is present.
- TUI has no `--no-auto-start` flag today; root `--no-auto-start` remains
  rejected for `tui`.
- TUI may still use tmux for focus and Ctrl-B interactions after a user
  selection; the no-direct-tmux rule applies to discovery/bootstrap/reconnect.
- Pre-bootstrap socket incompatibility should not clear the terminal into a
  misleading empty picker; show a clear unavailable frame.
- Post-bootstrap invalid snapshot/read error should keep the last valid panes
  visible with offline/reconnect state.
- `Unavailable::ServerClosing` and any future active-subscription `Shutdown`
  frame should not reconnect in the same TUI process.
- Reconnected snapshots should merge with existing row order using current
  `replace_panes` behavior.
- Selection after an offline state may still race with tmux pane removal; keep
  the existing tmux focus fallback and display-message behavior.
- The TUI must not touch a poisoned or stale cache file during startup or live
  updates.
- Tiny terminal widths must still keep every rendered line within terminal cell
  width.

## Test Plan

Focused tests:

- `cargo test tui`
- `cargo test daemon_socket`
- `cargo test --test daemon_integration tui`
- `cargo test --test daemon_integration display_popup`

Required coverage:

- TUI renders a connecting state before bootstrap.
- TUI bootstraps from daemon subscribe `hello_ack` plus first snapshot.
- TUI rerenders when the subscription sends a later snapshot.
- TUI does not read cache or direct tmux on startup/reconnect when cache is
  missing, stale, or poisoned; use fake socket tests or instrumentation to prove
  no bootstrap fallback is touched.
- TUI does not support `--refresh`, including root `-f tui`.
- Post-bootstrap EOF/read error preserves last panes and shows offline/reconnect
  state.
- Reconnect attempts use socket subscription and render a later valid snapshot.
- Explicit daemon shutdown via `Unavailable::ServerClosing` preserves last panes
  and disables reconnect.
- Pre-bootstrap retryable states (`DaemonNotReady`, `ServerBusy`,
  `SubscriberLimitReached`, missing/refused socket) render connecting/retry
  state instead of fatal unavailable.
- Pre-bootstrap fatal states (protocol/schema mismatch, startup failure, invalid
  bootstrap snapshot) render unavailable without refresh guidance.
- Post-bootstrap invalid snapshots show the last valid panes plus invalid-snapshot
  offline state with capped retry/backoff.
- Input events and subscription events can both update one TUI loop without
  blocking each other.
- Footer connection indicator lines fit within narrow terminal widths, including
  widths `0`, `1`, indicator-only, long status text, empty state, unavailable
  state, and pane-list state.
- Existing selection, paging, resize, Ctrl-B passthrough, Ctrl-C/Esc close, and
  missing-pane focus fallback behavior remain covered.

Regression gates:

- `cargo fmt --all --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_arguments`
- `cargo test`

## Documentation Impact

Update only the docs made stale by this issue: the TUI becomes socket-backed,
has no interactive refresh path, no longer polls cache mtime, and shows daemon
connection state. Full daemon migration/release docs remain later milestone
work.
