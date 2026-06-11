import { Deferred, Duration, Effect, Layer, Option, Queue, Ref, Stream, SubscriptionRef } from "effect";
import { describe, expect, it } from "vitest";
import {
  HotkeyIpc,
  SummonHotkey,
  SummonHotkeyConfig,
  summonHotkeyFailureMessage,
  summonHotkeyInUse,
  type SummonHotkeyState,
} from "./SummonHotkey";
import { IpcError } from "./TauriIpc";

describe("summonHotkeyInUse", () => {
  it("recognizes the macOS in-use failure and the plugin duplicate error", () => {
    expect(
      summonHotkeyInUse(new Error("Unable to register hotkey: RegisterEventHotKey failed for KeyA")),
    ).toBe(true);
    expect(summonHotkeyInUse(new Error("HotKey already registered"))).toBe(true);
  });

  it("treats other failures as terminal so the retry loop never arms", () => {
    expect(summonHotkeyInUse(new Error("permission denied"))).toBe(false);
    expect(summonHotkeyInUse("boom")).toBe(false);
  });
});

describe("summonHotkeyFailureMessage", () => {
  it("explains an in-use key from the macOS RegisterEventHotKey failure", () => {
    const message = summonHotkeyFailureMessage(
      new Error("Unable to register hotkey: RegisterEventHotKey failed for KeyA"),
    );
    expect(message).toBe(
      "⌘⇧A is in use — another agentscan instance may be running. Retrying until it frees up.",
    );
  });

  it("explains an in-use key from the plugin's already-registered error", () => {
    const message = summonHotkeyFailureMessage(new Error("HotKey already registereD"));
    expect(message).toContain("is in use");
  });

  it("falls back to the raw detail for other errors", () => {
    expect(summonHotkeyFailureMessage(new Error("permission denied"))).toBe(
      "Unable to register ⌘⇧A: permission denied",
    );
  });

  it("stringifies non-Error values", () => {
    expect(summonHotkeyFailureMessage("boom")).toBe("Unable to register ⌘⇧A: boom");
  });
});

// Zero retry backoff keeps the in-use retry loop event-driven (gated by the
// scripted outcomes below), no wall-clock.
const ZeroBackoff = Layer.succeed(SummonHotkeyConfig, { retryBackoff: Duration.zero });

const IN_USE = "Unable to register hotkey: RegisterEventHotKey failed for KeyA";
const TERMINAL = "permission denied";

// A scripted HotkeyIpc: each register consumes the next outcome (an error
// message, an Effect gate to await before succeeding, or undefined = succeed)
// and captures the press handler; ops records the register/unregister order.
type Outcome = string | Effect.Effect<void> | undefined;

const makeHarness = (script: ReadonlyArray<Outcome>) =>
  Effect.gen(function* () {
    const outcomes = yield* Ref.make<ReadonlyArray<Outcome>>(script);
    const ops = yield* Ref.make<ReadonlyArray<string>>([]);
    // The latest registration's handler, callable from the test like the plugin
    // firing the shortcut (a plain synchronous callback).
    let fire: (state: "Pressed" | "Released") => void = () => {};

    const layer = Layer.succeed(HotkeyIpc, {
      register: (shortcut: string, onEvent: (state: "Pressed" | "Released") => void) =>
        Effect.gen(function* () {
          const next = yield* Ref.modify(outcomes, (rest) => [rest[0], rest.slice(1)] as const);
          if (typeof next === "string") {
            yield* Ref.update(ops, (log) => [...log, `register:fail`]);
            return yield* Effect.fail(new IpcError({ op: "register_shortcut", message: next }));
          }
          if (next !== undefined) {
            yield* next;
          }
          yield* Ref.update(ops, (log) => [...log, `register:${shortcut}`]);
          fire = onEvent;
        }),
      unregister: (shortcut: string) =>
        Ref.update(ops, (log) => [...log, `unregister:${shortcut}`]),
    });

    return {
      layer: SummonHotkey.DefaultWithoutDependencies.pipe(
        Layer.provide(Layer.merge(layer, ZeroBackoff)),
      ),
      ops: Ref.get(ops),
      press: (state: "Pressed" | "Released") => fire(state),
    };
  });

// Block until the hotkey state reaches a given status (changes replays the
// current value, so this resolves immediately if already there).
const awaitStatus = (
  state: SubscriptionRef.SubscriptionRef<SummonHotkeyState>,
  status: SummonHotkeyState["status"],
): Effect.Effect<SummonHotkeyState> =>
  state.changes.pipe(
    Stream.filter((s) => s.status === status),
    Stream.runHead,
    Effect.flatMap(
      Option.match({
        onNone: () => Effect.die("state stream ended early"),
        onSome: Effect.succeed,
      }),
    ),
  );

