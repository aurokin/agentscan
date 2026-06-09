import { Duration, Effect, Layer, Queue, Stream, SubscriptionRef } from "effect";
import { describe, expect, it } from "vitest";
import { Profiles } from "./Profiles";
import { PrefsBridge } from "./PrefsBridge";
import { PROFILES_STORAGE_KEY, type ProfileState } from "./profileModel";
import type { PrefsSync } from "./prefs";
import type { ShellMode } from "./prefs";

// --- Fixtures -------------------------------------------------------------

const localProfile = {
  id: "local",
  kind: "local" as const,
  runner: { binaryPath: "", env: [] },
};

const sshProfile = (id: string, host = "box", enabled = true) => ({
  id,
  kind: "ssh" as const,
  host,
  clientTty: "",
  runner: { binaryPath: "", env: [] },
  enabled,
});

const stateOf = (activeProfileId: string, ...profiles: ProfileState["profiles"]): ProfileState => ({
  activeProfileId,
  profiles,
});

const seed = (state: ProfileState): Record<string, string> => ({
  [PROFILES_STORAGE_KEY]: JSON.stringify(state),
});

// A scripted PrefsBridge backed by an in-memory store + an `emitted` queue that
// records outbound broadcasts to assert on. Mirrors how the LiveConnection tests
// script TauriIpc via Layer.succeed. `store` is mutable so a test can simulate a
// concurrent dock-side write landing in storage between the ref and a mutator's
// merge-onto-latest. Profiles never reads `events` (React owns the gated `profiles`
// adoption), so an empty inbound stream satisfies the boundary shape.
const makeBridge = (mode: ShellMode, initial: Record<string, string> = {}) =>
  Effect.gen(function* () {
    const store = new Map<string, string>(Object.entries(initial));
    const emitted = yield* Queue.unbounded<PrefsSync>();
    const layer = Layer.succeed(PrefsBridge, {
      mode,
      loadRaw: (key: string) => store.get(key) ?? null,
      storeRaw: (key: string, value: string) => {
        store.set(key, value);
      },
      emit: (payload: PrefsSync) => Queue.offer(emitted, payload).pipe(Effect.asVoid),
      events: Stream.empty,
    });
    return { layer, store, emitted };
  });

const run = <A>(
  mode: ShellMode,
  initial: Record<string, string>,
  body: (ctx: {
    profiles: Effect.Effect.Success<typeof Profiles>;
    store: Map<string, string>;
    emitted: Queue.Queue<PrefsSync>;
  }) => Effect.Effect<A>,
) =>
  Effect.gen(function* () {
    const bridge = yield* makeBridge(mode, initial);
    const program = Effect.gen(function* () {
      const profiles = yield* Profiles;
      return yield* body({ profiles, store: bridge.store, emitted: bridge.emitted });
    });
    return yield* program.pipe(
      Effect.provide(Profiles.DefaultWithoutDependencies.pipe(Layer.provide(bridge.layer))),
    );
  }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise);

// --- Tests ---------------------------------------------------------------

