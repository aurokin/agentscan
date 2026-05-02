# Daemon Socket Migration

## Purpose

This document is the planning record for moving `agentscan` from a
file-cache-as-IPC model to a unix-socket subscription model with the daemon as
a hard requirement. It is meant to survive context resets across the multi-PR
refactor that implements it. Until phase 11 lands and this doc is converted
into a "current state" note, treat it as the source of truth for the in-flight
design.

## Status

Planning. No code from this plan has landed yet. The current branch still ships
the `agentscan popup` command, the default `cache-v1.json` cache file, and the
polling-based TUI loop described in `docs/notes/interactive-popup.md`.

## Goals

- Replace cache-file polling in the interactive UI with an event-driven
  subscription so updates render the moment the daemon publishes them.
- Make the daemon the single source of state. All consumers speak to it over a
  unix socket.
- Make the daemon a hard requirement, but auto-start it transparently so users
  do not have to wire it up as a service.
- Provide first-class daemon lifecycle commands (`start`, `stop`, `status`,
  `restart`).
- Rename the user-facing interactive command from `popup` to `tui`. The TUI is
  not necessarily hosted in a tmux popup; "popup" is a tmux primitive name, not
  ours.

## Non-goals (v1)

- Delta frames. The wire protocol carries full `SnapshotEnvelope` snapshots.
  Deltas are a future optimization once the baseline is stable.
- Idle-shutdown of the daemon. The daemon runs until explicit `stop`, kill, or
  reboot. Memory pressure has not been observed; revisit only on real reports.
- Multi-user defense in depth via `SO_PEERCRED` / `getpeereid`. v1 relies on
  filesystem permissions (parent dir `0700`, socket `0600`).
- Throttled cache writes. The cache is being removed entirely, not throttled.
- Windows support. macOS and Linux only, matching current platform priority.

## Architecture

```
┌──────────────────────┐      writes      ┌──────────────────┐
│   tmux control mode  │ ───────────────▶ │     daemon       │
└──────────────────────┘                  │  (single proc)   │
                                          │                  │
                                          │  ┌────────────┐  │
                                          │  │ snapshot   │  │
                                          │  │   state    │  │
                                          │  └─────┬──────┘  │
                                          │        │ broadcast│
                                          │  ┌─────▼──────┐  │   listen
                                          │  │  ipc::      │  │   accept
                                          │  │  server     │ ◀┼──── unix sock
                                          │  └─────┬──────┘  │
                                          └────────┼─────────┘
                                                   │
                                  per-client writer threads
                                                   │
                            ┌──────────────────────┼──────────────────────┐
                            ▼                      ▼                      ▼
                     ┌──────────────┐      ┌──────────────┐      ┌──────────────┐
                     │  agentscan   │      │  agentscan   │      │  agentscan   │
                     │     tui      │      │     list     │      │    focus     │
                     │ (long-lived) │      │ (one-shot)   │      │ (one-shot)   │
                     └──────────────┘      └──────────────┘      └──────────────┘
```

Target state after phase 9: the cache file is no longer a live transport. The
socket is the only live channel, even though old cache implementation code and
CLI cleanup may remain until phase 10. One-shot consumers connect, take a single
snapshot frame, disconnect.

Before phase 9, some phases intentionally dual-write the old cache file or keep
cache bootstrap fallback in place while socket clients are brought up. Those
temporary fallback paths are implementation scaffolding only; they are not part
of the final architecture and should be deleted as soon as their phase notes say
they are no longer needed.

## Wire protocol

JSON-Lines, `\n`-terminated, UTF-8. One frame per line.

### Frame types

```jsonc
// Sent first by the client on every new connection.
{"v":1,"type":"hello","schema_v":<CACHE_SCHEMA_VERSION>,"client_pid":67890,"mode":"snapshot"|"subscribe"}

// Sent by the daemon on accept of a compatible hello.
{"v":1,"type":"hello_ack","schema_v":<...>,"daemon_pid":12345,"daemon_started_at":"<rfc3339>","subscriber_count":3}

// Sent by the daemon: one-shot response, bootstrap snapshot, or live update.
{"v":1,"type":"snapshot","data":{<SnapshotEnvelope>}}

// Sent by the daemon to terminate a connection cleanly.
{"v":1,"type":"shutdown","reason":"sigterm"|"protocol_mismatch"|...,"client":{...},"daemon":{...}}
```

### Versioning

- `v` is the **frame protocol version** and the only wire-envelope version.
  Bump it when frame envelope shape or encoding rules change. A missing,
  unsupported, or non-integer `v` is a protocol mismatch. Do not add a second
  frame-version field inside `hello`; `{"v":1,"frame_v":2,...}` is invalid in
  v1 and must be rejected as `protocol_mismatch`.
- `schema_v` inside the `hello` / `hello_ack` is the negotiated inner snapshot
  schema version.
- `schema_v` reuses `CACHE_SCHEMA_VERSION` from the existing cache code during
  the migration. When `cache.rs` is deleted, move this constant with the
  snapshot-envelope helpers into `snapshot.rs`; the name may stay unchanged for
  compatibility with existing tests, but ownership is the snapshot contract, not
  the removed cache transport. Even though the cache file is being removed, the
  snapshot envelope itself keeps this field; it now governs the socket payload
  instead of the file.
- v1 requires an exact `schema_v == CACHE_SCHEMA_VERSION` match. A schema
  mismatch is a compatible frame protocol but an incompatible snapshot contract,
  so the daemon replies with `shutdown { reason: "schema_mismatch", ... }`
  without `hello_ack`. Clients should surface the same stale-daemon restart
  guidance as `protocol_mismatch`, but diagnostics and logs should distinguish
  frame-envelope mismatches from snapshot-schema mismatches.
- `client_pid` is self-reported and diagnostic only. It is logged with
  handshake failures but is not trusted for authorization or lifecycle control.
- `mode` is required in v1. `snapshot` means "send the current snapshot if ready
  and close"; one-shot commands (bare `agentscan`, `list`, `inspect`, `focus`,
  `snapshot`, `daemon status`) use it. `subscribe` means "send the bootstrap
  snapshot and keep sending updates"; the TUI uses it.
