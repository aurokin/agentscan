import {
  Deferred,
  Duration,
  Effect,
  Layer,
  Option,
  Queue,
  Ref,
  Stream,
  SubscriptionRef,
} from "effect";
import { describe, expect, it } from "vitest";
import { HostnameEnrichment } from "./HostnameEnrichment";
import { LiveConnection, type LiveStates } from "./LiveConnection";
import { Preflight, PreflightIpc, type PreflightState, type SyncedPreflight } from "./Preflight";
import { PrefsBridge } from "./PrefsBridge";
import { Profiles } from "./Profiles";
import { IpcError } from "./TauriIpc";
import {
  PROFILES_STORAGE_KEY,
  runnerKeyForProfile,
  type AgentscanPreflight,
  type ProfileState,
} from "./profileModel";
import type { DesktopRunnerSettings, LiveState } from "./types";

// --- Fixtures -------------------------------------------------------------

const LOCAL = { id: "local", kind: "local" as const, runner: { binaryPath: "", env: [] } };

const ssh = (id: string, host: string, probedHost?: string) => ({
  id,
  kind: "ssh" as const,
  host,
  clientTty: "",
  runner: { binaryPath: "", env: [] },
  enabled: true,
  ...(probedHost ? { probedHost } : {}),
});

const seed = (activeProfileId: string, profiles: unknown[]): Record<string, string> => ({
  [PROFILES_STORAGE_KEY]: JSON.stringify({ activeProfileId, profiles }),
});

const preflightOf = (remoteHostLabel: string | null): AgentscanPreflight => ({
  ok: true,
  binary: "agentscan",
  version: "0.7.2",
  error: null,
  remoteHostLabel,
  suggestedBinaryPath: null,
});

const online = (runnerKey: string): LiveState => ({
  connection: {
    status: "online",
    message: "ok",
    snapshot: { paneCount: 0, generatedAt: null, sourceKind: "tmux" },
  },
  rows: [],
  rowsRunnerKey: runnerKey,
});

// --- Harness ----------------------------------------------------------------

// Real Profiles over a scripted in-memory PrefsBridge (so the model-layer
// re-verification the service leans on is actually exercised), with
// Layer.succeed mocks for Preflight (a test-driven state ref), LiveConnection
// (a test-driven states map), and PreflightIpc (a scripted probe queue).
const makeHarness = (input: {
  initial: Record<string, string>;
  probes: ReadonlyArray<Effect.Effect<AgentscanPreflight, IpcError>>;
}) =>
  Effect.gen(function* () {
    const store = new Map<string, string>(Object.entries(input.initial));
    const Bridge = Layer.succeed(PrefsBridge, {
      mode: "dock" as const,
      loadRaw: (key: string) => store.get(key) ?? null,
      storeRaw: (key: string, value: string) => {
        store.set(key, value);
      },
      emit: () => Effect.void,
      events: Stream.empty,
    });
    const RealProfiles = Profiles.DefaultWithoutDependencies.pipe(Layer.provide(Bridge));

    const preflightState = yield* SubscriptionRef.make<PreflightState>({ status: "loading" });
    const syncedRef = yield* SubscriptionRef.make<SyncedPreflight | null>(null);
    const MockPreflight = Layer.succeed(Preflight, {
      state: preflightState,
      synced: syncedRef,
      configure: () => Effect.void,
      requestSync: Effect.void,
    });

    const liveStates = yield* SubscriptionRef.make<LiveStates>(new Map());
    const MockLive = Layer.succeed(LiveConnection, {
      states: liveStates,
      configure: () => Effect.void,
      reconnect: () => Effect.void,
      start: () => Effect.void,
    });

    const probeCalls = yield* Queue.unbounded<DesktopRunnerSettings>();
    const outcomes = yield* Ref.make(input.probes);
    const MockIpc = Layer.succeed(PreflightIpc, {
      probe: (settings: DesktopRunnerSettings) =>
        Effect.gen(function* () {
          yield* Queue.offer(probeCalls, settings);
          const next = yield* Ref.modify(outcomes, (rest) => [rest[0], rest.slice(1)] as const);
          return yield* next ?? Effect.die("unscripted probe call");
        }),
    });

    // The same dep layer REFERENCE feeds the service and the test program, so
    // Effect's layer memoization yields one Profiles instance for both.
    const deps = Layer.mergeAll(MockIpc, MockPreflight, MockLive, RealProfiles);
    return {
      layer: Layer.mergeAll(
        HostnameEnrichment.DefaultWithoutDependencies.pipe(Layer.provide(deps)),
        deps,
      ),
      store,
      preflightState,
      liveStates,
      probeCalls,
    };
  });

