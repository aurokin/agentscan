import { Deferred, Duration, Effect, Layer, Option, Queue, Ref, Stream, SubscriptionRef } from "effect";
import { describe, expect, it } from "vitest";
import { Preflight, PreflightIpc } from "./Preflight";
import { PrefsBridge } from "./PrefsBridge";
import { IpcError } from "./TauriIpc";
import type { PrefsSync, ShellMode } from "./prefs";
import type { AgentscanPreflight } from "./profileModel";
import type { DesktopRunnerSettings } from "./types";

// --- Fixtures -------------------------------------------------------------

const SETTINGS: DesktopRunnerSettings = { kind: "local", binaryPath: "", env: [] };
const PREFLIGHT: AgentscanPreflight = {
  binary: "agentscan",
  ok: true,
  version: "1.0.0",
  error: null,
  suggestedBinaryPath: null,
  remoteHostLabel: null,
};

// A scripted PrefsBridge: `emitted` records outbound broadcasts to assert on, `inbound`
// drives the cross-window stream the service consumes. Mirrors how the LiveConnection
// tests script TauriIpc via Layer.succeed. loadRaw/storeRaw are inert (Preflight never
// touches storage).
const bridgeLayer = (
  mode: ShellMode,
  emitted: Queue.Queue<PrefsSync>,
  inbound: Queue.Dequeue<PrefsSync>,
) =>
  Layer.succeed(PrefsBridge, {
    mode,
    loadRaw: () => null,
    storeRaw: () => {},
    emit: (payload: PrefsSync) => Queue.offer(emitted, payload).pipe(Effect.asVoid),
    events: Stream.fromQueue(inbound),
  });

const ipcLayer = (probe: PreflightIpc["probe"]) => Layer.succeed(PreflightIpc, { probe });

const withDeps = <A, E, R>(
  program: Effect.Effect<A, E, R>,
  bridge: Layer.Layer<PrefsBridge>,
  probe: PreflightIpc["probe"],
) =>
  program.pipe(
    Effect.provide(
      Preflight.DefaultWithoutDependencies.pipe(
        Layer.provide(Layer.merge(bridge, ipcLayer(probe))),
      ),
    ),
  );

// Block until a SubscriptionRef's stream emits a value matching `pred` (changes replays
// the current value, so this resolves immediately if already there).
const awaitWhere = <A>(stream: Stream.Stream<A>, pred: (a: A) => boolean) =>
  stream.pipe(
    Stream.filter(pred),
    Stream.runHead,
    Effect.flatMap(
      Option.match({
        onNone: () => Effect.die("stream ended early"),
        onSome: Effect.succeed,
      }),
    ),
  );

// --- Tests ---------------------------------------------------------------