- `subscriber_count` in `hello_ack` is deterministic and counts only registered
  long-lived `subscribe` clients that have not yet failed a write or been swept
  from the registry. It is a registry count, not a real-time liveness probe; in
  a quiet tmux session, a disconnected subscriber may remain counted until the
  next publish or shutdown write observes it. For `subscribe`
  handshakes it includes the just-accepted connection after successful
  registration. For `snapshot` handshakes it excludes the current client because
  that client is not registered as a subscriber. It always excludes clients
  rejected before registration.

### Handshake (Option 1: client speaks first)

```
[connect]
client → daemon: hello { v, schema_v, mode }
daemon decides:
  if compatible:
    daemon → client: hello_ack { ... }
    if initial snapshot is ready:
      daemon → client: snapshot { ... }   // bootstrap / one-shot response
      if mode == "snapshot":
        [daemon closes]
      else:
        daemon → client: snapshot { ... } // live updates
        ...
    else:
      daemon → client: shutdown { reason: "daemon_not_ready", daemon: {...} }
      [daemon closes]
  if mismatch:
    daemon → client: shutdown { reason: "protocol_mismatch", client: {...}, daemon: {...} }
    [daemon closes]
  if schema mismatch:
    daemon → client: shutdown { reason: "schema_mismatch", client: {...}, daemon: {...} }
    [daemon closes]
```

The client is purely a writer once for the `hello`, then a reader. The daemon
does not read further from the client after the handshake. `snapshot` clients are
never added to the subscriber registry; the daemon sends `hello_ack`, then either
the current snapshot and EOF or a retryable `daemon_not_ready` shutdown. This
prevents repeated one-shot commands from accumulating dead subscriber mailboxes
in a quiet tmux session. `subscribe` clients are registered before `hello_ack`
once a current snapshot exists, receive the bootstrap snapshot, and then receive
later updates until shutdown or write failure. Compatible clients always receive
`hello_ack` before either a bootstrap snapshot or a retryable `daemon_not_ready`
shutdown. This lets `agentscan daemon status` report PID, uptime, and registered
subscriber count even during the daemon's short initial-snapshot startup window.
Clients with incompatible frame protocol or snapshot schema receive `shutdown
protocol_mismatch` or `shutdown schema_mismatch` without `hello_ack`. This
preserves the "server-push" property after a single negotiation round-trip.

### Why client-first

- The daemon logs every rejection with both versions and the diagnostic
  client PID. Critical for diagnosing the most likely real-world failure:
  user upgraded via `cargo install` but did not restart the daemon.
- Standard pattern (HTTP, TLS, gRPC, SSH).
- Keeps the door open for server-side policy on old clients in v2.

## Socket lifecycle

### Path resolution

In order:
1. `$AGENTSCAN_SOCKET_PATH` (test override, multi-instance hatch).
2. `$XDG_RUNTIME_DIR/agentscan/daemon.sock` (Linux, when set).
3. `$TMPDIR/agentscan-$UID/daemon.sock` (macOS — `$TMPDIR` is per-user).
4. macOS fallback when `$TMPDIR` is unusable or too long:
   `~/Library/Caches/agentscan/runtime/daemon.sock`.
5. Unix fallback: `$XDG_STATE_HOME/agentscan/runtime/daemon.sock` or
   `~/.local/state/agentscan/runtime/daemon.sock`.

Parent dir mode: `0700`. Socket file mode: `0600`.

`AGENTSCAN_SOCKET_PATH` overrides only the path, not the security model. The
implementation must create a missing parent directory with `0700`, and must
reject an existing parent directory that is not owned by the current uid or is
group/world-accessible. Tests that need a custom socket path should point at an
owned temporary directory with equivalent permissions.

Do not place the default socket directly in the old cache directory. Existing
installs may already have `$XDG_CACHE_HOME/agentscan` or `~/.cache/agentscan`
created by the cache writer with ordinary user-cache permissions such as
`0755`. The default Unix fallback therefore uses XDG state, not XDG cache, so
`agentscan` can create or chmod the runtime directory to `0700` without changing
the permissions of the broader cache directory. If the runtime directory already
exists and is owned by the current uid but has broader permissions, tighten it to
`0700`; if it is not owned by the current uid, reject it.

Every resolved socket path must fit within the platform Unix-domain socket path
limit (`SUN_LEN`, roughly 104 bytes on macOS). If a default-derived path is too
long, path resolution falls through to the next default path. If
`AGENTSCAN_SOCKET_PATH` is too long, fail with a clear error instead of silently
choosing a different socket. `$TMPDIR` on macOS commonly resolves to a long path
(`/var/folders/...`), so it must be checked before choosing
`$TMPDIR/agentscan-$UID/daemon.sock`.

### Single-instance enforcement

Each socket path has sidecar lifecycle files derived from the full socket
filename:

- lock: `<socket_path>.lock`
- log: `<socket_path>.log`

The default path therefore uses `daemon.sock.lock` and `daemon.sock.log`.
Deriving sidecars from the socket filename keeps `AGENTSCAN_SOCKET_PATH` usable
as a multi-instance hatch even when two test sockets live in the same parent
directory.

The lock file is held with `flock(LOCK_EX | LOCK_NB)` for the lifetime of the
daemon process. The lock file contents are written on acquisition and are what
`daemon stop` reads:

```json
{"pid":12345,"started_at":"<rfc3339>","exe":"/path/to/agentscan"}
```

`pid` is the process to signal. `started_at` is the daemon wall-clock start time
and must match `daemon_started_at` from a compatible socket `hello_ack`; it is
not a kernel process start time. `exe` is diagnostic only. These fields reduce
PID-reuse risk when the daemon can answer the socket handshake, but they are not
a replacement for the held lock as the lifecycle authority. During the
migration, `stop` may also encounter a legacy lock file containing only a PID;
it may send SIGTERM to that PID while the lock is held, but it must not use
SIGKILL without a fresh compatible socket handshake confirming the same daemon
PID and start time.

Daemon startup:

```
1. open the lock sidecar without truncating it
2. acquire flock(lock sidecar, EX|NB)        # exits 0 silently on contention
3. truncate the lock sidecar and write daemon identity JSON
4. validate any existing socket path with lstat; if it is a Unix socket, unlink
   it as stale now that we own the lock; if it is any other file type, reject
   with a clear error and leave it untouched
5. bind + listen on the socket path
6. start ipc listener
7. start the tmux control-mode child and install the subscription
8. take and publish initial tmux snapshot only after control-mode attach and
   subscription setup have succeeded
9. enter main loop
```

