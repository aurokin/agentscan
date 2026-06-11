# ADR: Desktop App Latch-Only Daemon Launch

Status: accepted
Date: 2026-06-01

## Scope

This decision applies **only to the desktop app** (`desktop/`, the Tauri 2
client). It does **not** change the terminal `agentscan tui` or any one-shot
daemon-backed CLI command. Those keep their existing daemon auto-start behavior
(see [Relationship To Other ADRs](#relationship-to-other-adrs)). When this ADR
says "the dock never starts a daemon itself," it is a statement about the
desktop client process, not about `agentscan` as a whole.

## Context

The desktop app consumes the live picker by running `agentscan subscribe
--format json` through a runner (`LocalRunner` / `SshRunner`) and folding the
JSON-Lines events into connection state. Like the other daemon-backed consumers,
`subscribe` will **auto-start a daemon** when none is reachable unless it is
passed `--no-auto-start` (the flag is defined in `src/app/cli.rs` and
honored by `subscribe` in `src/app/commands.rs`; the env opt-out is
`AGENTSCAN_NO_AUTO_START=1`).

The earlier desktop spike leaned on that implicit auto-start: launching the app
with no daemon running would spawn one as a side effect of the first
subscription. That is convenient, but it makes the desktop a *second* actor that
can create daemons, in addition to the user and the TUI/CLI.

We are now migrating the desktop's connection handling onto a central,
observable Effect lifecycle (the "connection-lifecycle slice" — see
`desktop/src/effect/`). The lifecycle owns epoch fencing, the reconnect/latch
`Schedule`, runner-gating, and the supervisor fiber. For that migration to be
debuggable, the desktop has to be a **pure observer** of daemon lifecycle, with
exactly one explicit place a daemon can be born.

## Decision

The desktop app is **latch-only**: it attaches to an already-running daemon and
**never starts one except when the user explicitly asks** — not on app launch,
not on reconnect, not on `↻` refresh, not on any internal retry. The single
action that may start a daemon is the explicit **"Start agentscan"** button in
the dock.

Concretely, in the desktop client:

- **App launch** with no daemon reachable → the dock shows a **"No daemon"**
  banner with a **"Start agentscan"** button and keeps slow-polling to latch
  automatically if a daemon appears. It does **not** auto-start one. This is the
  behavior change from the earlier spike, which auto-started at launch.
- **Daemon closes while latched** (ServerClosing, tmux restart, socket
  superseded) → auto re-arm with backoff, latching the moment any daemon is
  reachable again. Banner shows **Reconnecting…**. No spawn.
- **`↻` Refresh** → re-arms the live subscription (a real reconnect), latch-only.
- **Genuinely fatal** (binary missing, SSH auth, bad config, or an explicit-Start
  *refusal* such as a macOS codesign/trust failure) → show the error with **both**
  a **"Start agentscan"** button and a latch-only **Reconnect** button. Because a
  fatal can be a Start refusal whose fix is to retry Start once resolved, the
  Start affordance (start-or-latch — strictly more capable) is kept as the
  recovery path alongside the latch-only Reconnect. There is still no *automatic*
  spawn: the only thing that can spawn a daemon is the user pressing "Start
  agentscan".
- **"Start agentscan"** → the *only* action that subscribes with auto-start
  enabled, and even then only for the first attempt of that worker (see below).

### Implementation

- `start_live_picker(.., auto_start: bool)` (`desktop/src-tauri/src/lib.rs`)
  threads an explicit `auto_start` flag from the frontend.
- `subscribe_args(auto_start)` (`lib.rs`) appends `--no-auto-start` whenever
  `auto_start` is `false`.
- The live worker is **single-shot**: it runs one `subscribe` per epoch with the
  `auto_start` it was handed, and never retries internally (AUR-517 removed the
  in-worker retry loop and its `auto_start_for_attempt` guard). Reconnect is owned by
  the layers that can observe it: the `agentscan subscribe` CLI self-recovers
  mid-stream transient drops in its own loop, and on a clean daemon loss or an
  abnormal subscribe-child death the worker emits a terminal frame that the
  `LiveConnection` service re-arms — **always with a fresh epoch and
  `autoStart: false`** (`first && target.autoStart`, `LiveConnection.ts`). So only the
  *first* subscribe of an explicit "Start agentscan" can auto-start; every reconnect
  latches. The latch-on-retry invariant therefore now lives in the TypeScript service,
  not a Rust guard inside a worker loop the service could not observe.
- The companion picker-row fetch latches too: `hotkeys_args()` (`lib.rs`)
  **always** includes `--no-auto-start`. The worker re-derives rows by running
  `agentscan hotkeys` on every subscribe snapshot; because `hotkeys` is itself a
  daemon-backed consumer that would auto-start by default, omitting the flag would
  let a daemon that exited between the snapshot and the row fetch be silently
  replaced. Latching the row fetch closes that path, so a mid-stream row-fetch
  failure degrades to a normal reconnect instead of an implicit spawn.
- On the TypeScript side, the Effect `LiveConnection` service
  (`desktop/src/effect/LiveConnection.ts`) drives launch, reconnect, refresh, and
  latch with `autoStart: false`. Only the "Start agentscan" action calls through
  with `autoStart: true`.

Mechanically, "Start agentscan" does **not** invoke `agentscan daemon start`. It
runs the same `subscribe` command with auto-start *left enabled* (no
`--no-auto-start`), so the daemon is born through `subscribe`'s ordinary
one-shot-consumer auto-start path — and therefore through the same macOS
signed-only trust preflight the auto-start ADR governs. "Latch-only" thus means
"leave auto-start off on every path except this one user action," not a switch to
an explicit `daemon start` exec.

The settings webview never reads the connection atoms, so it never starts a
subscription and never starts a daemon.

## Rationale

The driving reason is **debuggability during the Effect lifecycle migration**,
and it is intentional and specific:

1. **One attributable spawn path.** With latch-only, a new daemon process can
   only originate from a single, user-initiated action. When debugging the
   connection lifecycle (epoch supersession, the latch/reconnect `Schedule`,
   runner-gating, `noDaemon` vs `fatal` classification), we never have to ask
   "did the dock spawn this daemon, or did the user, or the TUI?" Implicit
   launch-time auto-start would race the supervisor fiber and obscure which layer
   created a daemon.
2. **Observed state reflects a real daemon.** Because every reconnect/refresh/
   retry latches onto an *already-running* daemon, the connection state the dock
   renders reflects that daemon's actual lifecycle. This is what lets us
   reproduce and reason about `ServerClosing → reconnecting → noDaemon`
   transitions deterministically instead of watching the dock paper over a gap by
   conjuring a fresh daemon.
3. **No spawn multiplication during development.** The desktop runs two webviews
   and, under React StrictMode / Vite HMR, can double-mount and reload. An
   implicit auto-start on subscription start previously produced duplicate or
   competing daemons that are painful to debug. Latch-only collapses that to zero
   implicit spawns.
4. **Smaller macOS launch-policy surface.** Self-launching a detached daemon on
   macOS has a fraught history of `AppleSystemPolicy` kernel panics (see the
   macOS auto-start ADR). Keeping the desktop out of the implicit-spawn business
   while we iterate confines daemon creation to the exact moments the user asked
   for it.

## Relationship To Other ADRs

- `docs/adr/macos-daemon-autostart-and-executable-assessment.md` governs *how* a
  daemon is allowed to be auto-started on macOS (signed-only detached start after
  parent-side trust preflight; ad-hoc/local binaries are refused). That ADR still
  applies to the TUI and one-shot CLI commands, **and** to the desktop's single
  "Start agentscan" path. This ADR is narrower: it says the desktop client does
  not take the implicit-auto-start path *at all* except via that explicit action.
- `docs/adr/desktop-shell-and-shared-client-contract.md` establishes that the
  desktop owns "reconnect policy" and displays daemon lifecycle without owning
  daemon internals. Latch-only is the concrete reconnect/launch policy that
  contract leaves to the desktop.

The TUI is explicitly out of scope: `agentscan tui` continues to use the same
signed-only detached auto-start policy as one-shot daemon-backed commands. This
ADR does not change that.

## Consequences

- The desktop never surprises the user (or the host) with a background daemon at
  launch; the user opts in once via "Start agentscan."
- The connection lifecycle is deterministic to debug: a daemon's existence is an
  input the dock observes, not a side effect the dock causes.
- A first-run user with no daemon sees "No daemon" + "Start agentscan" instead of
  an immediately-populated picker. This is a deliberate trade of one click for
  observability and is acceptable while the lifecycle work is in flight.

## Reversibility And Future

This is a deliberately reversible, desktop-scoped policy, **not** a claim that
desktop auto-start is inherently unsafe. Once the Effect connection lifecycle is
fully migrated and observable, a guarded **launch-time** auto-start could be
reintroduced (e.g. latch first, and if no daemon appears within a window, offer
or perform a single signed auto-start). That would be a one-line override at the
launch call site plus a product decision; the seam (`auto_start` threaded through
`start_live_picker`) already exists. Any such change must keep the macOS
signed-only preflight from the auto-start ADR and a hard user opt-out.

## Non-Goals

- This ADR does not change `agentscan tui` or any CLI auto-start behavior.
- It does not remove or weaken the `--no-auto-start` /
  `AGENTSCAN_NO_AUTO_START=1` opt-out for non-desktop consumers.
- It does not change the macOS signed-vs-ad-hoc trust boundary for the one
  remaining desktop spawn path ("Start agentscan").