const failWith = (message: string) =>
  Effect.fail(new IpcError({ op: "preflight_agentscan", message }));

// Block until the given profile carries the expected probedHost.
const awaitProbedHost = (
  state: SubscriptionRef.SubscriptionRef<ProfileState>,
  id: string,
  probedHost: string,
): Effect.Effect<void> =>
  state.changes.pipe(
    Stream.filter((s) =>
      s.profiles.some((p) => p.id === id && p.kind === "ssh" && p.probedHost === probedHost),
    ),
    Stream.runHead,
    Effect.flatMap(
      Option.match({
        onNone: () => Effect.die("profiles stream ended early"),
        onSome: () => Effect.void,
      }),
    ),
  );

const probedHostOf = (state: ProfileState, id: string): string | undefined => {
  const profile = state.profiles.find((p) => p.id === id);
  return profile?.kind === "ssh" ? profile.probedHost : undefined;
};

// Let the supervisors and forked probe fibers process a tick — everything in
// the harness is scheduler-driven (no real timers), so yielding is sufficient.
const settle = Effect.yieldNow().pipe(Effect.repeatN(60));

const run = <A>(
  harness: Effect.Effect.Success<ReturnType<typeof makeHarness>>,
  body: (ctx: {
    enrichment: Effect.Effect.Success<typeof HostnameEnrichment>;
    profiles: Effect.Effect.Success<typeof Profiles>;
  }) => Effect.Effect<A>,
) =>
  Effect.gen(function* () {
    const enrichment = yield* HostnameEnrichment;
    const profiles = yield* Profiles;
    return yield* body({ enrichment, profiles });
  }).pipe(Effect.provide(harness.layer));