describe("Profiles", () => {
  it("seeds initial state from storage", () =>
    run("dock", seed(stateOf("local", localProfile, sshProfile("ssh-1"))), ({ profiles }) =>
      Effect.gen(function* () {
        const state = yield* SubscriptionRef.get(profiles.state);
        expect(state.activeProfileId).toBe("local");
        expect(state.profiles.map((p) => p.id)).toEqual(["local", "ssh-1"]);
      }),
    ));

  it("defaults to a single local profile when storage is empty", () =>
    run("dock", {}, ({ profiles }) =>
      Effect.gen(function* () {
        const state = yield* SubscriptionRef.get(profiles.state);
        expect(state.activeProfileId).toBe("local");
        expect(state.profiles).toHaveLength(1);
        expect(state.profiles[0].kind).toBe("local");
      }),
    ));

  it("selectProfile switches active, persists, and broadcasts", () =>
    run(
      "dock",
      seed(stateOf("local", localProfile, sshProfile("ssh-1"))),
      ({ profiles, store, emitted }) =>
        Effect.gen(function* () {
          yield* profiles.selectProfile("ssh-1");
          const state = yield* SubscriptionRef.get(profiles.state);
          expect(state.activeProfileId).toBe("ssh-1");
          // Persisted.
          const persisted = JSON.parse(store.get(PROFILES_STORAGE_KEY)!) as ProfileState;
          expect(persisted.activeProfileId).toBe("ssh-1");
          // Broadcast a signal-only profiles sync.
          const broadcast = yield* Queue.take(emitted);
          expect(broadcast).toEqual({ kind: "profiles" });
        }),
    ));

  it("selectProfile ignores a non-runnable (disabled) profile", () =>
    run(
      "dock",
      seed(stateOf("local", localProfile, sshProfile("ssh-1", "box", false))),
      ({ profiles, emitted }) =>
        Effect.gen(function* () {
          yield* profiles.selectProfile("ssh-1");
          const state = yield* SubscriptionRef.get(profiles.state);
          expect(state.activeProfileId).toBe("local");
          expect(yield* Queue.size(emitted)).toBe(0);
        }),
    ));

  it("selectProfile of the already-active source is a no-op (no broadcast)", () =>
    run("dock", seed(stateOf("local", localProfile)), ({ profiles, emitted }) =>
      Effect.gen(function* () {
        yield* profiles.selectProfile("local");
        expect(yield* Queue.size(emitted)).toBe(0);
      }),
    ));

  it("addSshProfile appends an enabled ssh profile, makes it active, and broadcasts", () =>
    run("settings", seed(stateOf("local", localProfile)), ({ profiles, emitted }) =>
      Effect.gen(function* () {
        yield* profiles.addSshProfile;
        const state = yield* SubscriptionRef.get(profiles.state);
        expect(state.profiles).toHaveLength(2);
        const added = state.profiles.find((p) => p.kind === "ssh")!;
        expect(added.kind).toBe("ssh");
        expect(state.activeProfileId).toBe(added.id);
        expect(yield* Queue.take(emitted)).toEqual({ kind: "profiles" });
      }),
    ));

  it("deleteActiveProfile removes an active ssh profile and falls back to local", () =>
    run(
      "settings",
      seed(stateOf("ssh-1", localProfile, sshProfile("ssh-1"))),
      ({ profiles }) =>
        Effect.gen(function* () {
          yield* profiles.deleteActiveProfile;
          const state = yield* SubscriptionRef.get(profiles.state);
          expect(state.profiles.map((p) => p.id)).toEqual(["local"]);
          expect(state.activeProfileId).toBe("local");
        }),
    ));

  it("deleteActiveProfile is a no-op when the active profile is local", () =>
    run("settings", seed(stateOf("local", localProfile)), ({ profiles, emitted }) =>
      Effect.gen(function* () {
        yield* profiles.deleteActiveProfile;
        const state = yield* SubscriptionRef.get(profiles.state);
        expect(state.profiles.map((p) => p.id)).toEqual(["local"]);
        expect(yield* Queue.size(emitted)).toBe(0);
      }),
    ));

  it("applyRunnerSettings merges onto the LATEST persisted state, not a stale ref", () =>
    run("settings", seed(stateOf("local", localProfile)), ({ profiles, store }) =>
      Effect.gen(function* () {
        // Simulate a concurrent dock-side add landing in storage while this window
        // edits the local profile (its ref still only knows the local profile).
        store.set(
          PROFILES_STORAGE_KEY,
          JSON.stringify(stateOf("local", localProfile, sshProfile("ssh-9", "dock-box"))),
        );

        yield* profiles.applyRunnerSettings({
          runner: { binaryPath: "/opt/agentscan", env: [] },
          sshHost: "",
          sshClientTty: "",
        });

        const state = yield* SubscriptionRef.get(profiles.state);
        // The dock-side add survives, and the edited local profile got the new binary.
        expect(state.profiles.map((p) => p.id)).toEqual(["local", "ssh-9"]);
        const local = state.profiles.find((p) => p.id === "local")!;
        expect(local.runner.binaryPath).toBe("/opt/agentscan");
      }),
    ));

  it("reload reconciles the ref from storage (the dock-adopt + settings focus/clean path)", () =>
    run("settings", seed(stateOf("local", localProfile)), ({ profiles, store }) =>
      Effect.gen(function* () {
        store.set(
          PROFILES_STORAGE_KEY,
          JSON.stringify(stateOf("ssh-1", localProfile, sshProfile("ssh-1"))),
        );
        yield* profiles.reload;
        const state = yield* SubscriptionRef.get(profiles.state);
        expect(state.activeProfileId).toBe("ssh-1");
      }),
    ));
});
