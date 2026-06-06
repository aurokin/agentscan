import { Duration, Effect, Layer, Option, Queue, Stream, SubscriptionRef } from "effect";
import { describe, expect, it } from "vitest";
import { Appearance } from "./Appearance";
import { PrefsBridge } from "./PrefsBridge";
import {
  GLASS_STORAGE_KEY,
  ORIENTATION_STORAGE_KEY,
  SURFACE_ALPHA_DEFAULT,
  SURFACE_ALPHA_STORAGE_KEY,
  THEME_STORAGE_KEY,
} from "./appearanceModel";
import type { PrefsSync, ShellMode } from "./prefs";

// A scripted PrefsBridge backed by an in-memory store + an `emitted` queue (outbound
// broadcasts) + an `inbound` queue driving the cross-window `events` stream. Mirrors how
// the Profiles/Preflight tests script the boundary via Layer.succeed. `failWrites` makes
// storeRaw throw so the best-effort-persist behavior can be asserted.
const makeBridge = (
  mode: ShellMode,
  initial: Record<string, string> = {},
  failWrites = false,
) =>
  Effect.gen(function* () {
    const store = new Map<string, string>(Object.entries(initial));
    const emitted = yield* Queue.unbounded<PrefsSync>();
    const inbound = yield* Queue.unbounded<PrefsSync>();
    const layer = Layer.succeed(PrefsBridge, {
      mode,
      loadRaw: (key: string) => store.get(key) ?? null,
      storeRaw: (key: string, value: string) => {
        if (failWrites) {
          throw new Error("quota exceeded");
        }
        store.set(key, value);
      },
      emit: (payload: PrefsSync) => Queue.offer(emitted, payload).pipe(Effect.asVoid),
      events: Stream.fromQueue(inbound),
    });
    return { layer, store, emitted, inbound };
  });

const run = <A>(
  mode: ShellMode,
  initial: Record<string, string>,
  body: (ctx: {
    appearance: Effect.Effect.Success<typeof Appearance>;
    store: Map<string, string>;
    emitted: Queue.Queue<PrefsSync>;
    inbound: Queue.Queue<PrefsSync>;
  }) => Effect.Effect<A>,
  failWrites = false,
) =>
  Effect.gen(function* () {
    const bridge = yield* makeBridge(mode, initial, failWrites);
    const program = Effect.gen(function* () {
      const appearance = yield* Appearance;
      return yield* body({
        appearance,
        store: bridge.store,
        emitted: bridge.emitted,
        inbound: bridge.inbound,
      });
    });
    return yield* program.pipe(
      Effect.provide(Appearance.DefaultWithoutDependencies.pipe(Layer.provide(bridge.layer))),
    );
  }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise);

// Block until a stream emits a value matching `pred` (changes replays the current value).
const awaitWhere = <A>(stream: Stream.Stream<A>, pred: (a: A) => boolean) =>
  stream.pipe(
    Stream.filter(pred),
    Stream.runHead,
    Effect.flatMap(
      Option.match({
        onNone: () => Effect.die("state stream ended early"),
        onSome: Effect.succeed,
      }),
    ),
  );