describe("Preflight", () => {
  it("dock: probes a valid target, resolves ready, and broadcasts loading then ready", () =>
    Effect.gen(function* () {
      const emitted = yield* Queue.unbounded<PrefsSync>();
      const inbound = yield* Queue.unbounded<PrefsSync>();

      const program = Effect.gen(function* () {
        const preflight = yield* Preflight;
        yield* preflight.configure({ settings: SETTINGS, runnerKey: "k1", invalid: null });

        const ready = yield* awaitWhere(preflight.state.changes, (s) => s.status === "ready");
        expect(ready).toEqual({ status: "ready", runnerKey: "k1", preflight: PREFLIGHT });

        // Mirrored to settings: a loading frame, then the resolved ready frame.
        expect(yield* Queue.take(emitted)).toEqual({
          kind: "preflight",
          status: "loading",
          runnerKey: "k1",
          preflight: null,
        });
        expect(yield* Queue.take(emitted)).toEqual({
          kind: "preflight",
          status: "ready",
          runnerKey: "k1",
          preflight: PREFLIGHT,
        });
      });

      yield* withDeps(program, bridgeLayer("dock", emitted, inbound), () =>
        Effect.succeed(PREFLIGHT),
      );
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));

  it("dock: an invalid target resolves a synthetic failed preflight WITHOUT probing", () =>
    Effect.gen(function* () {
      const emitted = yield* Queue.unbounded<PrefsSync>();
      const inbound = yield* Queue.unbounded<PrefsSync>();
      const probeCalls = yield* Ref.make(0);

      const program = Effect.gen(function* () {
        const preflight = yield* Preflight;
        yield* preflight.configure({
          settings: SETTINGS,
          runnerKey: "k1",
          invalid: { binary: "ssh box agentscan", error: "Host is required." },
        });

        const ready = yield* awaitWhere(preflight.state.changes, (s) => s.status === "ready");
        expect(ready).toEqual({
          status: "ready",
          runnerKey: "k1",
          preflight: {
            binary: "ssh box agentscan",
            ok: false,
            version: null,
            error: "Host is required.",
            suggestedBinaryPath: null,
            remoteHostLabel: null,
          },
        });
        // The synthetic branch never touches the probe boundary.
        expect(yield* Ref.get(probeCalls)).toBe(0);
      });

      yield* withDeps(program, bridgeLayer("dock", emitted, inbound), () =>
        Effect.flatMap(Ref.update(probeCalls, (n) => n + 1), () => Effect.succeed(PREFLIGHT)),
      );
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));

  it("dock: a probe failure resolves to a failed state carrying the error message", () =>
    Effect.gen(function* () {
      const emitted = yield* Queue.unbounded<PrefsSync>();
      const inbound = yield* Queue.unbounded<PrefsSync>();

      const program = Effect.gen(function* () {
        const preflight = yield* Preflight;
        yield* preflight.configure({ settings: SETTINGS, runnerKey: "k1", invalid: null });

        const failed = yield* awaitWhere(preflight.state.changes, (s) => s.status === "failed");
        expect(failed).toEqual({ status: "failed", message: "boom: command failed" });
      });

      yield* withDeps(program, bridgeLayer("dock", emitted, inbound), () =>
        Effect.fail(new IpcError({ op: "preflight_agentscan", message: "boom: command failed" })),
      );
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));

  it("dock: keeps the previous ready preflight while the next target's probe is in flight", () =>
    Effect.gen(function* () {
      const emitted = yield* Queue.unbounded<PrefsSync>();
      const inbound = yield* Queue.unbounded<PrefsSync>();
      // Gate the SECOND probe so the switch can be observed mid-flight. `started` lets
      // the test wait until a probe has begun (and read the latest state) deterministically.
      const calls = yield* Ref.make(0);
      const started = yield* Queue.unbounded<number>();
      const gate = yield* Deferred.make<void>();

      const probe: PreflightIpc["probe"] = () =>
        Effect.gen(function* () {
          const n = yield* Ref.updateAndGet(calls, (x) => x + 1);
          yield* Queue.offer(started, n);
          if (n >= 2) {
            yield* Deferred.await(gate);
          }
          return PREFLIGHT;
        });

      const program = Effect.gen(function* () {
        const preflight = yield* Preflight;

        // Source k1 latches ready.
        yield* preflight.configure({ settings: SETTINGS, runnerKey: "k1", invalid: null });
        yield* Queue.take(started); // probe 1
        yield* awaitWhere(preflight.state.changes, (s) => s.status === "ready");

        // Switch to k2: its probe is gated. The service must KEEP k1's ready state (so the
        // dock shows "Switching…" via the runnerKey mismatch), not flash loading.
        yield* preflight.configure({ settings: SETTINGS, runnerKey: "k2", invalid: null });
        yield* Queue.take(started); // probe 2 has begun (state already read + kept)
        expect(yield* SubscriptionRef.get(preflight.state)).toEqual({
          status: "ready",
          runnerKey: "k1",
          preflight: PREFLIGHT,
        });

        // Release k2's probe → it resolves and replaces the kept state.
        yield* Deferred.succeed(gate, undefined);
        const ready = yield* awaitWhere(
          preflight.state.changes,
          (s) => s.status === "ready" && s.runnerKey === "k2",
        );
        expect(ready).toEqual({ status: "ready", runnerKey: "k2", preflight: PREFLIGHT });
      });

      yield* withDeps(program, bridgeLayer("dock", emitted, inbound), probe);
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));

  it("settings: adopts the dock's broadcast preflight into `synced`", () =>
    Effect.gen(function* () {
      const emitted = yield* Queue.unbounded<PrefsSync>();
      const inbound = yield* Queue.unbounded<PrefsSync>();

      const program = Effect.gen(function* () {
        const preflight = yield* Preflight;
        // The dock pushes its resolved preflight over the channel.
        yield* Queue.offer(inbound, {
          kind: "preflight",
          status: "ready",
          runnerKey: "k1",
          preflight: PREFLIGHT,
        });

        const synced = yield* awaitWhere(preflight.synced.changes, (s) => s !== null);
        expect(synced).toEqual({ status: "ready", runnerKey: "k1", preflight: PREFLIGHT });
        // The settings window never probes; it stays at the initial loading state.
        expect(yield* SubscriptionRef.get(preflight.state)).toEqual({ status: "loading" });
      });

      yield* withDeps(program, bridgeLayer("settings", emitted, inbound), () =>
        Effect.die("settings must not probe"),
      );
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));

  it("dock: answers a settings-side replay request by re-emitting its last preflight", () =>
    Effect.gen(function* () {
      const emitted = yield* Queue.unbounded<PrefsSync>();
      const inbound = yield* Queue.unbounded<PrefsSync>();

      const program = Effect.gen(function* () {
        const preflight = yield* Preflight;
        yield* preflight.configure({ settings: SETTINGS, runnerKey: "k1", invalid: null });
        yield* awaitWhere(preflight.state.changes, (s) => s.status === "ready");
        // Drain the proactive loading + ready broadcasts.
        yield* Queue.take(emitted);
        yield* Queue.take(emitted);

        // A settings window asks for a replay → the dock re-emits its current preflight.
        yield* Queue.offer(inbound, { kind: "preflight-request" });
        expect(yield* Queue.take(emitted)).toEqual({
          kind: "preflight",
          status: "ready",
          runnerKey: "k1",
          preflight: PREFLIGHT,
        });
      });

      yield* withDeps(program, bridgeLayer("dock", emitted, inbound), () =>
        Effect.succeed(PREFLIGHT),
      );
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));
});
