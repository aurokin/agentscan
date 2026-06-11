import { Duration, Effect, Fiber, Queue, Ref, Stream, SubscriptionRef } from "effect";
import { TauriIpc, type IpcError } from "./TauriIpc";
import type {
  ConnectionStatus,
  DesktopRunnerSettings,
  LivePickerEnvelope,
  LivePickerEvent,
  LiveState,
} from "./types";

// Keep in sync with the seed key the old App.tsx used so epochs stay monotonic
// across a reload that restarts the JS while a Rust worker lingers.
const LIVE_EPOCH_STORAGE_KEY = "agentscan.liveEpochSeq";

// Backoff between re-arm attempts, injected as a service so tests can zero it out
// (keeping them event-driven, no wall-clock). A recoverable close (daemon
// restarted / socket superseded) retries quickly to re-latch; a "no daemon" poll
// is slow — we are not starting one, just waiting to attach if the user brings one up.
export type LiveBackoff = {
  readonly recoverable: Duration.Duration;
  readonly noDaemon: Duration.Duration;
};

export class LiveConnectionConfig extends Effect.Service<LiveConnectionConfig>()(
  "desktop/LiveConnectionConfig",
  {
    succeed: {
      backoff: {
        recoverable: Duration.seconds(1),
        noDaemon: Duration.seconds(10),
      } as LiveBackoff,
    },
  },
) {}

// A terminal frame ends the subscribe child. The desktop owns reconnect policy, so
// we classify what to do next rather than wedging:
//   Recoverable — re-arm (latch) after a short backoff
//   NoDaemon    — no daemon reachable in latch-only mode; offer "Start agentscan",
//                 keep slow-polling to auto-latch if one appears
//   Fatal       — unrecoverable (bad binary/config); stop and wait for a manual retry
type Terminal =
  | { _tag: "Recoverable"; message: string }
  | { _tag: "NoDaemon"; message: string }
  | { _tag: "Fatal"; message: string };

// A terminal frame plus whether the subscription ever reached "online" (a rows
// frame) before it ended. `online` separates a daemon that came up and was later
// lost from one that never connected — the discriminator runTarget uses to keep a
// post-connection latch-miss as NoDaemon instead of a Start refusal.
type ConsumeResult = { readonly terminal: Terminal; readonly online: boolean };

// The Rust side stamps "daemon auto-start is disabled: …" when a `--no-auto-start`
// subscribe finds no daemon. That is precisely the latch-miss we surface as a
// "Start agentscan" prompt rather than an error.
const NO_DAEMON_RE = /auto-start is disabled/i;

type Target = {
  readonly gen: number;
  readonly enabled: boolean;
  readonly settings: DesktopRunnerSettings | null;
  readonly runnerKey: string;
  // Only the first attempt of an explicit "Start agentscan" carries autoStart; every
  // re-arm latches (autoStart false) so recovery never spawns a daemon on its own.
  readonly autoStart: boolean;
};

export type ConfigureInput = {
  readonly settings: DesktopRunnerSettings | null;
  readonly runnerKey: string;
  // "carry" keeps the enabled value this service last saw for the key (a new
  // key starts gated off). It exists for the active source's preflight lag: a
  // probe that hasn't resolved for the CURRENT runnerKey yet must not bounce
  // an already-armed subscription, and the service's own target is the
  // authoritative "last armed" record — no caller-side mirror needed.
  readonly enabled: boolean | "carry";
};

// The per-source live states the UI observes, keyed by runnerKey. Keys exist only
// for configured sources; read entries through liveStateFor so an absent key
// resolves to the same initial state a freshly added source starts from.
export type LiveStates = ReadonlyMap<string, LiveState>;

const INITIAL_STATE: LiveState = {
  connection: { status: "connecting", message: "Starting live client" },
  rows: [],
  rowsRunnerKey: null,
};

// Selector for one source's slice of the per-key live state map. Consumers that
// track a single source (the dock's active profile today) read through this so
// their view is identical to the old single-target state: a not-yet-configured
// key shows the same "Starting live client" connecting state the service used
// to seed globally.
export const liveStateFor = (states: LiveStates, runnerKey: string): LiveState =>
  states.get(runnerKey) ?? INITIAL_STATE;

