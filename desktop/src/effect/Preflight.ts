import { Effect, Ref, Stream, SubscriptionRef } from "effect";
import { invoke } from "@tauri-apps/api/core";
import { PrefsBridge } from "./PrefsBridge";
import { IpcError } from "./TauriIpc";
import type { PrefsSync, PreflightStatus } from "./prefs";
import type { AgentscanPreflight } from "./profileModel";
import type { DesktopRunnerSettings } from "./types";

const messageOf = (error: unknown) => (error instanceof Error ? error.message : String(error));

// The preflight probe boundary: the single Tauri `invoke` the dock runs to resolve
// whether the active runner's `agentscan` CLI is reachable. Injected as a service so
// the Preflight logic stays pure over it and a vitest layer can script success/failure
// without a real Tauri host. Kept separate from TauriIpc so the live-connection tests'
// scripted boundary is unaffected.
export class PreflightIpc extends Effect.Service<PreflightIpc>()("desktop/PreflightIpc", {
  succeed: {
    probe: (settings: DesktopRunnerSettings) =>
      Effect.tryPromise({
        try: () => invoke<AgentscanPreflight>("preflight_agentscan", { settings }),
        catch: (error) => new IpcError({ op: "preflight_agentscan", message: messageOf(error) }),
      }),
  },
}) {}

// The dock's resolved preflight (CLI reachability for the active runner). Mirrors the
// old App.tsx LoadState, minus its dead `profiles` field — the dock now reads the
// profile list straight from the Profiles service. Lags the active runner by one async
// cycle on a switch, so consumers compare `runnerKey` to the active one before trusting
// it for live decisions.
export type PreflightState =
  | { readonly status: "loading" }
  | {
      readonly status: "ready";
      readonly runnerKey: string;
      readonly preflight: AgentscanPreflight;
    }
  | { readonly status: "failed"; readonly message: string };

// The dock's resolved preflight as the settings window holds it, mirrored over the
// prefs channel. The card reproduces the dock's tones from `status` + `preflight`,
// guarded by `runnerKey` against its own active runner. `preflight` is non-null only
// when the dock status is "ready".
export type SyncedPreflight = {
  readonly status: PreflightStatus;
  readonly runnerKey: string;
  readonly preflight: AgentscanPreflight | null;
};

// What the dock asks Preflight to resolve. The caller (React) precomputes the
// synchronous profile validation: `invalid` non-null short-circuits the probe and
// resolves to a synthetic failed preflight carrying that binary label + message
// (exactly the old loadShellState invalid-profile branch); null means "probe it".
export type PreflightTarget = {
  readonly settings: DesktopRunnerSettings;
  readonly runnerKey: string;
  readonly invalid: { readonly binary: string; readonly error: string } | null;
};

const INITIAL_STATE: PreflightState = { status: "loading" };

// The cross-window wire shape of a resolved preflight. loading/failed carry no
// preflight payload and are stamped with the target's runnerKey (the active runner
// being probed), matching the old dockPreflightSync.
const wireOf = (
  state: PreflightState,
  runnerKey: string,
): Extract<PrefsSync, { kind: "preflight" }> =>
  state.status === "ready"
    ? { kind: "preflight", status: "ready", runnerKey: state.runnerKey, preflight: state.preflight }
    : { kind: "preflight", status: state.status, runnerKey, preflight: null };

// The armed target, gen-stamped so an identical re-configure still re-runs (the old
// effect re-fired on every runnerKey change). `target: null` is the idle target.
type Armed = { readonly gen: number; readonly target: PreflightTarget | null };

const IDLE: Armed = { gen: 0, target: null };

