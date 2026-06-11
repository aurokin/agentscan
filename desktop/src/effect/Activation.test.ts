import {
  Deferred,
  Duration,
  Effect,
  Layer,
  Option,
  Ref,
  Stream,
  SubscriptionRef,
  TestClock,
  TestContext,
} from "effect";
import { describe, expect, it } from "vitest";
import { Activation, ActivationConfig, FocusIpc, type ActivateInput } from "./Activation";
import { LiveConnection, type LiveStates } from "./LiveConnection";
import type { PickerActivation } from "./pickerViewModel";
import { IpcError } from "./TauriIpc";
import type { DesktopRunnerSettings, LiveState } from "./types";

const SETTINGS: DesktopRunnerSettings = { kind: "local", binaryPath: "", env: [] };

const ONLINE: LiveState = {
  connection: {
    status: "online",
    message: "ok",
    snapshot: { paneCount: 0, generatedAt: null, sourceKind: "tmux" },
  },
  rows: [],
  rowsRunnerKey: "k1",
};

const RECONNECTING: LiveState = {
  connection: { status: "reconnecting", message: "Reconnecting to agentscan" },
  rows: [],
  rowsRunnerKey: null,
};

// The production TTL, driven by TestClock below so the tests pin the real
// 10-second window, not a scaled-down stand-in.
const Ttl = Layer.succeed(ActivationConfig, { failureTtl: Duration.seconds(10) });

const failWith = (message: string) =>
  Effect.fail(new IpcError({ op: "focus_picker_row", message }));

// Scripted FocusIpc (each call consumes the next outcome) over a mock
// LiveConnection whose states the test drives directly — Activation only reads
// `states` and calls `reconnect`, so the rest of the interface is inert.
const makeHarness = (script: ReadonlyArray<Effect.Effect<void, IpcError>>) =>
  Effect.gen(function* () {
    const liveStates = yield* SubscriptionRef.make<LiveStates>(new Map());
    const reconnects = yield* Ref.make<ReadonlyArray<string>>([]);
    const focusCalls = yield* Ref.make<ReadonlyArray<string>>([]);
    const outcomes = yield* Ref.make(script);

    const MockFocus = Layer.succeed(FocusIpc, {
      focusRow: (input: { paneId: string; settings: DesktopRunnerSettings }) =>
        Effect.gen(function* () {
          yield* Ref.update(focusCalls, (calls) => [...calls, input.paneId]);
          const next = yield* Ref.modify(outcomes, (rest) => [rest[0], rest.slice(1)] as const);
          yield* next ?? Effect.void;
        }),
    });

    const MockLive = Layer.succeed(LiveConnection, {
      states: liveStates,
      configure: () => Effect.void,
      reconnect: (runnerKey: string) =>
        Ref.update(reconnects, (keys) => [...keys, runnerKey]),
      start: () => Effect.void,
    });

    return {
      layer: Activation.DefaultWithoutDependencies.pipe(
        Layer.provide(Layer.mergeAll(MockFocus, MockLive, Ttl)),
      ),
      liveStates,
      reconnects: Ref.get(reconnects),
      focusCalls: Ref.get(focusCalls),
    };
  });

const input = (
  paneId: string,
  sourceKey: string,
  log?: string[],
  isSourceOpen: () => boolean = () => true,
): ActivateInput => ({
  paneId,
  sourceKey,
  settings: SETTINGS,
  isSourceOpen,
  onLog: (detail) => log?.push(detail),
});

// Block until the activation reaches a given status (changes replays the
// current value, so this resolves immediately if already there).
const awaitStatus = (
  state: SubscriptionRef.SubscriptionRef<PickerActivation>,
  status: PickerActivation["status"],
): Effect.Effect<PickerActivation> =>
  state.changes.pipe(
    Stream.filter((a) => a.status === status),
    Stream.runHead,
    Effect.flatMap(
      Option.match({
        onNone: () => Effect.die("state stream ended early"),
        onSome: Effect.succeed,
      }),
    ),
  );

// Let the service's supervisor fibers process a state flip and (re)arm their
// TestClock sleeps before the test adjusts the clock — every step in between is
// scheduler-driven (no real timers), so yielding is sufficient and deterministic.
const settle = Effect.yieldNow().pipe(Effect.repeatN(40));