const setConnection =
  (connection: ConnectionStatus) =>
  (state: LiveState): LiveState => ({ ...state, connection });

const rowsMessage = (count: number) => `${count} picker ${count === 1 ? "row" : "rows"}`;

const defectMessage = (defect: unknown): string =>
  defect instanceof Error ? defect.message : String(defect);

// Fold one live frame into a state update, and flag terminal frames. Terminal
// frames leave the connection untouched here; runTarget sets the terminal-specific
// status after classification so the policy lives in one place. `runnerKey` stamps
// rows with the runner that produced them so the dock can reject a prior source's
// rows after a switch (see LiveState.rowsRunnerKey).
function foldEvent(
  event: LivePickerEvent,
  runnerKey: string,
): {
  readonly update?: (state: LiveState) => LiveState;
  readonly terminal?: Terminal;
} {
  switch (event.kind) {
    case "connecting":
      return { update: setConnection({ status: "connecting", message: event.message }) };
    case "rows":
      return {
        update: () => ({
          connection: {
            status: "online",
            message: rowsMessage(event.rows.length),
            snapshot: event.snapshot,
          },
          rows: event.rows,
          rowsRunnerKey: runnerKey,
        }),
      };
    case "offline":
      return event.retrying
        ? { update: setConnection({ status: "reconnecting", message: event.message }) }
        : {
            // retrying:false is terminal. "auto-start is disabled" is the latch-miss we offer
            // Start for (NoDaemon); anything else — including the single-shot worker's
            // Offline{retrying:false} for an abnormal subscribe-child death (AUR-517) — is
            // Recoverable, so it re-arms latch-only. We deliberately do NOT escalate an abnormal
            // death on an explicit Start to fatal: `!online` (no rows yet) can't reliably tell a
            // genuine "no daemon" from a daemon that connected (Snapshot) but whose row fetch is
            // failing, or an old daemon handing off via shutdown — escalating would wedge those
            // recoverable cases on fatal. A real start refusal still surfaces via the daemon's
            // own fatal frame or the NoDaemon latch-miss.
            terminal: NO_DAEMON_RE.test(event.message)
              ? { _tag: "NoDaemon", message: event.message }
              : { _tag: "Recoverable", message: event.message },
          };
    case "shutdown":
      // "daemon socket server is closing" — the daemon went away but a fresh latch
      // attempt may reattach (e.g. a newer daemon took the socket).
      return { terminal: { _tag: "Recoverable", message: event.message } };
    case "fatal":
      return {
        terminal: NO_DAEMON_RE.test(event.message)
          ? { _tag: "NoDaemon", message: event.message }
          : { _tag: "Fatal", message: event.message },
      };
  }
}