Daemon shutdown (`SIGTERM`, `SIGINT`, or `%exit` from tmux):

```
1. signal thread records shutdown reason and wakes the daemon loop
2. daemon loop observes shutdown request in normal control flow
3. broadcast { type: "shutdown", reason: "sigterm"|"sigint"|"tmux_exit" } to all subscribers
4. close listener
5. close stdin to the tmux control-mode child and wait briefly for it to exit
6. if the control-mode child does not exit, send SIGTERM and wait briefly; use
   SIGKILL only as a last cleanup fallback before daemon process exit
7. unlink the socket path
8. release flock (implicit on process exit)
9. exit
```

Crash recovery is automatic for leaked socket files: the next daemon unlinks the
stale Unix socket at startup step 4, after lock acquisition guarantees no live
owner. Startup must not unlink a regular file, directory, symlink target, FIFO,
or other non-socket path, even when the path came from `AGENTSCAN_SOCKET_PATH`.

Signal handling must not run cleanup directly from an async Unix signal
handler. Use a normal Rust thread with `sigwait`/`signal-hook` style delivery,
or another self-pipe/eventfd equivalent, and let the daemon loop perform all
locking, allocation, socket writes, and filesystem cleanup.

### Spawn race

Two concurrent auto-start invocations both spawn a daemon. The one that
acquires the flock first wins; the loser exits silently when step 2 fails. Both
clients then poll-connect to the surviving daemon. No coordination needed at
the consumer level.

## Daemon lifecycle commands

```
agentscan daemon run        # foreground loop. Internal entry point.
agentscan daemon start      # detached spawn of `run`. Idempotent.
agentscan daemon stop       # SIGTERM, wait, guarded SIGKILL fallback, cleanup.
agentscan daemon status     # connected? PID? uptime? registered subscriber count?
agentscan daemon restart    # stop + start.
```

`run` stays as the foreground entry point — what the daemon process actually
executes. `start` is the orchestration that spawns `run` detached. Auto-start
invokes `start` internally; there is no second daemonization implementation.
`start` is not fire-and-forget: after spawning `run`, it performs the same
readiness wait as normal auto-start consumers and returns success only after a
compatible socket handshake produces the first snapshot. If the child reports
`startup_failed`, exits before readiness, or times out before publishing an
initial snapshot, `start` exits non-zero and points at the daemon log path. If
another daemon is already running and can produce a snapshot, `start` exits 0
without spawning a replacement. If another daemon is reachable but incompatible,
`start` exits non-zero with `agentscan daemon restart` guidance.

`status` is a socket client, not a cache reader, and it never auto-starts the
daemon. If no daemon is reachable, it reports "not running" plus the expected
socket path and exits 0. If a daemon is reachable, `status` performs a
`snapshot`-mode handshake and reads `hello_ack` for daemon PID, start time, and
registered subscriber count. The status text must label the count as registered
subscriptions, for example `registered_subscribers: 2`, because it excludes the
current one-shot `daemon status` connection and is not a real-time liveness
probe. When a bootstrap snapshot follows, `status` also reports pane summary
fields. If the daemon instead sends
`shutdown { reason: "daemon_not_ready" }` after `hello_ack`, `status` still
reports the daemon lifecycle fields and marks snapshot state as initializing
rather than failing the command.

The old cache-health flag `daemon status --max-age-seconds` is removed with the
cache diagnostics surface. Socket status is about daemon reachability and
snapshot readiness, not persisted-cache freshness.

If the daemon cannot attach to tmux or cannot produce the initial snapshot, it
must not remain in a permanent `daemon_not_ready` state. It logs the startup
failure, records the server closing reason as `startup_failed`, sends
`shutdown { reason: "startup_failed" }` to compatible clients if the IPC server
is already accepting, unlinks the socket path during normal shutdown cleanup,
releases the lock, and exits non-zero. `daemon_not_ready` is reserved for the
bounded startup window while initial snapshot work is still in progress;
`startup_failed` is terminal for the current daemon process.

The handshake path must check server closing state before treating a missing
current snapshot as `daemon_not_ready`. If the server is closing with
`startup_failed` or another explicit reason, a compatible client receives
`hello_ack` followed by `shutdown { reason: <closing_reason> }`, without
subscriber registration. This covers clients that connect during the startup
failure window before any bootstrap snapshot has ever existed.

If `status` reaches a daemon but receives `protocol_mismatch` or
`schema_mismatch`, it reports the daemon as incompatible, prints the socket path
and `agentscan daemon restart` guidance, and exits non-zero. A reachable but
incompatible daemon is not "not running"; it is stale or from a different
client build and needs operator action.

### Detachment

Spawn pattern (using `nix` for `setsid`):

```rust
use std::os::unix::process::CommandExt;

let mut command = Command::new(current_exe()?);
command
    .args(["daemon", "run"])
    .stdin(Stdio::null())
    .stdout(open_log_for_append()?)
    .stderr(open_log_for_append()?);

unsafe {
    command.pre_exec(|| {
        nix::unistd::setsid()
            .map(|_| ())
            .map_err(std::io::Error::from)
    });
}

command.spawn()?;
```

`setsid()` makes the child a session leader, detaching it from the controlling
terminal so it survives terminal close. Single-fork is sufficient on macOS and
Linux; double-fork is reserved as a fallback if smoke tests reveal the
single-fork variant gets reaped on shell exit.

`pre_exec` is `unsafe`; keep the closure minimal and limited to async-signal-safe
work. If the implementation uses `libc::setsid()` instead of `nix`, it must check
for `-1` and return `std::io::Error::last_os_error()` on failure.

### Stop

