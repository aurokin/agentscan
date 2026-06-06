import { Effect, Stream, SubscriptionRef } from "effect";
import { PrefsBridge } from "./PrefsBridge";
import {
  appearanceEqual,
  loadAppearance,
  storeFrameless,
  storeGlassEnabled,
  storeOrientationPref,
  storeSurfaceAlpha,
  storeThemePref,
  type AppearanceState,
} from "./appearanceModel";
import type { OrientationPreference, ThemePreference } from "./prefs";

// Owns the persisted appearance prefs — theme, dock-layout orientation preference, the
// macOS glass toggle + tint alpha, and the frameless-chrome toggle — as a single
// SubscriptionRef both windows observe via an atom. Replaces the React useState + per-pref
// persist effects + the theme/orientation/glass branches of App.tsx's cross-window listener.
//
// The DOM/Tauri APPLY of these prefs (data-theme, set_window_glass, the CSS vars, window
// shaping, logo variant) stays in React — it is inherently view/host-coupled. This
// service owns only the STATE: persistence, the cross-window broadcast on each user
// action, inbound adoption of the other window's changes, and a value-guarded reconcile.
//
// Unlike Profiles (signal-only `{kind:"profiles"}` broadcasts whose receiver re-reads
// storage), the appearance broadcasts carry the value, so the receiver adopts it directly.
// Persistence is best-effort: a failed write still applies in-memory and propagates to the
// other window, and a stale on-disk value is reconciled on the next focus — exactly the old
// try/catch-wrapped setItem semantics. BOTH windows persist (the originator on the user
// action, the receiver on inbound adoption), matching the old code where the receiving
// window's apply effects re-wrote storage — so a write that fails on the originator is
// healed by the receiver (and vice versa) instead of silently reverting on the next
// reconcile/restart. Each field is updated independently (merge-onto-current), so a
// concurrent change to a different field in the other window can't clobber this one.
export class Appearance extends Effect.Service<Appearance>()("desktop/Appearance", {
  dependencies: [PrefsBridge.Default],
  scoped: Effect.gen(function* () {
    const bridge = yield* PrefsBridge;
    const stateRef = yield* SubscriptionRef.make<AppearanceState>(loadAppearance(bridge.loadRaw));

    // Best-effort persist: a quota/availability failure must NOT abort the apply +
    // broadcast (the value still propagates to the other window over the carried payload,
    // and a stale on-disk value is recovered on the next focus reconcile).
    const persist = (write: () => void) => Effect.try(write).pipe(Effect.ignore);

    // Every user-action setter follows the same order: persist (best-effort) -> apply
    // locally (the ref the local window observes) -> notify the other window. Applying
    // before the broadcast keeps the local update independent of the mirror, matching the
    // old onClick order (setState, then a fire-and-forget broadcastPrefs). `bridge.emit`
    // is itself fire-and-forget (it does not await emitTo and swallows rejection), so it
    // cannot fail the setter, but the ordering makes that independence explicit.
    const setTheme = (themePref: ThemePreference) =>
      Effect.gen(function* () {
        yield* persist(() => storeThemePref(bridge.storeRaw, themePref));
        yield* SubscriptionRef.update(stateRef, (s) => ({ ...s, themePref }));
        yield* bridge.emit({ kind: "theme", theme: themePref });
      });

    const setOrientationPref = (orientationPref: OrientationPreference) =>
      Effect.gen(function* () {
        yield* persist(() => storeOrientationPref(bridge.storeRaw, orientationPref));
        yield* SubscriptionRef.update(stateRef, (s) => ({ ...s, orientationPref }));
        yield* bridge.emit({ kind: "orientation", orientation: orientationPref });
      });

    // The glass toggle and the tint slider each carry BOTH fields in their broadcast (the
    // `{kind:"glass"}` payload), so the receiver always adopts the pair. Each setter
    // persists only the field it changed and reads the other from the current ref.
    const setGlassEnabled = (glassEnabled: boolean) =>
      Effect.gen(function* () {
        yield* persist(() => storeGlassEnabled(bridge.storeRaw, glassEnabled));
        const { surfaceAlpha } = yield* SubscriptionRef.get(stateRef);
        yield* SubscriptionRef.update(stateRef, (s) => ({ ...s, glassEnabled }));
        yield* bridge.emit({ kind: "glass", enabled: glassEnabled, alpha: surfaceAlpha });
      });

    const setSurfaceAlpha = (surfaceAlpha: number) =>
      Effect.gen(function* () {
        yield* persist(() => storeSurfaceAlpha(bridge.storeRaw, surfaceAlpha));
        const { glassEnabled } = yield* SubscriptionRef.get(stateRef);
        yield* SubscriptionRef.update(stateRef, (s) => ({ ...s, surfaceAlpha }));
        yield* bridge.emit({ kind: "glass", enabled: glassEnabled, alpha: surfaceAlpha });
      });

    const setFrameless = (framelessEnabled: boolean) =>
      Effect.gen(function* () {
        yield* persist(() => storeFrameless(bridge.storeRaw, framelessEnabled));
        yield* SubscriptionRef.update(stateRef, (s) => ({ ...s, framelessEnabled }));
        yield* bridge.emit({ kind: "frameless", enabled: framelessEnabled });
      });

    // Value-guarded re-read of the persisted appearance, backing the settings window's
    // focus reconcile (emitTo has no replay, so a broadcast missed while hidden is
    // recovered on the next focus). No persist, no re-broadcast — it only adopts storage.
    const reconcile = SubscriptionRef.update(stateRef, (current) => {
      const reloaded = loadAppearance(bridge.loadRaw);
      return appearanceEqual(current, reloaded) ? current : reloaded;
    });

    // Adopt a remote change: persist it best-effort (the heal — see the class comment),
    // then merge it into the ref. No re-broadcast (that would loop).
    const adopt = (next: Partial<AppearanceState>, writes: ReadonlyArray<() => void>) =>
      Effect.gen(function* () {
        for (const write of writes) {
          yield* persist(write);
        }
        yield* SubscriptionRef.update(stateRef, (s) => ({ ...s, ...next }));
      });

    // Inbound cross-window adoption, forked for the service's lifetime: apply the other
    // window's theme/orientation/glass/frameless change. Other kinds (profiles/preflight)
    // belong to their own owners and are ignored here.
    yield* bridge.events.pipe(
      Stream.runForEach((payload) => {
        switch (payload.kind) {
          case "theme":
            return adopt({ themePref: payload.theme }, [
              () => storeThemePref(bridge.storeRaw, payload.theme),
            ]);
          case "orientation":
            return adopt({ orientationPref: payload.orientation }, [
              () => storeOrientationPref(bridge.storeRaw, payload.orientation),
            ]);
          case "glass":
            return adopt({ glassEnabled: payload.enabled, surfaceAlpha: payload.alpha }, [
              () => storeGlassEnabled(bridge.storeRaw, payload.enabled),
              () => storeSurfaceAlpha(bridge.storeRaw, payload.alpha),
            ]);
          case "frameless":
            return adopt({ framelessEnabled: payload.enabled }, [
              () => storeFrameless(bridge.storeRaw, payload.enabled),
            ]);
          default:
            return Effect.void;
        }
      }),
      Effect.forkScoped,
    );

    return {
      state: stateRef,
      setTheme,
      setOrientationPref,
      setGlassEnabled,
      setSurfaceAlpha,
      setFrameless,
      reconcile,
    };
  }),
}) {}
