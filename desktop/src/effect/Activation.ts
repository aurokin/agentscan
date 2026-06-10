// The pane-activation lifecycle, extracted from App.tsx: the one-at-a-time
// in-flight guard, the failure surface scoped to the row's own source, the
// failure TTL, and its interplay with that source's post-failure recovery.
// App.tsx shapes the request (row + profile → ActivateInput), renders `state`,
// and reconciles open folders via prune; everything stateful lives here.

import { Duration, Effect, Fiber, Stream, SubscriptionRef } from "effect";
import { invoke } from "@tauri-apps/api/core";
import { LiveConnection, liveStateFor } from "./LiveConnection";
import type { PickerActivation } from "./pickerViewModel";
import { IpcError } from "./TauriIpc";
import type { DesktopRunnerSettings } from "./types";

const messageOf = (error: unknown) => (error instanceof Error ? error.message : String(error));

// The focus-invoke boundary, injected as a service so the Activation logic stays
// pure over it and a vitest layer can script slow/failing focuses without a real
// Tauri host.
export class FocusIpc extends Effect.Service<FocusIpc>()("desktop/FocusIpc", {
  succeed: {
    focusRow: (input: { paneId: string; settings: DesktopRunnerSettings }) =>
      Effect.tryPromise({
        try: () =>
          invoke<void>("focus_picker_row", {
            paneId: input.paneId,
            settings: input.settings,
          }),
        catch: (error) => new IpcError({ op: "focus_picker_row", message: messageOf(error) }),
      }),
  },
}) {}

// How long a failed activation's error strip stays up. Long enough to read,
// short enough that one-shot action feedback doesn't linger as a standing
// condition (the full error remains in the debug log). Injected so tests can
// drive it with a TestClock.
export class ActivationConfig extends Effect.Service<ActivationConfig>()(
  "desktop/ActivationConfig",
  {
    succeed: {
      failureTtl: Duration.seconds(10),
    },
  },
) {}

export type ActivateInput = {
  readonly paneId: string;
  // The runnerKey of the row's OWN source (rows are tagged by the folder that
  // renders them); scopes the running pulse and the failure surface.
  readonly sourceKey: string;
  readonly settings: DesktopRunnerSettings;
  // Command-log sink ("started", then "ok" or the failure detail) — the debug
  // panel is React state, so the caller routes entries, like SummonHotkey's
  // configure-time press callback.
  readonly onLog: (detail: string) => void;
};

const IDLE: PickerActivation = { status: "idle" };

type Running = {
  // Identity for the lock-free settle cleanup: the guard can be freed EARLY
  // (prune interrupts a wedged in-flight source) and re-taken by a newer
  // activation before the stale fiber's finalizer runs, which must then leave
  // the newer entry alone.
  readonly token: object;
  // Assigned right after the fork (still under the activate mutex, so prune —
  // which needs the same permit — never observes the null window).
  fiber: Fiber.RuntimeFiber<void> | null;
};