```
1. try acquire the lock sidecar with LOCK_EX | LOCK_NB
   ├─ succeeds: no live daemon owns the lock; lstat the socket path and unlink
   │  it only if it is a Unix socket, leave non-sockets untouched with a clear
   │  error, release, exit 0
   └─ would block: a live process owns the lock; continue
2. read daemon identity from the lock sidecar contents
   ├─ valid JSON identity: continue with force-kill eligibility subject to later checks
   ├─ valid legacy PID-only contents: continue, but mark force-kill ineligible
   └─ missing/invalid identity: retry read + socket handshake briefly, then report "daemon starting or lock corrupt"
3. best-effort connect to the socket path and read hello_ack; if it succeeds and
   either `daemon_pid` differs from the lock PID or `daemon_started_at` differs
   from the lock `started_at`, refuse to signal and print a lock/socket identity
   mismatch error
4. immediately retry LOCK_EX | LOCK_NB on the lock sidecar
   ├─ succeeds: daemon exited during stop; lstat the socket path and unlink it
   │  only if it is a Unix socket, leave non-sockets untouched with a clear
   │  error, release, exit 0
   └─ would block: continue
5. verify the lock PID still exists with best-effort platform evidence
   (`kill(pid, 0)`, and `proc_pidpath` / `/proc/<pid>/exe` when available).
   This is enough for SIGTERM only. Do not compare the lock `started_at` to a
   kernel process start time; it is the daemon's own wall-clock start time.
6. SIGTERM(lock PID)
7. poll the socket path for disappearance and lock acquisition, timeout 5s
8. before SIGKILL fallback, retry LOCK_EX | LOCK_NB on the lock sidecar again
   ├─ succeeds: daemon exited after SIGTERM; lstat the socket path and unlink it
   │  only if it is a Unix socket, leave non-sockets untouched with a clear
   │  error, release, exit 0
   └─ would block: perform a fresh compatible socket handshake, then
      SIGKILL(lock PID) only if `hello_ack.daemon_pid` and
      `hello_ack.daemon_started_at` still match the lock identity; acquire lock
      before unlinking the socket path
9. if the fresh compatible handshake cannot be completed, refuse SIGKILL,
   report a stuck daemon with lock/socket/log paths and the PID that received
   SIGTERM, and exit non-zero
10. exit 0
```

`stop` is idempotent: if no daemon running, it exits 0 with "no daemon
running". It must re-check the lock immediately before sending any signal,
because the daemon can exit after the initial would-block result and before the
signal attempt. The held lock is the lifecycle authority, but the PID alone is
not enough identity to justify a forced kill. `stop` should still send SIGTERM
to an old daemon after an upgrade when the lock is held and the PID is valid,
even if the normal socket handshake would reject the new client; it must skip
SIGKILL unless a compatible socket handshake revalidates daemon PID and start
time immediately before the forced kill.
Because `run` can hold the lock before the PID write is visible, `stop` treats
an empty or unparsable lock file under a held lock as a transient startup state:
retry for a short bounded interval, then fail without signaling.
v1 does not add `daemon stop --force`; manual cleanup is required for a daemon
that remains lock-holding after SIGTERM but cannot complete a compatible
handshake.

Every `stop` cleanup path that removes a stale socket path uses the same
`lstat` guard as daemon startup: unlink a Unix socket only, report and leave any
regular file, directory, symlink, FIFO, or other non-socket path untouched. This
applies even when the path came from `AGENTSCAN_SOCKET_PATH`.

### Logs

`<socket_path>.log`, append-only, redirected stdout+stderr from the detached
child. Truncate on daemon startup if size exceeds 10MB. Proper rotation is a v2
follow-up if needed.

## Auto-start

Every daemon-backed consumer (`agentscan` without a subcommand, `tui`, `list`,
`inspect`, `focus`, `snapshot`) follows the same connect flow. One-shot
commands send `hello` with `mode:"snapshot"`; the TUI sends
`mode:"subscribe"`. The bare `agentscan` command is the default list flow, so it
must behave exactly like `agentscan list` for auto-start, `--no-auto-start`, and
`AGENTSCAN_NO_AUTO_START`.

```
1. attempt connect(socket) + handshake + first snapshot
   ├─ bootstrap snapshot arrives → use it
   ├─ hello_ack then shutdown { reason: "daemon_not_ready" } → retry until the
   │  readiness deadline; the daemon may already have been starting before this
   │  client arrived
   ├─ hello_ack then shutdown { reason: "startup_failed" } → error with log path
   ├─ protocol_mismatch / schema_mismatch → error with `agentscan daemon
   │  restart` guidance. For refresh-capable one-shot commands, also mention
   │  `--refresh`.
   └─ connect fails (ECONNREFUSED / ENOENT):
      ├─ if --no-auto-start or AGENTSCAN_NO_AUTO_START=1:
      │     error: "no daemon running at <path>. Start with `agentscan daemon
      │             start`." For refresh-capable one-shot commands, also
      │             mention `--refresh`.
      └─ else:
            run the same internal start helper used by `agentscan daemon start`
            once; if it returns non-zero, propagate that startup failure instead
            of replacing it with a generic socket timeout. If start succeeds,
            continue the same retry loop
2. poll connect(socket) + handshake + first snapshot every 50ms until the 5s
   readiness deadline
   ├─ bootstrap snapshot arrives → use it
   ├─ hello_ack then shutdown { reason: "daemon_not_ready" } → retry
   ├─ hello_ack then shutdown { reason: "startup_failed" } → error with log path
   ├─ protocol_mismatch / schema_mismatch → error as above
   └─ timeout → error: "daemon started or was already starting but did not
                publish an initial snapshot, see <log_path>"
```

Readiness is defined by receiving the first `snapshot`, not merely by the socket
file appearing. The daemon binds and accepts before the initial tmux snapshot is
available so clients can distinguish "process is starting" from "process failed
to start." During that window the server answers compatible handshakes with
`hello_ack` followed by `shutdown { reason: "daemon_not_ready" }`. Every normal
consumer retries that response until its readiness deadline, regardless of
whether this command invocation spawned the daemon or raced a daemon that was
already starting.

Auto-start must share the `daemon start` implementation path rather than
spawning a detached daemon through a separate code path. The internal helper
returns the same readiness outcomes as the CLI command: already-running success,
new-daemon success after first snapshot, incompatible-daemon error, terminal
`startup_failed`, child-exited-before-readiness, or timeout with the daemon log
path. Consumers only enter their post-start retry loop after that helper returns
success.