describe("Appearance", () => {
  it("seeds from storage", () =>
    run(
      "dock",
      {
        [THEME_STORAGE_KEY]: "light",
        [ORIENTATION_STORAGE_KEY]: "horizontal",
        [GLASS_STORAGE_KEY]: "off",
        [SURFACE_ALPHA_STORAGE_KEY]: "0.40",
      },
      ({ appearance }) =>
        Effect.gen(function* () {
          const state = yield* SubscriptionRef.get(appearance.state);
          expect(state).toEqual({
            themePref: "light",
            orientationPref: "horizontal",
            glassEnabled: false,
            surfaceAlpha: 0.4,
          });
        }),
    ));

  it("defaults when storage is empty (system theme, auto layout, glass on, default alpha)", () =>
    run("dock", {}, ({ appearance }) =>
      Effect.gen(function* () {
        const state = yield* SubscriptionRef.get(appearance.state);
        expect(state).toEqual({
          themePref: "system",
          orientationPref: "auto",
          glassEnabled: true,
          surfaceAlpha: SURFACE_ALPHA_DEFAULT,
        });
      }),
    ));

  it("setTheme persists, broadcasts, and updates the ref", () =>
    run("settings", {}, ({ appearance, store, emitted }) =>
      Effect.gen(function* () {
        yield* appearance.setTheme("dark");
        expect(store.get(THEME_STORAGE_KEY)).toBe("dark");
        expect(yield* Queue.take(emitted)).toEqual({ kind: "theme", theme: "dark" });
        expect((yield* SubscriptionRef.get(appearance.state)).themePref).toBe("dark");
      }),
    ));

  it("setOrientationPref persists, broadcasts, and updates the ref", () =>
    run("settings", {}, ({ appearance, store, emitted }) =>
      Effect.gen(function* () {
        yield* appearance.setOrientationPref("vertical");
        expect(store.get(ORIENTATION_STORAGE_KEY)).toBe("vertical");
        expect(yield* Queue.take(emitted)).toEqual({ kind: "orientation", orientation: "vertical" });
        expect((yield* SubscriptionRef.get(appearance.state)).orientationPref).toBe("vertical");
      }),
    ));

  it("setGlassEnabled broadcasts BOTH glass fields (carrying the current alpha)", () =>
    run("settings", { [SURFACE_ALPHA_STORAGE_KEY]: "0.30" }, ({ appearance, store, emitted }) =>
      Effect.gen(function* () {
        yield* appearance.setGlassEnabled(false);
        expect(store.get(GLASS_STORAGE_KEY)).toBe("off");
        expect(yield* Queue.take(emitted)).toEqual({ kind: "glass", enabled: false, alpha: 0.3 });
        expect((yield* SubscriptionRef.get(appearance.state)).glassEnabled).toBe(false);
      }),
    ));

  it("setSurfaceAlpha broadcasts BOTH glass fields (carrying the current enabled) and persists 2dp", () =>
    run("settings", { [GLASS_STORAGE_KEY]: "on" }, ({ appearance, store, emitted }) =>
      Effect.gen(function* () {
        yield* appearance.setSurfaceAlpha(0.625);
        expect(store.get(SURFACE_ALPHA_STORAGE_KEY)).toBe("0.63"); // toFixed(2)
        expect(yield* Queue.take(emitted)).toEqual({ kind: "glass", enabled: true, alpha: 0.625 });
        expect((yield* SubscriptionRef.get(appearance.state)).surfaceAlpha).toBe(0.625);
      }),
    ));

  it("adopts an inbound theme change, persisting it (the heal) but not re-broadcasting", () =>
    run("dock", { [THEME_STORAGE_KEY]: "system" }, ({ appearance, store, emitted, inbound }) =>
      Effect.gen(function* () {
        yield* Queue.offer(inbound, { kind: "theme", theme: "light" });
        const state = yield* awaitWhere(appearance.state.changes, (s) => s.themePref === "light");
        expect(state.themePref).toBe("light");
        // The receiver re-persists the adopted value (heals a failed originator write,
        // matching the old receiving-window apply effects) but never re-emits.
        expect(store.get(THEME_STORAGE_KEY)).toBe("light");
        expect(yield* Queue.size(emitted)).toBe(0);
      }),
    ));

  it("adopts an inbound glass change as a pair and persists both keys (heal)", () =>
    run("dock", {}, ({ appearance, store, inbound }) =>
      Effect.gen(function* () {
        yield* Queue.offer(inbound, { kind: "glass", enabled: false, alpha: 0.8 });
        const state = yield* awaitWhere(
          appearance.state.changes,
          (s) => s.glassEnabled === false && s.surfaceAlpha === 0.8,
        );
        expect(state.glassEnabled).toBe(false);
        expect(state.surfaceAlpha).toBe(0.8);
        expect(store.get(GLASS_STORAGE_KEY)).toBe("off");
        expect(store.get(SURFACE_ALPHA_STORAGE_KEY)).toBe("0.80");
      }),
    ));

  it("reconcile adopts a value changed in shared storage (the focus path)", () =>
    run("settings", { [THEME_STORAGE_KEY]: "dark" }, ({ appearance, store }) =>
      Effect.gen(function* () {
        store.set(ORIENTATION_STORAGE_KEY, "horizontal");
        yield* appearance.reconcile;
        const state = yield* SubscriptionRef.get(appearance.state);
        expect(state.orientationPref).toBe("horizontal");
        expect(state.themePref).toBe("dark");
      }),
    ));

  it("best-effort persist: a failing write still broadcasts and applies in-memory", () =>
    run(
      "settings",
      {},
      ({ appearance, store, emitted }) =>
        Effect.gen(function* () {
          yield* appearance.setTheme("dark");
          // The write threw, so storage was NOT updated...
          expect(store.has(THEME_STORAGE_KEY)).toBe(false);
          // ...but the change still propagated (broadcast) and applied (ref).
          expect(yield* Queue.take(emitted)).toEqual({ kind: "theme", theme: "dark" });
          expect((yield* SubscriptionRef.get(appearance.state)).themePref).toBe("dark");
        }),
      true,
    ));
});