describe("HostnameEnrichment", () => {
  it("probes a never-probed non-active source once online, logs the command, and persists the hostname exactly once", () =>
    Effect.gen(function* () {
      const initial = seed("local", [LOCAL, ssh("s1", "box")]);
      const k1 = runnerKeyForProfile(ssh("s1", "box"));
      const harness = yield* makeHarness({
        initial,
        probes: [Effect.succeed(preflightOf("boxy"))],
      });
      const log: string[] = [];

      yield* run(harness, ({ enrichment, profiles }) =>
        Effect.gen(function* () {
          yield* enrichment.configure((label, detail) => log.push(`${label}|${detail}`));
          // Not a candidate until its channel comes ONLINE.
          yield* settle;
          expect(log).toEqual([]);

          yield* SubscriptionRef.set(harness.liveStates, new Map([[k1, online(k1)]]));
          yield* awaitProbedHost(profiles.state, "s1", "boxy");
          expect(log).toEqual(["hostname probe (box)|started", "hostname probe (box)|ok"]);

          // Recording committed a profiles tick and the live map keeps
          // ticking; neither re-probes (probedHost + the attempt mark).
          yield* SubscriptionRef.set(harness.liveStates, new Map([[k1, online(k1)]]));
          yield* settle;
          expect(yield* Queue.size(harness.probeCalls)).toBe(1);
        }),
      );
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));

  it("a failed probe logs the raw detail and retries only after its key leaves and returns", () =>
    Effect.gen(function* () {
      const initial = seed("local", [LOCAL, ssh("s1", "box")]);
      const k1 = runnerKeyForProfile(ssh("s1", "box"));
      const harness = yield* makeHarness({
        initial,
        probes: [failWith("ssh: connect refused"), Effect.succeed(preflightOf("boxy"))],
      });
      const log: string[] = [];

      yield* run(harness, ({ enrichment, profiles }) =>
        Effect.gen(function* () {
          yield* enrichment.configure((label, detail) => log.push(`${label}|${detail}`));
          yield* SubscriptionRef.set(harness.liveStates, new Map([[k1, online(k1)]]));
          yield* settle;
          expect(log).toEqual([
            "hostname probe (box)|started",
            "hostname probe (box)|ssh: connect refused",
          ]);

          // The failure stands: further live ticks don't re-probe...
          yield* SubscriptionRef.set(harness.liveStates, new Map([[k1, online(k1)]]));
          yield* settle;
          expect(yield* Queue.size(harness.probeCalls)).toBe(1);

          // ...until the key leaves the source list (delete) and returns
          // (re-add with the same connection), which forgets the attempt.
          harness.store.set(PROFILES_STORAGE_KEY, JSON.stringify({ activeProfileId: "local", profiles: [LOCAL] }));
          yield* profiles.reload;
          yield* settle;
          harness.store.set(PROFILES_STORAGE_KEY, JSON.stringify({ activeProfileId: "local", profiles: [LOCAL, ssh("s1", "box")] }));
          yield* profiles.reload;
          yield* awaitProbedHost(profiles.state, "s1", "boxy");
          expect(yield* Queue.size(harness.probeCalls)).toBe(2);
        }),
      );
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));

  it("a probe that resolves after its profile was retargeted records nothing (stale runnerKey dropped)", () =>
    Effect.gen(function* () {
      const initial = seed("local", [LOCAL, ssh("s1", "box")]);
      const k1 = runnerKeyForProfile(ssh("s1", "box"));
      const gate = yield* Deferred.make<void>();
      const harness = yield* makeHarness({
        initial,
        probes: [Deferred.await(gate).pipe(Effect.zipRight(Effect.succeed(preflightOf("old-box"))))],
      });

      yield* run(harness, ({ enrichment, profiles }) =>
        Effect.gen(function* () {
          yield* enrichment.configure(() => {});
          yield* SubscriptionRef.set(harness.liveStates, new Map([[k1, online(k1)]]));
          yield* Queue.take(harness.probeCalls); // probe armed, in flight

          // Host edit while the probe is in flight: the runnerKey moves.
          harness.store.set(
            PROFILES_STORAGE_KEY,
            JSON.stringify({ activeProfileId: "local", profiles: [LOCAL, ssh("s1", "elsewhere")] }),
          );
          yield* profiles.reload;
          yield* Deferred.succeed(gate, undefined);
          yield* settle;

          // The stale result is dropped by the model-layer re-verification.
          expect(probedHostOf(yield* SubscriptionRef.get(profiles.state), "s1")).toBeUndefined();
        }),
      );
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));

  it("records the driver's resolved probe onto the runnerKey's owner even when no longer active, and skips unknown keys", () =>
    Effect.gen(function* () {
      // Active is LOCAL: the old App effect would have skipped this recording
      // (matchedPreflight gate); the service records onto the owner — the
      // deliberate strict-superset deviation.
      const initial = seed("local", [LOCAL, ssh("s1", "box")]);
      const k1 = runnerKeyForProfile(ssh("s1", "box"));
      const harness = yield* makeHarness({ initial, probes: [] });

      yield* run(harness, ({ enrichment, profiles }) =>
        Effect.gen(function* () {
          yield* enrichment.configure(() => {});
          // A ready state for a runnerKey no profile owns is skipped — and must
          // not kill the recorder for the session.
          yield* SubscriptionRef.set(harness.preflightState, {
            status: "ready",
            runnerKey: "unknown-runner",
            preflight: preflightOf("ghost"),
          });
          yield* settle;
          expect(probedHostOf(yield* SubscriptionRef.get(profiles.state), "s1")).toBeUndefined();

          yield* SubscriptionRef.set(harness.preflightState, {
            status: "ready",
            runnerKey: k1,
            preflight: preflightOf("boxy"),
          });
          yield* awaitProbedHost(profiles.state, "s1", "boxy");
          // No background probe was ever needed.
          expect(yield* Queue.size(harness.probeCalls)).toBe(0);
        }),
      );
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));

  it("a re-configure mid-probe neither cancels nor duplicates the in-flight probe", () =>
    Effect.gen(function* () {
      const initial = seed("local", [LOCAL, ssh("s1", "box")]);
      const k1 = runnerKeyForProfile(ssh("s1", "box"));
      const gate = yield* Deferred.make<void>();
      const harness = yield* makeHarness({
        initial,
        probes: [Deferred.await(gate).pipe(Effect.zipRight(Effect.succeed(preflightOf("boxy"))))],
      });
      const log: string[] = [];

      yield* run(harness, ({ enrichment, profiles }) =>
        Effect.gen(function* () {
          yield* enrichment.configure((label, detail) => log.push(`${label}|${detail}`));
          yield* SubscriptionRef.set(harness.liveStates, new Map([[k1, online(k1)]]));
          yield* Queue.take(harness.probeCalls); // in flight

          // StrictMode's second configure: the replayed tick must skip the
          // marked key, and the surviving probe fiber must still record.
          yield* enrichment.configure((label, detail) => log.push(`${label}|${detail}`));
          yield* settle;
          expect(yield* Queue.size(harness.probeCalls)).toBe(0);

          yield* Deferred.succeed(gate, undefined);
          yield* awaitProbedHost(profiles.state, "s1", "boxy");
          expect(log).toEqual(["hostname probe (box)|started", "hostname probe (box)|ok"]);
        }),
      );
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));

  it("an ok probe with no hostname logs ok, records nothing, and is not retried", () =>
    Effect.gen(function* () {
      const initial = seed("local", [LOCAL, ssh("s1", "box")]);
      const k1 = runnerKeyForProfile(ssh("s1", "box"));
      const harness = yield* makeHarness({
        initial,
        probes: [Effect.succeed(preflightOf(null))],
      });
      const log: string[] = [];

      yield* run(harness, ({ enrichment, profiles }) =>
        Effect.gen(function* () {
          yield* enrichment.configure((label, detail) => log.push(`${label}|${detail}`));
          yield* SubscriptionRef.set(harness.liveStates, new Map([[k1, online(k1)]]));
          yield* settle;
          expect(log).toEqual(["hostname probe (box)|started", "hostname probe (box)|ok"]);
          expect(probedHostOf(yield* SubscriptionRef.get(profiles.state), "s1")).toBeUndefined();

          yield* SubscriptionRef.set(harness.liveStates, new Map([[k1, online(k1)]]));
          yield* settle;
          expect(yield* Queue.size(harness.probeCalls)).toBe(1);
        }),
      );
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));

  it("stays fully inert when never configured (the settings window)", () =>
    Effect.gen(function* () {
      const initial = seed("local", [LOCAL, ssh("s1", "box")]);
      const k1 = runnerKeyForProfile(ssh("s1", "box"));
      const harness = yield* makeHarness({ initial, probes: [] });

      yield* run(harness, ({ profiles }) =>
        Effect.gen(function* () {
          // Online candidate AND a resolved preflight with a hostname — but no
          // configure means no supervisors, so nothing probes or records.
          yield* SubscriptionRef.set(harness.liveStates, new Map([[k1, online(k1)]]));
          yield* SubscriptionRef.set(harness.preflightState, {
            status: "ready",
            runnerKey: k1,
            preflight: preflightOf("boxy"),
          });
          yield* settle;
          expect(yield* Queue.size(harness.probeCalls)).toBe(0);
          expect(probedHostOf(yield* SubscriptionRef.get(profiles.state), "s1")).toBeUndefined();
        }),
      );
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));
});