`startup_failed` is not retryable within the same command invocation. The daemon
has decided it cannot produce the initial snapshot and is exiting; clients
should surface the daemon log path and lifecycle guidance rather than spawning
another daemon immediately.

Intentional daemon shutdown is not retryable. When a consumer receives
`shutdown { reason: "sigterm"|"sigint"|"tmux_exit" }` from an already-connected
daemon, it must not auto-start a replacement in the same command invocation.
One-shot commands exit with daemon lifecycle guidance. The TUI exits if it has
not yet rendered a bootstrap snapshot; after a successful bootstrap it stays
open on the last-known state with the offline indicator and reconnect attempts
disabled until the user exits and restarts the TUI.

### Opt-out

- `--no-auto-start` flag on every consumer.
- `AGENTSCAN_NO_AUTO_START=1` environment variable.

CI and automation environments should set the env var. Default-on is right
for desktop use; opt-out is right for scripting.

### Bypass entirely

`--refresh` on `list`, `inspect`, `focus`, and `snapshot` performs a direct tmux
scan via the existing `scanner::snapshot_from_tmux()` path. It does not connect
to the daemon, does not auto-start one, and works in any environment that has
tmux. This is the documented escape hatch for ad-hoc and broken-daemon
scenarios.

`tui --refresh` is intentionally not supported in the final socket-backed TUI.
The TUI is an interactive live view, so a one-shot direct scan would create a
stale interactive surface. If the daemon cannot be reached or auto-started,
`agentscan tui` exits with daemon lifecycle guidance instead of falling back to
direct tmux scanning. During phases where the renamed TUI is still cache-backed
or has a temporary cache bootstrap fallback, the legacy refresh flag may remain
only as migration scaffolding; remove it when the first-socket-frame bootstrap
lands, no later than phase 9.

## Concurrency model in the daemon

```
main thread (existing tmux ctl-mode loop)        listener thread (new)
─────────────────────────────────────────        ─────────────────────
start listener                                  loop {
take initial snapshot                              accept() unix stream
publish(Arc<SnapshotEnvelope>)                     spawn writer thread for client
read tmux notification                          }
reconcile snapshot
publish(Arc<SnapshotEnvelope>)
loop

                                                     writer thread (per client, new)
                                                     ──────────────────────────────
                                                     read client hello with timeout
                                                     if mismatch: send shutdown, close
                                                     if mode == snapshot:
                                                       clone current snapshot under lock
                                                       send hello_ack
                                                       if no current snapshot: send
                                                         shutdown daemon_not_ready, close
                                                       else:
                                                         send snapshot, close
                                                     if mode == subscribe:
                                                       if no current snapshot:
                                                         send hello_ack
                                                         send shutdown daemon_not_ready, close
                                                       else:
                                                         register mailbox under lock
                                                         clone current snapshot under lock
                                                         send hello_ack
                                                         send first snapshot (bootstrap)
                                                         loop {
                                                           wait for mailbox frame
                                                           write_all to stream
                                                           on err: drop + cleanup
                                                         }
```

`ServerHandle` owns a short-held `Mutex<ServerState>` containing the current
snapshot, closing state, and subscriber registry. Daemon startup binds the
socket and starts the listener before taking the initial tmux snapshot, then
attaches the tmux control-mode child and installs the subscription. The first
snapshot is published only after that attach/subscription path has succeeded, so
`daemon start` readiness means both "snapshot available" and "future tmux
events are wired." Every later tmux update calls
`server.publish(Arc<SnapshotEnvelope>)`, which replaces the current snapshot and
then performs non-blocking fan-out to connected subscribers.

Client bootstrap uses that owned current snapshot. `snapshot` mode is handled
without registration:

1. writer validates the client `hello`
2. writer acquires the server state mutex
3. writer reads the current registered subscriber count for `hello_ack`
4. if server closing state is set, writer clones the closing reason, releases
   the mutex, sends `hello_ack`, sends `shutdown { reason: <closing_reason> }`,
   and closes
5. otherwise, writer clones the current snapshot under the server state lock, if
   one exists
6. writer releases the server state mutex
7. writer sends `hello_ack`
8. if no current snapshot was available, writer sends `shutdown` reason
   `daemon_not_ready` and closes
9. otherwise, writer sends the snapshot and closes the connection

`subscribe` mode registers a mailbox before `hello_ack` only after a current
snapshot exists:

1. writer validates the client `hello`
2. writer acquires the server state mutex
3. if server closing state is set, writer reads the current registered
   subscriber count, clones the closing reason, releases the mutex, sends
   `hello_ack`, sends `shutdown { reason: <closing_reason> }`, and closes
   without registering
4. if no current snapshot exists, writer reads the current registered subscriber
   count for `hello_ack`, releases the mutex, sends `hello_ack`, sends
   `shutdown { reason: "daemon_not_ready" }`, and closes without registering
5. if `MAX_SUBSCRIBERS` would be exceeded, writer reads the current registered
   subscriber count, releases the mutex, sends `hello_ack`, sends
   `shutdown { reason: "subscriber_limit" }`, and closes without registering
6. writer registers a mailbox, so `subscriber_count` in `hello_ack` includes
   this client
7. writer clones the current snapshot under the same server state lock
8. writer releases the server state mutex
9. writer sends `hello_ack`
10. writer sends the bootstrap `snapshot`
11. writer waits on the mailbox for later updates

If no current snapshot exists, `daemon_not_ready` is expected only during the
short startup window before the initial tmux snapshot is published. Auto-start
clients treat it as retryable until their startup deadline; ordinary clients
surface it as "daemon is still initializing."

If initial snapshot collection fails instead of merely taking time, the daemon
transitions to shutdown with reason `startup_failed` and exits. That reason is a
terminal startup failure, not a transient readiness state.

Writer threads must observe the server closing state before returning
`daemon_not_ready` for a missing current snapshot. Once startup has failed, new
compatible connections receive `hello_ack` followed by
`shutdown { reason: "startup_failed" }`; they are not registered as subscribers,
and they do not wait for a snapshot that will never be published.

Pre-handshake clients are bounded. The listener sets a short read timeout
(500ms target) for the initial `hello`; clients that do not send a complete
JSON-Lines `hello` frame in time are closed. The server also keeps a small
`MAX_PENDING_HANDSHAKES` cap so same-user processes cannot exhaust threads by
opening sockets and staying silent.