// Owns the activation state machine the picker renders. One supervised fiber per
// activation (at most one at a time), plus a TTL supervisor that expires a
// surfaced failure once its source's recovery settles.
export class Activation extends Effect.Service<Activation>()("desktop/Activation", {
  dependencies: [FocusIpc.Default, ActivationConfig.Default, LiveConnection.Default],
  scoped: Effect.gen(function* () {
    const ipc = yield* FocusIpc;
    const { failureTtl } = yield* ActivationConfig;
    const lc = yield* LiveConnection;
    const stateRef = yield* SubscriptionRef.make<PickerActivation>(IDLE);
    // The service scope: activation fibers forked by activate are owned by it,
    // so they die with the runtime.
    const scope = yield* Effect.scope;
    // Serializes activate/prune so a click and a folder-close can't interleave
    // their reads of the in-flight slot.
    const mutex = yield* Effect.makeSemaphore(1);
    // The in-flight activation. Mutated under `mutex`, except the settle
    // cleanup's token-guarded clear (a single synchronous step).
    let running: Running | null = null;

    // One focus attempt: log the command, reflect the outcome into the shared
    // state, and on failure re-arm the source's live client.
    //
    // No settle-time ownership check is needed here (the old App.tsx token
    // guard): this fiber holds the slot for its whole life. A successor can
    // start only after the ensuring in activate frees the slot (this fiber is
    // done — its writes already landed) or after prune interrupts it (a fiber
    // with a pending interrupt resumes only to run finalizers, never this
    // continuation). Either way a superseded activation cannot write here. The
    // old code needed the token because a settled JS promise's continuation
    // always runs; an interrupted fiber's does not.
    const runActivation = (input: ActivateInput): Effect.Effect<void> =>
      Effect.gen(function* () {
        yield* Effect.sync(() => input.onLog("started"));
        const failure = yield* ipc.focusRow({ paneId: input.paneId, settings: input.settings }).pipe(
          Effect.as<string | null>(null),
          Effect.catchAll((error) => Effect.succeed<string | null>(error.message)),
        );
        if (failure === null) {
          yield* Effect.sync(() => input.onLog("ok"));
          // Persistent-window model: focusing a pane must not hide the desktop.
          // Reset to idle and leave the window visible.
          yield* SubscriptionRef.set(stateRef, IDLE);
          return;
        }
        yield* Effect.sync(() => input.onLog(failure));
        yield* SubscriptionRef.set(stateRef, {
          status: "failed",
          message: failure,
          sourceKey: input.sourceKey,
        });
        // A failed focus is strong evidence the row is stale (the pane is gone). The
        // daemon is event-driven with periodic reconcile OFF by default, so a missed
        // tmux close notification won't self-correct — agentscan's own design names the
        // connect/reconnect bootstrap as the ground-truth recovery (config.rs). Re-arm
        // the live client: re-subscribing makes the daemon publish a fresh initial
        // snapshot, which the worker re-derives via load_picker_rows, dropping the dead
        // row. This is the push-model equivalent of the old one-shot refetch.
        //
        // Unconditional on purpose — a "skip if the source closed meanwhile"
        // guard is dead in every interleaving: once prune has observed the
        // close it has already interrupted this fiber (the reconnect is never
        // reached), and before prune fires any service-held open-set is stale.
        // The residual race (the source closes and the failure settles inside
        // React's state→effect latency) costs one latch-only re-arm that the
        // targets reconcile immediately tears down; reconnect no-ops entirely
        // once the key is deconfigured. The old render-synced ref guard had
        // the same window, merely render- instead of effect-wide.
        yield* lc.reconnect(input.sourceKey);
      });

    // Whether one source's live client is mid-recovery, deduped so unrelated
    // live frames don't re-trigger the TTL switch below (the old React effect
    // got this from a memoized boolean for the same reason). An unconfigured
    // key resolves to the initial "connecting" state and so reads as
    // recovering, exactly like the old liveStateFor-based memo.
    const recoveringFor = (sourceKey: string): Stream.Stream<boolean> =>
      lc.states.changes.pipe(
        Stream.map((states) => {
          const status = liveStateFor(states, sourceKey).connection.status;
          return status === "connecting" || status === "reconnecting";
        }),
        Stream.changes,
      );

    // TTL supervisor. A failed activation is one-shot action feedback, not
    // ongoing state — left alone it outlives its moment and reads like a
    // standing condition, so it expires after a beat once recovery settles.
    // While the post-failure reconnect re-derives the failed source's rows, the
    // failure doubles as that source's stale-row mask (sourceViews'
    // `recovering`): expiring it mid-recovery would re-expose the known-dead
    // pane and make it instantly re-clickable, so each recovering episode drops
    // the pending timer and each settle arms a FRESH full TTL (the switch
    // semantics below; pinned by the flap test). The identity guard inside the
    // update clears only the exact failure the timer was armed for.
    yield* stateRef.changes.pipe(
      Stream.changes,
      Stream.flatMap(
        (current) =>
          current.status !== "failed"
            ? Stream.empty
            : recoveringFor(current.sourceKey).pipe(
                Stream.flatMap(
                  (recovering) =>
                    recovering
                      ? Stream.empty
                      : Stream.fromEffect(
                          Effect.sleep(failureTtl).pipe(
                            Effect.zipRight(
                              SubscriptionRef.update(stateRef, (state) =>
                                state === current ? IDLE : state,
                              ),
                            ),
                          ),
                        ),
                  { switch: true },
                ),
              ),
        { switch: true },
      ),
      Stream.runDrain,
      Effect.forkScoped,
    );

    return {
      state: stateRef,

      // Focus one row. At most one activation runs at a time across all
      // sources: a second call while one is in flight bails here, under the
      // mutex, so a double-click can't fire focus_picker_row twice (the old
      // synchronous ref guard). The work is forked so the caller returns
      // immediately — an interrupted caller fiber (Atom.fn re-invocation)
      // never cancels an in-flight focus.
      activate: (input: ActivateInput) =>
        mutex.withPermits(1)(
          Effect.gen(function* () {
            if (running !== null) {
              return;
            }
            const entry: Running = { token: {}, fiber: null };
            running = entry;
            yield* SubscriptionRef.set(stateRef, {
              status: "running",
              paneId: input.paneId,
              sourceKey: input.sourceKey,
            });
            const fiber = yield* runActivation(input).pipe(
              // Settle cleanup, token-guarded (see Running.token). Runs on
              // completion AND on prune's interrupt, where the slot is already
              // cleared or re-owned.
              Effect.ensuring(
                Effect.sync(() => {
                  if (running?.token === entry.token) {
                    running = null;
                  }
                }),
              ),
              Effect.forkIn(scope),
            );
            // The fiber may already have completed (and its cleanup run) while
            // we were suspended on the fork — only attach it to a slot we still
            // own, never resurrect a freed one.
            if (running === entry) {
              entry.fiber = fiber;
            }
          }),
        ),

      // Reconcile the activation against the open folders: drop a pulse/error
      // whose source is no longer open (closed, deleted, or retargeted by a
      // settings edit) — there is no list left for it to describe. Driven by
      // React whenever the open-folder set changes.
      prune: (openKeys: ReadonlyArray<string>) =>
        mutex.withPermits(1)(
          Effect.gen(function* () {
            const current = yield* SubscriptionRef.get(stateRef);
            if (current.status === "idle" || openKeys.includes(current.sourceKey)) {
              return;
            }
            if (current.status === "running" && running?.fiber) {
              // A still-running activation's invoke may be wedged until the
              // Rust-side focus timeout; "running" means the guard is held by
              // exactly this activation, so free it with the visible state —
              // otherwise every source's clicks/keys silently no-op behind an
              // invisible in-flight call. interruptFork: don't await the wedged
              // invoke (the interrupted fiber abandons it; its outcome is
              // discarded with the fiber's continuation).
              yield* Fiber.interruptFork(running.fiber);
              running = null;
            }
            yield* SubscriptionRef.set(stateRef, IDLE);
          }),
        ),
    };
  }),
}) {}
