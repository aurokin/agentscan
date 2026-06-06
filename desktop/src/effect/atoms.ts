import { Atom } from "@effect-atom/atom-react";
import { Effect, Layer } from "effect";
import { LiveConnection, type ConfigureInput } from "./LiveConnection";
import { Profiles, type ApplyRunnerSettingsInput } from "./Profiles";

// One runtime per webview window, providing the desktop Effect services. Profiles
// pulls in the shared PrefsBridge (the single agentscan:prefs-sync listener), so a
// later Preflight/Appearance service added here reuses the same channel rather than
// opening its own. Both windows instantiate this layer; LiveConnection's supervisor
// idles in the settings window because the dock-only configure path never enables a
// target (the latch-only invariant is enforced there, not by withholding the layer).
const runtime = Atom.runtime(Layer.mergeAll(LiveConnection.Default, Profiles.Default));

// --- Live connection slice ---

// The live state the dock observes: Result<LiveState> (connection status + rows).
// keepAlive so the supervised connection fiber persists across re-renders/StrictMode
// remounts for the dock session rather than tearing down when momentarily unmounted.
export const liveStateAtom = Atom.keepAlive(
  runtime.subscriptionRef(Effect.map(LiveConnection, (lc) => lc.state)),
);

// The only path that may spawn a daemon — the "Start agentscan" affordance.
export const startAtom = runtime.fn(
  Effect.fnUntraced(function* () {
    const lc = yield* LiveConnection;
    yield* lc.start;
  }),
);

// Re-arm the live connection now (latch only). Backs the Refresh button and the
// fatal-state Reconnect action.
export const reconnectAtom = runtime.fn(
  Effect.fnUntraced(function* () {
    const lc = yield* LiveConnection;
    yield* lc.reconnect;
  }),
);

// Re-target the connection when the active profile/preflight changes.
export const configureAtom = runtime.fn(
  Effect.fnUntraced(function* (input: ConfigureInput) {
    const lc = yield* LiveConnection;
    yield* lc.configure(input);
  }),
);

// --- Profiles / settings slice ---

// The persisted profile state both windows observe: Result<ProfileState>. keepAlive
// so the Profiles supervisor (inbound cross-window adoption) and the shared
// PrefsBridge persist across StrictMode remounts.
export const profilesAtom = Atom.keepAlive(
  runtime.subscriptionRef(Effect.map(Profiles, (p) => p.state)),
);

export const selectProfileAtom = runtime.fn(
  Effect.fnUntraced(function* (id: string) {
    const profiles = yield* Profiles;
    yield* profiles.selectProfile(id);
  }),
);

export const addSshProfileAtom = runtime.fn(
  Effect.fnUntraced(function* () {
    const profiles = yield* Profiles;
    yield* profiles.addSshProfile;
  }),
);

export const deleteActiveProfileAtom = runtime.fn(
  Effect.fnUntraced(function* () {
    const profiles = yield* Profiles;
    yield* profiles.deleteActiveProfile;
  }),
);

export const applyRunnerSettingsAtom = runtime.fn(
  Effect.fnUntraced(function* (input: ApplyRunnerSettingsInput) {
    const profiles = yield* Profiles;
    yield* profiles.applyRunnerSettings(input);
  }),
);

// Value-guarded reconcile from storage, driven by React on the cross-window profiles
// sync and the settings window's focus/clean transitions (emitTo has no replay).
export const reloadProfilesAtom = runtime.fn(
  Effect.fnUntraced(function* () {
    const profiles = yield* Profiles;
    yield* profiles.reload;
  }),
);