Post-handshake subscribers are also bounded. v1 should use a conservative
`MAX_SUBSCRIBERS` cap (64 target) because each accepted subscriber owns a writer
thread and mailbox. When the cap is reached, the daemon sends
`hello_ack` followed by `shutdown { reason: "subscriber_limit" }` after
validating the `hello` but before registration, then closes the connection.
Normal desktop use should be nowhere near this limit; hitting it is a same-user
misuse or leak signal worth surfacing clearly.

`server.publish(&Arc<SnapshotEnvelope>)` fan-out is non-blocking:

- Briefly holds `Mutex<ServerState>` to replace the current snapshot and clone
  the mailbox handles for currently registered subscribers.
- Each `ClientMailbox` is a tiny watch slot:
  `Mutex<MailboxState>` + `Condvar`, where `MailboxState` contains an optional
  outbound server frame (`snapshot` or `shutdown`) plus a closed flag.
- For each subscriber: briefly lock the slot, replace any queued snapshot with
  the newest `Arc`, notify the writer thread, and move on.
- "Latest wins" — the TUI wants current state, not historical state. Slow
  clients can miss intermediate snapshots but cannot block the tmux loop or
  accumulate unbounded memory.
- Writer threads send their bootstrap from the server-owned current snapshot,
  then wait on their mailbox, take the latest server frame, write it to the
  stream with a bounded write timeout, and mark themselves closed on write
  errors, write timeout, or after sending a shutdown frame. Closed mailboxes are
  removed from the registry on the next sweep.
- Subscriber cleanup must not depend solely on later snapshot publishes. Each
  subscriber needs an independent liveness path, such as a paired read/EOF
  monitor, periodic registry sweep, or another bounded heartbeat-free check that
  can retire disconnected clients in a quiet tmux session. Otherwise crashed
  TUI clients could occupy `MAX_SUBSCRIBERS` until the next tmux event.

`server.shutdown(reason)` uses the same mailbox registry. It sets each open
mailbox to `shutdown { reason }`, marks the server as closing, and notifies all
writer threads before the listener is closed. Shutdown delivery is best-effort:
healthy readers should receive the frame, but the daemon must not wait
indefinitely for a stalled client. Client socket writes use the same bounded
write timeout as normal snapshot delivery; crashes still rely on clients
observing EOF or read errors.

The publish fan-out cost is O(N) short mailbox updates + one `Arc` clone per
client. The envelope itself is allocated once per event (already happens for
the existing cache write). The main tmux loop is never blocked by slow or dead
clients beyond brief uncontended mailbox locks.

## TUI client state machine

```
                ┌──────────────────────────┐
                │  start: spawn key thread │
                │  + subscription thread   │
                └────────────┬─────────────┘
                             │
                             ▼
                ┌──────────────────────────┐  hello_ack +    ┌────────────────┐
                │  Connecting              │ ─bootstrap ───▶ │  Connected     │
                │  ("connecting…" frame +  │                 │  (live frames) │
                │   spinner)               │                 └────────┬───────┘
                └────────────┬─────────────┘                          │ EOF / err
                             │ first connect                          │
                             │ fails/exhausts                         │
                             ▼                                        │
                     exit with daemon error                            │
                                                                      │
                ┌──────────────────────────┐                          │
                │  Disconnected            │ ◀────────────────────────┘
                │  (last-known state +     │
                │   reconnect backoff)     │
                └──────────────────────────┘
```

- **Final bootstrap is socket-only.** After phase 9, the cache no longer exists;
  first frame comes from the daemon's `snapshot` after `hello_ack`. Locally this
  is sub-10ms.
- **No final initial cache read.** TUI startup is gated on the daemon. If the
  daemon fails to auto-start, the TUI exits with daemon lifecycle guidance and
  no direct-scan fallback.
- **Temporary phase 4 fallback.** During phases 4 through 8 only, the TUI may
  keep a cache bootstrap fallback so the subscription refactor can land before
  the cache transport is deleted. That fallback is migration scaffolding: it
  must be removed in phase 9 when the TUI switches to first-socket-frame
  bootstrap.
- **Reconnect backoff:** 100ms, 250ms, 500ms, 1s, 2s, 5s, capped at 5s. Reset
  on successful reconnect. Post-bootstrap reconnect attempts for EOF or read
  errors are socket reconnects only; they do not auto-start a replacement
  daemon. Explicit shutdown frames disable reconnect for the current TUI process,
  so operator shutdown and tmux-control-mode exit do not trigger reconnect.
- **Disconnected mode** keeps showing the last received envelope with the
  offline indicator. It is only reachable after at least one successful
  bootstrap snapshot. After phase 9, there is no cache fallback.
- **Operator shutdown leaves the current TUI offline.** EOF or read errors use
  socket-only reconnect backoff; explicit
  `shutdown { reason: "sigterm"|"sigint"|"tmux_exit" }` disables reconnect for
  the rest of this TUI process and keeps showing the last-known state with the
  offline indicator. This keeps `agentscan daemon stop` stopped and prevents
  tmux-control-mode exit from spawning a replacement daemon.

### Footer indicator

A single character flush-right on the existing first footer line. Color via
`crossterm::style`:

- `●` green: `Connected`
- `○` yellow: `Connecting` / `Reconnecting`
- `·` gray: `Disconnected` (after at least one prior `Connected` state)

The renderer reserves indicator width first, truncates the footer text to the
remaining width, then writes the indicator flush-right. It must not append the
indicator after filling the full terminal width.

### Threading

```rust
enum TuiEvent {
    Key(crossterm::event::Event),
    Snapshot(Arc<SnapshotEnvelope>),
    SubscriptionUp,
    SubscriptionDown { reason: String },
    KeyReaderFailed(String),
}
```

Threads:
- **Key reader** (`src/app/tui/key_reader.rs`): blocking `event::read()` →
  `TuiEvent::Key`.
- **Subscription** (`src/app/tui/subscription.rs`): owns connect → handshake →
  read loop → reconnect backoff. Sends `Snapshot`, `SubscriptionUp`,
  `SubscriptionDown`.