// Owns the dock's resolved preflight (`state`) and the settings window's mirror of it
// (`synced`), plus the cross-window protocol that keeps them in sync over the shared
// PrefsBridge channel — replacing the old emitTo/listen + LoadState/syncedPreflight in
// App.tsx. Only the dock probes (configure is dock-driven); the settings service stays
// idle and just adopts the dock's broadcasts. A supervised fiber runs one probe per
// target and is interrupted by the next configure, superseding an in-flight probe the
// way the old `cancelled` flag did.
export class Preflight extends Effect.Service<Preflight>()("desktop/Preflight", {
  dependencies: [PreflightIpc.Default, PrefsBridge.Default],
  scoped: Effect.gen(function* () {
    const ipc = yield* PreflightIpc;
    const bridge = yield* PrefsBridge;
    const stateRef = yield* SubscriptionRef.make<PreflightState>(INITIAL_STATE);
    const syncedRef = yield* SubscriptionRef.make<SyncedPreflight | null>(null);
    const targetRef = yield* SubscriptionRef.make<Armed>(IDLE);
    // The last wire payload the dock broadcast, so a settings-side replay request
    // (preflight-request) is answered without recomputing — mirrors dockPreflightSyncRef.
    const lastWireRef = yield* Ref.make<Extract<PrefsSync, { kind: "preflight" }>>(
      wireOf(INITIAL_STATE, ""),
    );

    // Set a resolved state and (dock only) mirror it to the settings window.
    const publish = (next: PreflightState, runnerKey: string) =>
      Effect.gen(function* () {
        yield* SubscriptionRef.set(stateRef, next);
        if (bridge.mode === "dock") {
          const wire = wireOf(next, runnerKey);
          yield* Ref.set(lastWireRef, wire);
          yield* bridge.emit(wire);
        }
      });

    // Resolve one target: synthetic-failed for an invalid profile, else probe the CLI.
    const runTarget = ({ target }: Armed): Effect.Effect<never> =>
      Effect.gen(function* () {
        if (target === null) {
          // Idle: the settings window (never configures) and the dock before its first
          // configure. Don't touch state or broadcast — just park until a target lands.
          return yield* Effect.never;
        }
        // Keep an existing ready state through a switch so the dock shows the picker's
        // "Switching…" view (runnerKey mismatch), not the boot screen. Only a first
        // load (no ready yet) drops to loading.
        const current = yield* SubscriptionRef.get(stateRef);
        if (current.status !== "ready") {
          yield* publish({ status: "loading" }, target.runnerKey);
        }
        const resolved: PreflightState = target.invalid
          ? {
              status: "ready",
              runnerKey: target.runnerKey,
              preflight: {
                binary: target.invalid.binary,
                ok: false,
                version: null,
                error: target.invalid.error,
                suggestedBinaryPath: null,
              },
            }
          : yield* ipc.probe(target.settings).pipe(
              Effect.map(
                (preflight): PreflightState => ({
                  status: "ready",
                  runnerKey: target.runnerKey,
                  preflight,
                }),
              ),
              // A probe failure is the old loadShellState catch → a failed state with the
              // IPC error message (Reconnect/Open settings is offered by the dock UI).
              Effect.catchAll((error) =>
                Effect.succeed<PreflightState>({ status: "failed", message: error.message }),
              ),
            );
        yield* publish(resolved, target.runnerKey);
        // Park so the supervisor keeps this fiber (holding the resolved state) until the
        // next target interrupts it.
        return yield* Effect.never;
      });

    // Supervisor: each new target interrupts the running probe and replaces it. A probe
    // interrupted mid-flight drops its result (the Tauri invoke can't be cancelled, but
    // its outcome is discarded) — exactly the old `cancelled` guard.
    yield* targetRef.changes.pipe(
      Stream.changes,
      Stream.flatMap((armed) => Stream.fromEffect(runTarget(armed)), { switch: true }),
      Stream.runDrain,
      Effect.forkScoped,
    );

    // Inbound cross-window handling, forked for the service's lifetime: settings adopts
    // the dock's broadcast preflight; the dock answers a settings-side replay request.
    // Each window ignores the kind it doesn't own (the dock is the producer; settings
    // never probes), so this can't loop.
    yield* bridge.events.pipe(
      Stream.runForEach((payload) =>
        Effect.gen(function* () {
          if (bridge.mode === "settings" && payload.kind === "preflight") {
            yield* SubscriptionRef.set(syncedRef, {
              status: payload.status,
              runnerKey: payload.runnerKey,
              preflight: payload.preflight,
            });
          } else if (bridge.mode === "dock" && payload.kind === "preflight-request") {
            const wire = yield* Ref.get(lastWireRef);
            yield* bridge.emit(wire);
          }
        }),
      ),
      Effect.forkScoped,
    );

    return {
      // The dock's resolved preflight (observed by the dock via an atom).
      state: stateRef,
      // The settings window's mirror of the dock's preflight (observed by settings).
      synced: syncedRef,
      // Dock: set the runner to probe. Bumping gen makes an identical re-configure
      // still re-run, matching the old effect re-firing on every runnerKey change.
      configure: (input: PreflightTarget) =>
        SubscriptionRef.update(targetRef, (current) => ({ gen: current.gen + 1, target: input })),
      // Settings: ask the dock to re-emit its current preflight (emitTo has no replay,
      // so a broadcast missed while this window was hidden is recovered on focus).
      requestSync: bridge.emit({ kind: "preflight-request" }),
    };
  }),
}) {}