describe("SummonHotkey", () => {
  it("registers and routes only Pressed events to the configured action", () =>
    Effect.gen(function* () {
      const harness = yield* makeHarness([undefined]);
      const presses: string[] = [];

      yield* Effect.gen(function* () {
        const hotkey = yield* SummonHotkey;
        yield* hotkey.configure(() => presses.push("summon"));
        yield* awaitStatus(hotkey.state, "registered");

        harness.press("Released");
        harness.press("Pressed");
        expect(presses).toEqual(["summon"]);
        expect(yield* harness.ops).toEqual(["register:CommandOrControl+Shift+A"]);
      }).pipe(Effect.provide(harness.layer));
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));

  it("retries an in-use failure until the holder frees the key, without re-publishing the standing banner", () =>
    Effect.gen(function* () {
      // TWO in-use failures, then a gated success: the second failure must not
      // re-emit the (identical) failed state — every emission re-renders the
      // dock, and the retry loop runs indefinitely while the key is held.
      const freed = yield* Deferred.make<void>();
      const harness = yield* makeHarness([IN_USE, IN_USE, Deferred.await(freed)]);

      yield* Effect.gen(function* () {
        const hotkey = yield* SummonHotkey;
        const emissions = yield* Queue.unbounded<SummonHotkeyState>();
        yield* hotkey.state.changes.pipe(
          Stream.runForEach((state) => Queue.offer(emissions, state)),
          Effect.fork,
        );
        expect(yield* Queue.take(emissions)).toEqual({ status: "inactive" });

        yield* hotkey.configure(() => {});
        expect(yield* Queue.take(emissions)).toEqual({
          status: "failed",
          message:
            "⌘⇧A is in use — another agentscan instance may be running. Retrying until it frees up.",
        });

        // The holder quits → the gated retry registers and clears the banner.
        // Both failures ran before it (ops below), yet exactly one failed
        // emission reached subscribers.
        yield* Deferred.succeed(freed, undefined);
        expect(yield* Queue.take(emissions)).toEqual({ status: "registered" });
        expect(yield* Queue.poll(emissions)).toEqual(Option.none());
        expect(yield* harness.ops).toEqual([
          "register:fail",
          "register:fail",
          "register:CommandOrControl+Shift+A",
        ]);
      }).pipe(Effect.provide(harness.layer));
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));

  it("surfaces a terminal failure once and never retries it", () =>
    Effect.gen(function* () {
      // The success canary after the terminal failure would flip the state to
      // registered if the loop (wrongly) retried.
      const harness = yield* makeHarness([TERMINAL, undefined]);

      yield* Effect.gen(function* () {
        const hotkey = yield* SummonHotkey;
        yield* hotkey.configure(() => {});

        const failed = yield* awaitStatus(hotkey.state, "failed");
        expect(failed).toEqual({
          status: "failed",
          message: "Unable to register ⌘⇧A: permission denied",
        });

        // Give a (wrong) zero-backoff retry every chance to run, then confirm
        // the loop parked: one attempt, state still failed.
        yield* Effect.yieldNow().pipe(Effect.repeatN(20));
        expect(yield* SubscriptionRef.get(hotkey.state)).toEqual(failed);
        expect(yield* harness.ops).toEqual(["register:fail"]);
      }).pipe(Effect.provide(harness.layer));
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));

  it("configure(null) releases the key, and a re-configure re-registers after the release", () =>
    Effect.gen(function* () {
      const harness = yield* makeHarness([undefined, undefined]);

      yield* Effect.gen(function* () {
        const hotkey = yield* SummonHotkey;
        yield* hotkey.configure(() => {});
        yield* awaitStatus(hotkey.state, "registered");

        yield* hotkey.configure(null);
        yield* awaitStatus(hotkey.state, "inactive");

        // Re-arm: configure awaited the dying fiber's unregister, so the order
        // is strictly register → unregister → register.
        yield* hotkey.configure(() => {});
        yield* awaitStatus(hotkey.state, "registered");
        expect(yield* harness.ops).toEqual([
          "register:CommandOrControl+Shift+A",
          "unregister:CommandOrControl+Shift+A",
          "register:CommandOrControl+Shift+A",
        ]);
      }).pipe(Effect.provide(harness.layer));
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));

  it("a re-configure recovers a terminal failure and swaps the press action", () =>
    Effect.gen(function* () {
      const harness = yield* makeHarness([TERMINAL, undefined]);
      const presses: string[] = [];

      yield* Effect.gen(function* () {
        const hotkey = yield* SummonHotkey;
        yield* hotkey.configure(() => presses.push("first"));
        yield* awaitStatus(hotkey.state, "failed");

        // No key was held (register never succeeded), so recovery is just a new
        // attempt — no unregister in between.
        yield* hotkey.configure(() => presses.push("second"));
        yield* awaitStatus(hotkey.state, "registered");
        harness.press("Pressed");
        expect(presses).toEqual(["second"]);
        expect(yield* harness.ops).toEqual([
          "register:fail",
          "register:CommandOrControl+Shift+A",
        ]);
      }).pipe(Effect.provide(harness.layer));
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));
});