- **Main**: `recv_timeout(MAIN_TICK)` on a unified `crossbeam` or `std::mpsc`
  channel.

No mtime-poll thread, no cache-watch thread.

## Module layout

```
src/app/
├── ipc/
│   ├── mod.rs          // Frame enum, FRAME_PROTOCOL_VERSION, encode/decode
│   ├── path.rs         // socket path resolution + socket length guard
│   ├── server.rs       // ServerHandle, listener, broadcast, lock file, PID write
│   └── client.rs       // Subscriber, hello → frame iterator, mismatch handling
├── daemon.rs           // calls server.publish(...) after each snapshot update
├── snapshot.rs         // (renamed from cache.rs) survivors: CACHE_SCHEMA_VERSION,
│                       //   validate_snapshot, summarize_snapshot,
│                       //   sort_snapshot_panes, filter_snapshot, now_rfc3339
└── tui/                // (renamed from popup/)
    ├── mod.rs          // unified event loop
    ├── input.rs
    ├── render.rs       // includes footer indicator
    ├── state.rs
    ├── terminal_session.rs
    ├── subscription.rs // ipc::client wrapper, reconnect state machine
    └── key_reader.rs   // threaded crossterm reader
```

## Removed surface

- `src/app/cache.rs` — deleted. Survivors moved to `src/app/snapshot.rs`.
- `agentscan cache show` command — renamed to `agentscan snapshot`.
- `agentscan cache path` command — removed with the cache file.
- `agentscan cache validate` command — removed; socket health moves to
  `agentscan daemon status`, and snapshot schema validation remains internal
  plus test-covered.
- `agentscan daemon status --max-age-seconds` — removed with cache freshness
  diagnostics. `daemon status` reports live daemon reachability/readiness, not
  persisted-cache age.
- `AGENTSCAN_CACHE_PATH` env var.
- `cache_diagnostics`, `daemon_cache_status`, `daemon_status_reason`, and
  related plumbing — replaced by `agentscan daemon status`.
- mtime-polling loop in the TUI (`src/app/popup/mod.rs:49`).
- `KEY_POLL_INTERVAL` constant.
- `agentscan popup` command (renamed to `agentscan tui`, no alias).
- `--refresh` on the interactive command. Direct tmux refresh is for one-shot
  commands only, not the live TUI.
- `AGENTSCAN_POPUP_READY_PATH` and `AGENTSCAN_POPUP_DONE_PATH` env vars
  (renamed).
- `--refresh` on `agentscan scan`. `scan` is already the direct tmux snapshot
  command, so the flag is redundant; today it only changes whether the direct
  snapshot is also written through the cache path. Remove it before deleting
  cache write helpers.

## Renamed surface

| Before | After |
|---|---|
| `agentscan popup` | `agentscan tui` |
| `agentscan cache show` | `agentscan snapshot` |
| `src/app/popup/` | `src/app/tui/` |
| `src/app/cache.rs` (most of) | `src/app/snapshot.rs` |
| `AGENTSCAN_POPUP_READY_PATH` | `AGENTSCAN_TUI_READY_PATH` |
| `AGENTSCAN_POPUP_DONE_PATH` | `AGENTSCAN_TUI_DONE_PATH` |
| `AGENTSCAN_RUN_DISPLAY_POPUP_TESTS` | `AGENTSCAN_RUN_TMUX_POPUP_TESTS` |
| `AGENTSCAN_CACHE_PATH` | `AGENTSCAN_SOCKET_PATH` |
| `PopupArgs`, `PopupState`, `PopupFrame`, etc. | `TuiArgs`, `TuiState`, `TuiFrame`, etc. |
| `command_popup`, `run_popup_loop`, etc. | `command_tui`, `run_tui_loop`, etc. |
| `start_agentscan_display_popup` (test harness) | `start_tui_in_tmux_popup` |
| `DisplayPopupHandle` (test harness) | `TmuxPopupHandle` |

## Kept (deliberately)

- `tmux display-popup` references in the test harness and docs. This is
  tmux's primitive name, not ours. The TUI is typically launched inside one
  via `tmux display-popup -E 'agentscan tui'`.
- `agentscan scan`. It remains the direct tmux snapshot command for debugging,
  benchmarks, and recovery. It never connects to or auto-starts the daemon, and
  after the CLI cleanup it has no `--refresh` flag because every `scan`
  invocation is already fresh.
- `--refresh` flag on refresh-capable one-shot consumers. Direct tmux scan,
  daemon-free.
- Pane filtering and sorting helpers from the old cache module. They operate
  on snapshots in memory and remain useful regardless of transport.

## Phasing

Each phase is one (or a small number of) reviewable commits. Each leaves the
build green.

| # | Phase | Notes |
|---|---|---|
| 0 | Rename `popup` → `tui` | Intentional breaking CLI change. No alias. Preserve temporary interactive `--refresh` only while the TUI still depends on cache bootstrap. |
| 1 | `ipc` scaffolding | `ipc::path`, frame types, encode/decode, unit tests. Frame decode/validation tests must cover unknown top-level wire fields such as `frame_v`, malformed or missing `v`, missing `mode`, and unknown `mode`; serde defaults must not silently accept invalid v1 envelopes. |
| 2 | `ipc::server` + lock file + dual-write | Daemon binds socket and broadcasts; still writes cache as a transport fallback. |
| 3 | `ipc::client` + handshake | Synthetic-server tests; mismatch path covered. |
| 4 | TUI subscription + footer indicator | Threaded crossterm; cache becomes bootstrap-only fallback. Keep any interactive `--refresh` support clearly marked temporary. |
| 5 | Daemon lifecycle commands | `start`, `stop`, `status`, `restart`. PID in lock file. Signal handler. Add `nix` dependency for `setsid`/signals if implementation follows this plan. |
| 6 | Auto-start plumbing | Shared connect/spawn/retry helper, `--no-auto-start`, `AGENTSCAN_NO_AUTO_START`, and any socket-client wiring available before one-shots migrate. Race-safe. |
| 7 | Migrate one-shots | Bare `agentscan`, `list`, `inspect`, `focus`, `cache show` → `snapshot`. All socket-based and using the phase 6 auto-start helper. Remove redundant `scan --refresh` so `scan` no longer routes through cache-writing helpers. Start converting command integration tests away from seeded cache files. |
| 8 | Test harness rewrite | Cache fixtures → daemon-spawning fixtures keyed off `AGENTSCAN_SOCKET_PATH`. Existing cache-dependent tests must be converted, deleted, or explicitly kept behind the temporary dual-write path before phase 9 starts. |
| 9 | Stop writing cache | Daemon no longer calls `write_snapshot_to_cache`. TUI bootstrap switches to first-socket-frame and drops interactive `--refresh`. Build stays green because the harness no longer depends on cache writes. |
| 10 | Delete `cache.rs` | Move survivors to `snapshot.rs`. Drop `cache path`, `cache validate`, and `AGENTSCAN_CACHE_PATH`. Remove any remaining cache-only tests in the same change. |
| 11 | Docs + ROADMAP | Breaking-change notes; convert this doc into a current-state record. |