describe("Activation", () => {
  // THE pin for the TTL/recovery interplay, matching the old React effects:
  // the failure mask holds (no timer) while the failed source's client is
  // recovering, and every recovery episode re-arms a FRESH full TTL window —
  // never the remainder of a previous one. A one-shot timer would re-expose
  // the known-dead row mid-flap.
  it("holds a failure while its source recovers and re-arms a fresh TTL per episode", () =>
    Effect.gen(function* () {
      const harness = yield* makeHarness([failWith("focus failed: pane gone")]);

      yield* Effect.gen(function* () {
        const activation = yield* Activation;
        yield* SubscriptionRef.set(harness.liveStates, new Map([["k1", ONLINE]]));
        yield* activation.prune(["k1"]);
        yield* activation.activate(input("%1", "k1"));
        const failed = yield* awaitStatus(activation.state, "failed");
        expect(failed).toEqual({
          status: "failed",
          message: "focus failed: pane gone",
          sourceKey: "k1",
        });

        // 9s into the 10s window: still up.
        yield* settle;
        yield* TestClock.adjust("9 seconds");
        yield* settle;
        expect((yield* SubscriptionRef.get(activation.state)).status).toBe("failed");

        // The failed source's client starts recovering: the pending timer is
        // dropped and the failure (doubling as the stale-row mask) holds
        // indefinitely — expiring it would make the dead pane clickable again.
        yield* SubscriptionRef.set(harness.liveStates, new Map([["k1", RECONNECTING]]));
        yield* settle;
        yield* TestClock.adjust("60 seconds");
        yield* settle;
        expect((yield* SubscriptionRef.get(activation.state)).status).toBe("failed");

        // Recovery settles: a FRESH 10s window arms. 9s in it is still up —
        // the pre-flap timer (1s remaining) must not have carried over.
        yield* SubscriptionRef.set(harness.liveStates, new Map([["k1", ONLINE]]));
        yield* settle;
        yield* TestClock.adjust("9 seconds");
        yield* settle;
        expect((yield* SubscriptionRef.get(activation.state)).status).toBe("failed");

        // ...and the full window elapsing finally expires it.
        yield* TestClock.adjust("2 seconds");
        yield* awaitStatus(activation.state, "idle");
      }).pipe(Effect.provide(harness.layer), Effect.provide(TestContext.TestContext));
    }).pipe(Effect.runPromise));

  it("runs one activation at a time, never expires a running one, and resets to idle on success", () =>
    Effect.gen(function* () {
      const gate = yield* Deferred.make<void>();
      const harness = yield* makeHarness([Deferred.await(gate)]);
      const firstLog: string[] = [];
      const secondLog: string[] = [];

      yield* Effect.gen(function* () {
        const activation = yield* Activation;
        yield* SubscriptionRef.set(harness.liveStates, new Map([["k1", ONLINE]]));
        yield* activation.prune(["k1"]);

        yield* activation.activate(input("%1", "k1", firstLog));
        const running = yield* awaitStatus(activation.state, "running");
        expect(running).toEqual({ status: "running", paneId: "%1", sourceKey: "k1" });

        // A second click while one is in flight bails before reaching the IPC
        // (the double-click guard), leaving the first untouched.
        yield* activation.activate(input("%2", "k1", secondLog));
        expect(yield* harness.focusCalls).toEqual(["%1"]);
        expect(secondLog).toEqual([]);

        // Running is not one-shot feedback; only failures TTL out.
        yield* settle;
        yield* TestClock.adjust("60 seconds");
        yield* settle;
        expect((yield* SubscriptionRef.get(activation.state)).status).toBe("running");

        // Persistent-window model: success resets to idle (no hide).
        yield* Deferred.succeed(gate, undefined);
        yield* awaitStatus(activation.state, "idle");
        expect(firstLog).toEqual(["started", "ok"]);
      }).pipe(Effect.provide(harness.layer), Effect.provide(TestContext.TestContext));
    }).pipe(Effect.runPromise));

  it("surfaces a failure on its open source, logs the detail, and re-arms that source's live client", () =>
    Effect.gen(function* () {
      const harness = yield* makeHarness([failWith("no server running")]);
      const log: string[] = [];

      yield* Effect.gen(function* () {
        const activation = yield* Activation;
        yield* SubscriptionRef.set(harness.liveStates, new Map([["k1", ONLINE]]));
        yield* activation.prune(["k1"]);

        yield* activation.activate(input("%1", "k1", log));
        const failed = yield* awaitStatus(activation.state, "failed");
        expect(failed).toEqual({
          status: "failed",
          message: "no server running",
          sourceKey: "k1",
        });
        expect(yield* harness.reconnects).toEqual(["k1"]);
        expect(log).toEqual(["started", "no server running"]);
      }).pipe(Effect.provide(harness.layer), Effect.provide(TestContext.TestContext));
    }).pipe(Effect.runPromise));

  it("closing the in-flight source frees the guard for other sources and drops the outcome", () =>
    Effect.gen(function* () {
      // The first invoke wedges forever (the Rust-side focus timeout stands in
      // for it); the second must still be able to run.
      const wedge = yield* Deferred.make<void>();
      const harness = yield* makeHarness([Deferred.await(wedge), Effect.void]);

      yield* Effect.gen(function* () {
        const activation = yield* Activation;
        yield* SubscriptionRef.set(
          harness.liveStates,
          new Map([
            ["k1", ONLINE],
            ["k2", ONLINE],
          ]),
        );
        yield* activation.prune(["k1", "k2"]);

        yield* activation.activate(input("%1", "k1"));
        yield* awaitStatus(activation.state, "running");

        // k1 closes mid-flight: the visible state drops AND the guard frees —
        // otherwise every source's clicks silently no-op behind the wedged call.
        yield* activation.prune(["k2"]);
        yield* awaitStatus(activation.state, "idle");

        yield* activation.activate(input("%2", "k2"));
        yield* awaitStatus(activation.state, "idle");
        expect(yield* harness.focusCalls).toEqual(["%1", "%2"]);
        // The abandoned activation must not act on its source after the close.
        expect(yield* harness.reconnects).toEqual([]);
      }).pipe(Effect.provide(harness.layer), Effect.provide(TestContext.TestContext));
    }).pipe(Effect.runPromise));

  it("drops a failure whose source is closed by the time it settles", () =>
    Effect.gen(function* () {
      const harness = yield* makeHarness([failWith("late failure")]);

      yield* Effect.gen(function* () {
        const activation = yield* Activation;
        yield* SubscriptionRef.set(harness.liveStates, new Map([["k1", ONLINE]]));
        yield* activation.prune(["k1"]);
        yield* activation.activate(input("%1", "k1"));
        yield* awaitStatus(activation.state, "failed");

        // The prune that follows the close reconciles the surfaced failure away
        // (there is no folder list left for it to describe).
        yield* activation.prune([]);
        yield* awaitStatus(activation.state, "idle");
      }).pipe(Effect.provide(harness.layer), Effect.provide(TestContext.TestContext));
    }).pipe(Effect.runPromise));

  it("a failure settling after the source closed (before prune) is dropped without a reconnect", () =>
    Effect.gen(function* () {
      // The close lands between the activation starting and its failure
      // settling — the render-synced probe flips before prune gets to run.
      const gate = yield* Deferred.make<void>();
      const harness = yield* makeHarness([
        Deferred.await(gate).pipe(Effect.zipRight(failWith("late failure"))),
      ]);
      let open = true;

      yield* Effect.gen(function* () {
        const activation = yield* Activation;
        yield* SubscriptionRef.set(harness.liveStates, new Map([["k1", ONLINE]]));
        yield* activation.prune(["k1"]);
        yield* activation.activate(input("%1", "k1", undefined, () => open));
        yield* awaitStatus(activation.state, "running");

        open = false;
        yield* Deferred.succeed(gate, undefined);
        yield* awaitStatus(activation.state, "idle");
        // No failure surfaced for the closed folder, and crucially no re-arm
        // of its live client (over SSH that would spawn a doomed subscribe).
        expect(yield* harness.reconnects).toEqual([]);
      }).pipe(Effect.provide(harness.layer), Effect.provide(TestContext.TestContext));
    }).pipe(Effect.runPromise));
});
