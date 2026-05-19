# Control-Mode Command Transport Evaluation

This note records the AUR-340 spike. It evaluates whether daemon refresh reads
should move from short-lived `tmux list-panes` commands onto the daemon's
existing long-lived tmux control-mode client.

## Current Subprocess Inventory

Daemon startup:

- `tmux -V` is read once through `tmux_version()` and cached in daemon snapshot
  source metadata.
- `tmux list-panes -a -F ...` builds the initial daemon snapshot before socket
  readiness is published.
- `tmux -C attach-session -t <session>` starts the long-lived control-mode
  client used for event subscription.

Daemon steady state:

- pane and title control-mode events trigger `tmux list-panes -t <pane> -F ...`
  through targeted pane refresh.
- window control-mode events trigger `tmux list-panes -t <window> -F ...`.
- session control-mode events trigger `tmux list-panes -t <session> -F ...`.
- reconcile interval, timeout, and full-resnapshot fallback trigger
  `tmux list-panes -a -F ...`.
- provider-scoped pane-output status fallback may trigger `tmux capture-pane`,
  but only for provider-known unknown-status candidates and now through a
  short-lived daemon-local cache.

Out of scope for this evaluation:

- direct `scan` and one-shot `--refresh` paths
- focus and display-message helpers
- metadata set/unset helpers
- daemon self-spawn lifecycle
- macOS `codesign` preflight

## Prototype Findings

tmux control-mode command responses are framed by `%begin`, `%end`, and
`%error` lines that include a frame id. A synchronous command transport can
parse the frame shape, collect output lines, and map `%error` frames to the
same missing-target behavior used by short-lived commands.

The hard part is not parsing. The hard part is ownership of the stream.
The daemon currently has one thread reading the control-mode client's stdout and
feeding every line into the event loop. Issuing read commands over the same
client would require the same stream reader to also act as a command broker.

The prototype tests intentionally cover these cases:

- command frame markers parse into stable ids
- output between matching `%begin` and `%end` can be collected
- `%error` can be surfaced as a command failure
- subscription/event lines seen before a command frame must be deferred back to
  the daemon event path
- nested or interleaved command frames must be treated as unsafe without a
  broker that owns correlation and ordering

## Risks

Response correlation:

- A control-mode command response is correlated by frame id, but the current
  daemon code does not own a request table or response dispatcher.

Ordering:

- A synchronous request made from the daemon loop would need to keep processing
  unrelated subscription events discovered while waiting for `%begin` / `%end`,
  or buffer and replay them without changing snapshot ordering semantics.

Timeouts:

- A command transport needs separate read timeouts from the existing startup
  subscription timeout. Timeout handling also has to decide whether the
  long-lived tmux client is poisoned after a partial command frame.

Missing targets:

- Existing short-lived commands rely on stderr matching for missing pane/window
  targets. A control-mode transport would need equivalent `%error` parsing and
  classification.

Lifecycle:

- If command reads share the event client, command transport failure could
  affect event subscription health. Keeping these independent is one reason the
  current subprocess boundary is simple.

## Decision

Do not move daemon refresh reads onto the existing control-mode client in this
slice.

The migration is viable only with an explicit control-mode broker that owns:

- the single stdout reader
- stdin command writes
- frame-id correlation
- event buffering or immediate event dispatch while command responses are
  pending
- command timeouts and poisoned-client recovery

That broker is daemon architecture work, so it should be handled with the
portless-inspired daemon redesign rather than as a narrow cleanup slice. The
current short-lived `tmux list-panes` reads remain acceptable after AUR-339's
pane-output throttling.