This is not a reversible user-facing rollout. Phase 0 and phase 7 intentionally
change command names before the cache is removed, and no compatibility aliases
are planned. Phases before phase 9 keep the cache transport available only to
reduce implementation risk while socket clients are brought up; they are not a
promise that the CLI surface can be rolled back without user impact.

Phase 9 ("stop writing cache") is the transport point of no return: after it
lands, the cache file is no longer an IPC fallback.

## Risks and mitigations

1. **Stale binary after `cargo install` upgrade.** Old daemon, new client.
   Mitigation: handshake mismatch path. Daemon logs the rejection; client
   prints `agentscan daemon restart` guidance and points refresh-capable
   commands at `--refresh` for direct tmux recovery.
2. **Daemon detachment quirks on macOS.** `launchd` reaps orphans oddly in
   some configurations. Mitigation: smoke test from a Terminal tab — spawn
   daemon, close tab, verify daemon survives and is reachable from a new
   shell. Add second fork only if smoke test fails.
3. **Unix socket-path length.** Long default paths, especially macOS
   `/var/folders/...` paths, can exceed `SUN_LEN`. Mitigation: explicit length
   check in path resolution, fallback to the next default path, and a clear
   error for an explicit `AGENTSCAN_SOCKET_PATH` that is too long.
4. **Test runtime increase.** Spawning a daemon process per test is slower
   than dropping a JSON file in place. Mitigation: in-process daemon harness
   for unit-ish tests; real subprocess only for end-to-end. New
   `DaemonFixture` helper.
5. **`agentscan list` without daemon now hard-errors.** Anyone running it
   ad-hoc out of habit will see the error. Mitigation: clear error message
   pointing at `daemon start` and `--refresh`. Auto-start defaults make this
   rare in interactive use.
6. **TUI cold start now bound by daemon responsiveness.** Hung tmux
   control-mode or initial snapshot collection can leave the TUI at
   "connecting…". Mitigation: readiness is defined by receiving the first
   `snapshot`; consumers retry `daemon_not_ready` until the 5s readiness
   deadline, then fail clearly with the daemon log path.
7. **Single-instance enforcement is a behavior change.** Today nothing
   prevents two daemons running. Mitigation: release notes line.
8. **Surprise auto-spawn for one-off CLI use.** Running `agentscan list` once
   leaves a daemon running indefinitely. Mitigation: `--no-auto-start` and
   the env opt-out exist; document that auto-start is the default for
   desktop use.
9. **`flock` on NFS.** `$TMPDIR` on a network mount could misbehave.
   Mitigation: documented platform assumption (local FS).
10. **Daemon log growth.** Truncate-on-startup-if-large is a hack. Mitigation:
    revisit if any user reports growth.

## Follow-ups (not in v1)

- Delta frames (`pane_updated`, `pane_removed`) instead of full snapshots.
- Idle shutdown of the daemon after N minutes with no subscribers and no
  recent tmux activity.
- `SO_PEERCRED` / `getpeereid` peer-uid check for defense in depth on shared
  hosts.
- Proper log rotation.
- `agentscan daemon status --subscribers` showing connected client PIDs.
- Telemetry on auto-start spawns and handshake rejections.

## Breaking-change notes (for release notes)

- `agentscan popup` is renamed to `agentscan tui`. No alias. Update tmux
  binds, shell aliases, and wrapper scripts.
- The interactive command no longer accepts `--refresh`; use `agentscan list
  --refresh`, `inspect --refresh`, `focus --refresh`, or `snapshot --refresh`
  for one-shot direct tmux recovery.
- `agentscan scan` no longer accepts `--refresh`; it is always a fresh direct
  tmux snapshot and never writes cache state.
- `agentscan cache show` is renamed to `agentscan snapshot`.
- `agentscan daemon status --max-age-seconds` is removed. `daemon status` no
  longer validates persisted-cache freshness because the cache file is gone.
- `agentscan daemon status` reports "not running" and exits 0 when no daemon is
  reachable. A reachable but incompatible daemon is still an error and exits
  non-zero with restart guidance.
- The persisted cache file (`cache-v1.json` by default) is no longer written.
  Anyone reading it directly must migrate to `agentscan snapshot --format json`
  (which connects to the daemon internally).
- `AGENTSCAN_CACHE_PATH` is removed. Use `AGENTSCAN_SOCKET_PATH` for socket
  overrides.
- `AGENTSCAN_POPUP_READY_PATH` and `AGENTSCAN_POPUP_DONE_PATH` are renamed to
  `AGENTSCAN_TUI_READY_PATH` and `AGENTSCAN_TUI_DONE_PATH`.
- The daemon is now required for bare `agentscan`, `list`, `inspect`, `tui`,
  `focus`, and `snapshot`. Auto-start is the default; opt out with
  `--no-auto-start` or `AGENTSCAN_NO_AUTO_START=1`. Use `--refresh` on
  refresh-capable one-shot commands for direct tmux scans without the daemon.
- Two concurrent daemons on the same socket path are no longer permitted.
  Subsequent invocations of `daemon run` exit silently while another holds
  the lock.

## Maintenance rule

Treat this file as the in-flight planning record for the migration. As phases
land, prefer to delete sections that have been fully realized rather than
duplicate them in the README, ROADMAP, or `docs/architecture.md`. After phase
11, this doc should be a thin "current state" reference or removed entirely
in favor of updates to the primary docs.