// The live-connection lifecycle. It owns a SubscriptionRef of the per-key live
// states (the single map the UI observes) and one supervised fiber per configured
// source, each subscribing, folding frames, and re-arming per the latch policy
// independently. UI/profile changes drive it through configure/reconnect/start —
// there is no other coupling to App state.
export class LiveConnection extends Effect.Service<LiveConnection>()(
  "desktop/LiveConnection",
  {
    dependencies: [TauriIpc.Default, LiveConnectionConfig.Default],
    scoped: Effect.gen(function* () {
      const tauri = yield* TauriIpc;
      const { backoff } = yield* LiveConnectionConfig;
      const statesRef = yield* SubscriptionRef.make<LiveStates>(new Map());
      // One monotonic epoch counter shared across keys: Rust gates stale starts
      // per key, so cross-key uniqueness is strictly stronger than required and
      // keeps the persisted reload/HMR seed a single value.
      const epochRef = yield* Ref.make<number>(seedEpoch());
      // The service scope: per-key supervisor fibers forked by configure are
      // owned by it, so they die with the runtime like the old single supervisor.
      const scope = yield* Effect.scope;
      // Serializes configure/reconnect/start so a diff and a concurrent retarget
      // can't interleave their edits of the per-key supervisor map.
      const mutex = yield* Effect.makeSemaphore(1);

      const updateKeyState = (key: string, update: (state: LiveState) => LiveState) =>
        SubscriptionRef.update(statesRef, (states) => {
          const next = new Map(states);
          next.set(key, update(states.get(key) ?? INITIAL_STATE));
          return next;
        });
      const setKeyState = (key: string, state: LiveState) =>
        updateKeyState(key, () => state);
      // Drop a removed key's state entry — unless the key was re-added, in which
      // case the new supervisor owns it. The `entries.has` re-check runs INSIDE
      // the update function (under the ref's update semaphore) so check-and-delete
      // is atomic: a dying supervisor's pending drop can't interleave with a
      // remove+re-add of the same key and delete the new entry's state. `entries`
      // is declared below; this closure only runs from configure-installed
      // finalizers, long after initialization.
      const dropKeyState = (key: string) =>
        SubscriptionRef.update(statesRef, (states) => {
          if (entries.has(key) || !states.has(key)) {
            return states;
          }
          const next = new Map(states);
          next.delete(key);
          return next;
        });

      const nextEpoch = Effect.gen(function* () {
        const epoch = yield* Ref.updateAndGet(epochRef, (n) => n + 1);
        yield* Effect.sync(() => {
          try {
            window.localStorage.setItem(LIVE_EPOCH_STORAGE_KEY, String(epoch));
          } catch {
            // Persistence is best-effort; monotonicity within this page still holds.
          }
        });
        return epoch;
      });

      // Drain the live queue for one subscription (epoch), reflecting each frame into
      // this source's state entry, until a terminal frame is seen. The queue is
      // already filtered to this source at offer time (tauri.liveEvents); the
      // sourceKey check here is a redundant guard, while the epoch check drops
      // frames from a superseded epoch of this source (a late worker after a
      // re-arm).
      const consumeUntilTerminal = (
        queue: Queue.Dequeue<LivePickerEnvelope>,
        epoch: number,
        runnerKey: string,
      ): Effect.Effect<ConsumeResult> =>
        Effect.gen(function* () {
          let online = false;
          while (true) {
            const envelope = yield* Queue.take(queue);
            if (envelope.sourceKey !== runnerKey || envelope.epoch !== epoch) {
              continue;
            }
            // A rows frame means a daemon answered: latch any later terminal as a
            // post-connection loss, not a never-connected Start refusal.
            if (envelope.kind === "rows") {
              online = true;
            }
            const outcome = foldEvent(envelope, runnerKey);
            if (outcome.update) {
              yield* updateKeyState(runnerKey, outcome.update);
            }
            if (outcome.terminal) {
              return { terminal: outcome.terminal, online };
            }
          }
        });

      // Run one target to completion (it loops forever, re-arming) until its key's
      // supervisor `switch` interrupts it for a newer target — or the key is removed
      // by configure. Can fail with an IpcError if the event listener
      // (tauri.liveEvents) cannot be installed; superviseTarget turns that into a
      // fatal state rather than letting it propagate and kill the supervisor.
      const runTarget = (target: Target): Effect.Effect<never, IpcError> =>
        Effect.scoped(
          Effect.gen(function* () {
            if (!target.enabled || target.settings === null) {
              yield* setKeyState(target.runnerKey, {
                connection: { status: "connecting", message: "Waiting for a source" },
                rows: [],
                rowsRunnerKey: null,
              });
              return yield* Effect.never;
            }
            const settings = target.settings;
            // One listener for the whole target; awaited so it is live before we
            // start the first subscription (no early frame missed). Filtered to
            // this source at offer time, so the queue stays quiet (instead of
            // buffering sibling sources' frames unboundedly) while this fiber is
            // parked on fatal or sleeping in the noDaemon poll below.
            const queue = yield* tauri.liveEvents(target.runnerKey);

            let first = true;
            while (true) {
              const epoch = yield* nextEpoch;
              const autoStart = first && target.autoStart;
              yield* updateKeyState(
                target.runnerKey,
                setConnection(
                  first
                    ? { status: "connecting", message: "Connecting to agentscan" }
                    : { status: "reconnecting", message: "Reconnecting to agentscan" },
                ),
              );

              // acquireUseRelease pairs the worker start with its stop so the epoch is
              // ALWAYS torn down — on a terminal frame, an error, OR interruption.
              // Crucially, acquire is uninterruptible: if a target switch interrupts us
              // while startLivePicker is still in flight, the start completes (the Rust
              // invoke can't be cancelled mid-IPC anyway) and THEN release stops that
              // epoch, instead of leaking a worker the switch left with no cleanup.
              const { terminal: rawTerminal, online }: ConsumeResult =
                yield* Effect.acquireUseRelease(
                  // acquire: install the worker. Capture (don't discard) the start error —
                  // a rejection means the worker couldn't be installed at all (an IPC/
                  // command-layer failure, distinct from the daemon-state frames it streams).
                  tauri
                    .startLivePicker({
                      settings,
                      sourceKey: target.runnerKey,
                      epoch,
                      autoStart,
                    })
                    .pipe(
                      Effect.as<string | null>(null),
                      Effect.catchAll((error) => Effect.succeed<string | null>(error.message)),
                    ),
                  // use: drain frames until terminal, unless the worker never installed.
                  (startError) =>
                    startError === null
                      ? consumeUntilTerminal(queue, epoch, target.runnerKey)
                      : // Failing to even install the worker is an actionable transport
                        // error, not a transient daemon blip — surface the real message
                        // (with a Reconnect action) rather than fast-looping forever on a
                        // generic "Unable to start" with the cause swallowed. It never
                        // reached online, so online:false.
                        Effect.succeed<ConsumeResult>({
                          terminal: { _tag: "Fatal", message: startError },
                          online: false,
                        }),
                  // release: stop this key's worker for this epoch (idempotent/ignored if
                  // it never installed). Runs after acquire completed, so the
                  // interrupted-start race can't orphan a subscription.
                  () =>
                    tauri
                      .stopLivePicker({ sourceKey: target.runnerKey, epoch })
                      .pipe(Effect.ignore),
                );

              // An explicit Start (autoStart) whose only outcome is a NoDaemon latch-miss that
              // never reached online is a refusal of our OWN start — e.g. the macOS codesign/
              // trust preflight, where lifecycle.rs emits "auto-start is disabled". Promote it to
              // Fatal so the dock surfaces the real reason instead of silently re-arming into a
              // noDaemon poll the user can't fix. We promote ONLY NoDaemon, and only on the first
              // (autoStart) attempt before any rows arrived: every latch re-arm runs with
              // autoStart=false (the only kind this service issues after the first attempt), and
              // once the worker has connected the terminal is a genuine drop. We deliberately do
              // NOT promote a Recoverable terminal here (a clean shutdown/ServerClosing, or the
              // single-shot worker's Offline{retrying:false} for an abnormal subscribe-child
              // death): `!online` (no rows yet) can't reliably distinguish a true start failure
              // from a daemon that connected but whose row fetch is failing, so promoting would
              // wedge recoverable cases on fatal. Those re-arm latch-only instead.
              //
              // This is not a regression for abnormal first-attempt deaths: pre-AUR-517 the Rust
              // worker's in-worker loop ALSO recovered them (emitting retrying:true and retrying
              // latch-only) — they never settled as fatal. Genuine start failures still surface:
              // a command that can't even be built emits Fatal (lib.rs run_live_picker_subscription),
              // the daemon's own refusal arrives as a Fatal frame, and a clean no-daemon refusal
              // ("auto-start is disabled") is the NoDaemon latch-miss promoted right here.
              const terminal: Terminal =
                rawTerminal._tag === "NoDaemon" && autoStart && !online
                  ? { _tag: "Fatal", message: rawTerminal.message }
                  : rawTerminal;

              if (terminal._tag === "Fatal") {
                yield* setKeyState(target.runnerKey, {
                  connection: { status: "fatal", message: terminal.message },
                  rows: [],
                  rowsRunnerKey: null,
                });
                // Stop re-arming; a manual reconnect/configure makes a new target.
                return yield* Effect.never;
              }

              if (terminal._tag === "NoDaemon") {
                yield* setKeyState(target.runnerKey, {
                  connection: { status: "noDaemon", message: terminal.message },
                  rows: [],
                  rowsRunnerKey: null,
                });
                // AUR-518: while no daemon is reachable, cheap-poll `agentscan daemon
                // status` instead of re-arming a full subscribe each tick (which over SSH
                // spins up a whole remote subscribe process). Sleep first — the subscribe
                // that just ended already told us there's no daemon — then probe. Keep
                // polling only while the probe is confident there's no daemon; a reachable
                // daemon, or a probe that can't tell (error → escalate, today's behavior),
                // breaks out to re-arm a full subscribe that then connects (→ online) or
                // surfaces the real terminal. The sleep is the throttle: a constant-false
                // stub under zero backoff would hot-spin, so tests drive false→true.
                let reachable = false;
                while (!reachable) {
                  yield* Effect.sleep(backoff.noDaemon);
                  reachable = yield* tauri.pollDaemonStatus(settings).pipe(
                    Effect.map((result) => result.reachable),
                    Effect.catchAll(() => Effect.succeed(true)),
                  );
                }
              } else {
                yield* updateKeyState(
                  target.runnerKey,
                  setConnection({ status: "reconnecting", message: terminal.message }),
                );
                yield* Effect.sleep(backoff.recoverable);
              }
              first = false;
            }
          }),
        );

      // Surface a fatal state for one source and PARK, so an unexpected failure/
      // defect never tears that source's supervisor down (a dead supervisor would
      // strand every later configure/reconnect/start with no consumer, wedging the
      // dock with no recovery). Parking on Effect.never keeps the fiber alive so
      // the key's next target switch re-arms us.
      const parkFatal = (runnerKey: string, message: string): Effect.Effect<never> =>
        Effect.zipRight(
          setKeyState(runnerKey, {
            connection: { status: "fatal", message },
            rows: [],
            rowsRunnerKey: null,
          }),
          Effect.never,
        );

      // Wrap each target run so a failure (the event listener failing to install) or
      // an unexpected defect inside runTarget is parked as fatal. catchAll +
      // catchAllDefect deliberately do NOT catch interruption, so the supervisor's
      // `switch` on a new target still cancels this run cleanly rather than being
      // swallowed and flashed as a spurious fatal.
      const superviseTarget = (target: Target): Effect.Effect<never> =>
        runTarget(target).pipe(
          Effect.catchAll((error) => parkFatal(target.runnerKey, error.message)),
          Effect.catchAllDefect((defect) =>
            parkFatal(target.runnerKey, defectMessage(defect)),
          ),
        );

      // Per-key supervisor: each distinct target interrupts and replaces that key's
      // running one. Stream.changes dedupes the same-reference target an idempotent
      // configure/retarget returns, so only real changes (enabled flip, reconnect,
      // start) re-arm.
      const superviseKey = (targetRef: SubscriptionRef.SubscriptionRef<Target>) =>
        targetRef.changes.pipe(
          Stream.changes,
          Stream.flatMap((target) => Stream.fromEffect(superviseTarget(target)), {
            switch: true,
          }),
          Stream.runDrain,
        );

      type Entry = {
        readonly targetRef: SubscriptionRef.SubscriptionRef<Target>;
        readonly fiber: Fiber.RuntimeFiber<void>;
      };
      // The running per-key supervisors. Mutated only under `mutex`.
      const entries = new Map<string, Entry>();

      // Reconcile the running sources to `inputs`, diffing by runnerKey: start
      // added keys, interrupt removed keys, leave unchanged keys running. An
      // existing key re-targets only when its enabled flag flips — settings are
      // part of the runnerKey, so a same-key input carries the same settings.
      // Removal interrupts in the background (the interrupted run's release stops
      // its Rust worker), and the key's state entry is dropped once its supervisor
      // finishes dying — unless the key was re-added meanwhile, in which case the
      // new entry owns the state.
      const configure = (inputs: ReadonlyArray<ConfigureInput>) =>
        mutex.withPermits(1)(
          Effect.gen(function* () {
            const next = new Map(inputs.map((input) => [input.runnerKey, input] as const));
            for (const [key, entry] of [...entries]) {
              if (next.has(key)) {
                continue;
              }
              entries.delete(key);
              yield* Fiber.interruptFork(entry.fiber);
            }
            for (const [key, input] of next) {
              const existing = entries.get(key);
              if (existing) {
                // "carry" resolves against the same current.enabled the diff
                // below compares, inside this mutex, so the carried value is
                // exactly what the service last armed — by construction a
                // no-op update that re-arms nothing.
                yield* SubscriptionRef.update(existing.targetRef, (current) => {
                  const enabled =
                    input.enabled === "carry" ? current.enabled : input.enabled;
                  return current.enabled === enabled
                    ? current
                    : {
                        gen: current.gen + 1,
                        enabled,
                        settings: input.settings,
                        runnerKey: key,
                        autoStart: false,
                      };
                });
                continue;
              }
              // A re-added key may inherit the previous supervisor's state entry:
              // removal interrupts in the background, and once this key is
              // re-registered the dying fiber's dropKeyState defers to the new
              // owner. Reset it here so a fast close-then-reopen can't present the
              // prior session's rows as ready (and clickable) while the fresh
              // subscription is still connecting. Same-supervisor re-arms
              // (reconnect) keep their rows — that flicker-avoidance is per
              // supervisor, not per re-add. Either interleaving with a pending
              // drop ends clean: drop-after skips (entry owned), drop-before
              // deletes and the supervisor recreates from INITIAL_STATE.
              yield* setKeyState(key, INITIAL_STATE);
              const targetRef = yield* SubscriptionRef.make<Target>({
                gen: 0,
                // A new key has no history to carry, so "carry" gates it off
                // until a real verdict arrives (launch, or an in-place edit
                // that moved the runnerKey).
                enabled: input.enabled === "carry" ? false : input.enabled,
                settings: input.settings,
                runnerKey: key,
                autoStart: false,
              });
              const fiber = yield* superviseKey(targetRef).pipe(
                Effect.ensuring(dropKeyState(key)),
                Effect.forkIn(scope),
              );
              entries.set(key, { targetRef, fiber });
            }
          }),
        );

      // Bump one source's target so its supervisor re-arms now. The autoStart latch
      // applies only to the key it was issued for; an unconfigured key is a no-op
      // (there is no subscription to re-arm).
      const retarget = (runnerKey: string, autoStart: boolean) =>
        mutex.withPermits(1)(
          Effect.suspend(() => {
            const entry = entries.get(runnerKey);
            if (entry === undefined) {
              return Effect.void;
            }
            return SubscriptionRef.update(entry.targetRef, (current) => ({
              ...current,
              gen: current.gen + 1,
              autoStart,
            }));
          }),
        );

      return {
        states: statesRef,
        // Reconcile to the listed targets on a profile/preflight change. Diffing by
        // runnerKey + enabled means an idempotent call leaves every running key
        // untouched.
        configure,
        // Re-arm one source now (latch only) — used by the Refresh button and the
        // fatal-state Reconnect action.
        reconnect: (runnerKey: string) => retarget(runnerKey, false),
        // The only path that may spawn a daemon — the "Start agentscan" action.
        start: (runnerKey: string) => retarget(runnerKey, true),
      };
    }),
  },
) {}

function seedEpoch(): number {
  let base = Date.now();
  try {
    // `window` is dereferenced INSIDE this try on purpose. Under Vitest's node
    // environment (and any non-DOM host) the bare `window` throws a ReferenceError;
    // the catch below swallows it so service construction still succeeds and seeds
    // from Date.now(). Do not hoist this access out of the try.
    const stored = Number.parseInt(
      window.localStorage.getItem(LIVE_EPOCH_STORAGE_KEY) ?? "",
      10,
    );
    if (Number.isFinite(stored) && stored >= base) {
      base = stored;
    }
  } catch {
    // localStorage unavailable (or running under tests); Date.now() still seeds it.
  }
  return base;
}
