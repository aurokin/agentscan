import { Atom } from "@effect-atom/atom-react";
import { Effect, Layer } from "effect";
import { LiveConnection, type ConfigureInput } from "./LiveConnection";
import { Profiles, type ApplyRunnerSettingsInput } from "./Profiles";
import { Preflight, type PreflightTarget } from "./Preflight";
import { Appearance } from "./Appearance";
import type { OrientationPreference, ThemePreference } from "./prefs";

// One runtime per webview window, providing the desktop Effect services. Profiles,
// Preflight, and Appearance all pull in the shared PrefsBridge (the single
// agentscan:prefs-sync channel), so they reuse one listener rather than opening their
// own — Effect memoizes PrefsBridge.Default by reference across the merge. Both windows
// instantiate this layer; LiveConnection's and Preflight's supervisors idle in the
// settings window because the dock-only configure paths never enable a target (the
// latch-only invariant is enforced there, not by withholding the layer).
const runtime = Atom.runtime(
  Layer.mergeAll(
    LiveConnection.Default,
    Profiles.Default,
    Preflight.Default,
    Appearance.Default,
  ),
);

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

// --- Preflight slice ---

// The dock's resolved preflight (CLI reachability for the active runner): Result<
// PreflightState>. keepAlive so the supervised probe fiber + the shared PrefsBridge
// listener persist across StrictMode remounts for the dock session.
export const preflightStateAtom = Atom.keepAlive(
  runtime.subscriptionRef(Effect.map(Preflight, (p) => p.state)),
);

// The settings window's mirror of the dock's preflight: Result<SyncedPreflight | null>.
// keepAlive so the inbound-adoption fiber persists across remounts.
export const syncedPreflightAtom = Atom.keepAlive(
  runtime.subscriptionRef(Effect.map(Preflight, (p) => p.synced)),
);

// Dock-only: point Preflight at the active runner (re-probe). Driven by React on every
// runnerKey change, with the synchronous profile validation precomputed into `invalid`.
export const configurePreflightAtom = runtime.fn(
  Effect.fnUntraced(function* (input: PreflightTarget) {
    const preflight = yield* Preflight;
    yield* preflight.configure(input);
  }),
);

// Settings-only: ask the dock to re-emit its current preflight (focus reconcile).
export const requestPreflightSyncAtom = runtime.fn(
  Effect.fnUntraced(function* () {
    const preflight = yield* Preflight;
    yield* preflight.requestSync;
  }),
);

// --- Appearance slice ---

// The persisted appearance triad (theme + dock-layout orientation + glass) both windows
// observe: Result<AppearanceState>. keepAlive so the inbound-adoption fiber + shared
// PrefsBridge persist across StrictMode remounts. React keeps the DOM/Tauri apply effects.
export const appearanceAtom = Atom.keepAlive(
  runtime.subscriptionRef(Effect.map(Appearance, (a) => a.state)),
);

export const setThemeAtom = runtime.fn(
  Effect.fnUntraced(function* (theme: ThemePreference) {
    const appearance = yield* Appearance;
    yield* appearance.setTheme(theme);
  }),
);

export const setOrientationAtom = runtime.fn(
  Effect.fnUntraced(function* (orientation: OrientationPreference) {
    const appearance = yield* Appearance;
    yield* appearance.setOrientationPref(orientation);
  }),
);

export const setGlassEnabledAtom = runtime.fn(
  Effect.fnUntraced(function* (enabled: boolean) {
    const appearance = yield* Appearance;
    yield* appearance.setGlassEnabled(enabled);
  }),
);

export const setSurfaceAlphaAtom = runtime.fn(
  Effect.fnUntraced(function* (alpha: number) {
    const appearance = yield* Appearance;
    yield* appearance.setSurfaceAlpha(alpha);
  }),
);

// Value-guarded reconcile from storage, driven by React on the settings window's focus
// (emitTo has no replay, so a change missed while hidden is recovered on focus).
export const reloadAppearanceAtom = runtime.fn(
  Effect.fnUntraced(function* () {
    const appearance = yield* Appearance;
    yield* appearance.reconcile;
  }),
);
