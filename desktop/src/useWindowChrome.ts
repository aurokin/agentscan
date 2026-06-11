// The dock window's chrome driver: one hook owning every native/DOM apply for
// the appearance preferences — theme, live orientation tracking, window
// shape/constraints, the macOS glass backdrop, the surface-alpha tokens, the
// frameless decorations toggle — plus arming the summon hotkey (whose placement
// follows the live orientation). DockApp is the only caller: glass, frameless,
// and the summon hotkey are dock-only by design (the settings window keeps its
// native frame and never frosts, reshapes, or binds the shortcut), so this hook
// must never be mounted twice. The hook owns DRIVING the applies; the queued-op
// bodies live in windowOperations (with injectable sinks, transcript-tested),
// the pure shape plan in effect/windowChromeModel, and atom-bound collaborators
// (the debug-log appender, the summon-hotkey setter) arrive as plain function
// args — the useSettingsForm precedent. Moving here consolidated the seven
// effects into one commit block at the hook call site (they used to interleave
// with DockApp's other effects); verified benign because nothing outside this
// hook reads the chrome DOM attributes (data-theme/data-glass/data-frameless,
// the CSS vars) synchronously during mount — CSS and the queued native calls
// are the only consumers.

import { useEffect, useRef, useState } from "react";
import type { LogoTheme } from "./providerLogos";
import { glassClearFor } from "./effect/appearanceModel";
import type { DebugEntryInput } from "./effect/DebugLog";
import type { Orientation, OrientationPreference, ThemePreference } from "./effect/prefs";
import { sizePlanFor } from "./effect/windowChromeModel";
import { IS_MAC } from "./platform";
import {
  applyFrameless,
  applyGlass,
  applyWindowShape,
  enqueueFramelessOperation,
  enqueueGlassOperation,
  enqueueWindowOperation,
  FRAMELESS_CORNER_RADIUS,
  placeBarWindow,
  placePickerWindow,
  raisePickerWindow,
} from "./windowOperations";
import { errorMessage } from "./shared";

// Appearance prefs (storage keys, alpha bounds, glassClearFor, the parsers) live in
// effect/appearanceModel and are owned by the Appearance Effect service; the DOM apply
// (this setter, the theme/glass/sizing effects) stays here.
const setGlassClear = (clear: number) => {
  document.documentElement.style.setProperty("--glass-clear", clear.toFixed(3));
};

// Collapse a preference to the concrete theme in effect, resolving "system" from
// the OS appearance. Used to pick per-theme logo variants.
function resolveThemeMode(pref: ThemePreference): LogoTheme {
  if (pref !== "system") {
    return pref;
  }
  try {
    return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  } catch {
    return "dark";
  }
}

// Wider than tall reads as a horizontal bar; otherwise the default vertical strip.
// Base CSS is vertical, so an unset/indeterminate result harmlessly stays vertical.
function orientationForViewport(): Orientation {
  return window.innerWidth > window.innerHeight ? "horizontal" : "vertical";
}

export function useWindowChrome({
  themePref,
  orientationPref,
  glassEnabled,
  surfaceAlpha,
  framelessEnabled,
  appendDebugEntry,
  configureSummonHotkey,
}: {
  themePref: ThemePreference;
  orientationPref: OrientationPreference;
  glassEnabled: boolean;
  surfaceAlpha: number;
  framelessEnabled: boolean;
  // Registry-stable atom setters, passed as plain functions (this module stays
  // atom-free). configureSummonHotkey is typed structurally — atoms.ts exports
  // no name for the setter's input shape.
  appendDebugEntry: (entry: DebugEntryInput) => void;
  configureSummonHotkey: (input: { onPress: (() => void) | null }) => void;
}): {
  effectiveOrientation: Orientation;
  framelessApplied: boolean;
  resolvedTheme: LogoTheme;
  dragRegion: "" | undefined;
} {
  // Whether the native frame has ACTUALLY been removed (the set_window_decorations effect
  // resolved successfully), as opposed to the desired `framelessEnabled` preference. All
  // custom window chrome (drag regions + minimize/close) is gated on this, never the bare
  // preference, so it can't show as duplicate controls over a still-decorated window while a
  // toggle is mid-flight or after the native call rejected.
  const [framelessApplied, setFramelessApplied] = useState(false);
  // Layout axis, seeded from the current window shape and kept in sync on resize.
  const [orientation, setOrientation] = useState<Orientation>(orientationForViewport);
  // Layout preference: "auto" follows the live `orientation`; a pinned value overrides it.
  const effectiveOrientation: Orientation =
    orientationPref === "auto" ? orientation : orientationPref;
  // The summon hotkey is registered once but must place by the LIVE orientation, so a
  // pinned/auto horizontal bar is re-summoned as a bar, not snapped to the vertical
  // strip. A render-synced ref keeps the registered handler current.
  const summonPlacementRef = useRef<() => Promise<void>>(placePickerWindow);
  summonPlacementRef.current =
    effectiveOrientation === "horizontal" ? placeBarWindow : placePickerWindow;
  // Set once the orientation-sizing effect has scheduled the initial dock placement, so
  // it places on first mount (and every pinned reshape) but never re-snaps an "auto"
  // window on a later drag.
  const didInitialPlaceRef = useRef(false);
  // Concrete theme in effect, kept in sync by the theme effect; drives per-theme
  // logo variant selection. Seeded from the themePref prop so first paint picks the
  // right logos — equivalent to the old seed from the Appearance service's initial
  // theme, because the initializer runs only on the first render, where the
  // appearance atom hasn't resolved yet and themePref IS the initial-storage value.
  const [resolvedTheme, setResolvedTheme] = useState<LogoTheme>(() =>
    resolveThemeMode(themePref),
  );
  // The glass toggle's async resolution sets `--glass-clear` from the latest
  // alpha; reading it through a render-synced ref keeps the toggle effect off
  // surfaceAlpha's dep list (so a slider tick can't re-fire the native call).
  const surfaceAlphaRef = useRef(surfaceAlpha);
  surfaceAlphaRef.current = surfaceAlpha;

  useEffect(() => {
    // The global summon hotkey belongs to the dock alone (the settings window
    // never configures it — a second registration would double-bind the
    // shortcut). Registration, the in-use retry loop, and the failure banner
    // state live in the SummonHotkey service — this effect only points it at
    // the summon action. The callback reads summonPlacementRef at press time,
    // so the one registration always places by the LIVE orientation.
    configureSummonHotkey({
      onPress: () => {
        void raisePickerWindow(summonPlacementRef.current);
      },
    });
    return () => configureSummonHotkey({ onPress: null });
  }, [configureSummonHotkey]);

  // Apply the theme to <html data-theme>. "system" resolves from prefers-color-scheme
  // and re-resolves live when the OS appearance changes. Persistence + the cross-window
  // broadcast are owned by the Appearance service (driven by the setter); this effect
  // only applies the resolved theme to the DOM and the logo variant.
  useEffect(() => {
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      const resolved =
        themePref === "system" ? (media.matches ? "dark" : "light") : themePref;
      document.documentElement.setAttribute("data-theme", resolved);
      setResolvedTheme(resolved);
    };
    apply();

    if (themePref !== "system") {
      return;
    }
    media.addEventListener("change", apply);
    return () => media.removeEventListener("change", apply);
  }, [themePref]);

  // Track the window's aspect ratio; the sidebar renders data-orientation from this
  // state and the horizontal axis overrides in styles.css key off it. Re-deriving on
  // every resize is cheap, and setOrientation no-ops when the axis is unchanged, so a
  // drag that stays vertical never re-renders.
  useEffect(() => {
    const apply = () => setOrientation(orientationForViewport());
    apply();
    window.addEventListener("resize", apply);
    return () => window.removeEventListener("resize", apply);
  }, []);

  // The layout preference is persisted by the Appearance service (driven by the setter);
  // window shaping for the current orientation is handled by the effect below.
  // Shape and constrain the dock window for the current orientation preference in one
  // race-free sequence ("auto" = free: no cap, no snap). Caps are lifted before min is
  // raised so a larger min can never transiently exceed a stale max; then the real cap
  // is applied and we snap to the canonical strip/bar. A pinned change reshapes; "auto"
  // just follows the user's drag. The settings window is separate, so opening it no
  // longer reshapes anything here.
  useEffect(() => {
    // The plan (pinned-bar min==max lock, the vertical strip cap, the
    // place-on-first-mount-or-pinned-reshape rule) is sizePlanFor
    // (windowChromeModel); the queued native sequence is applyWindowShape
    // (windowOperations) — both tested there. The placement thunk reads
    // summonPlacementRef at op-run time so it follows the live orientation.
    const plan = sizePlanFor(orientationPref, effectiveOrientation, didInitialPlaceRef.current);
    // Set synchronously, never inside the queued op: StrictMode runs this
    // effect twice on mount, and only the first run may see shouldPlace=true
    // in auto mode — an op-time ref-set would double-place.
    didInitialPlaceRef.current = true;
    void enqueueWindowOperation(() =>
      applyWindowShape({ plan, place: () => summonPlacementRef.current() }),
    );
  }, [orientationPref, effectiveOrientation]);

  // Toggle the macOS glass backdrop. Order matters so we never flash the bare
  // desktop through the transparent window: when enabling, raise the blur layer
  // first, then mark the surface translucent; when disabling, go opaque first,
  // then drop the blur. macOS-only — the toggle isn't offered anywhere else.
  // Persistence + the cross-window mirror are owned by the Appearance service; this
  // effect only applies the native vibrancy, which lives on the dock (the settings
  // window is a solid, normally-chromed window and never frosts itself).
  useEffect(() => {
    if (!IS_MAC) {
      return;
    }

    let cancelled = false;
    // Round the vibrancy backdrop to match the frameless CSS corners; null lets a framed
    // window's native rounding apply. Keyed on the APPLIED frameless state (like the CSS
    // rounding via data-frameless), not the bare preference, so the frost only rounds once
    // the frame is actually gone. Re-applied whenever that state changes (dep below).
    const radius = framelessApplied ? FRAMELESS_CORNER_RADIUS : null;
    // The ordering/cancellation/failure sequence is applyGlass (windowOperations,
    // tested there); this effect provides the sinks. currentClear is a thunk so a
    // slider move while the op is queued still lands the fresh value. The debug
    // appender is registry-stable and deliberately NOT a dep — adding it would
    // re-fire native calls on log-identity churn.
    enqueueGlassOperation(() =>
      applyGlass({
        enabled: glassEnabled,
        radius,
        isCancelled: () => cancelled,
        currentClear: () => glassClearFor(surfaceAlphaRef.current),
        setAttr: (value) => document.documentElement.setAttribute("data-glass", value),
        setClear: setGlassClear,
        onError: (error) =>
          appendDebugEntry({
            kind: "command",
            label: "Glass effect",
            detail: errorMessage(error),
          }),
      }),
    );

    return () => {
      cancelled = true;
    };
  }, [glassEnabled, framelessApplied]);

  // Drive the tint opacity via a CSS variable; the data-glass rules in styles.css
  // only consume it while glass is on, so this is harmless when glass is off.
  // `--glass-clear` (a 0..1 see-through scalar the adaptive tokens interpolate
  // against) is owned by the glass-toggle effect so it stays in lockstep with the
  // actual data-glass state, not the pending React intent. Here we only refresh it
  // for slider moves while glass is already live; on/off transitions are that
  // effect's job. Persistence is owned by the Appearance service (driven by the setter).
  useEffect(() => {
    const root = document.documentElement;
    root.style.setProperty("--surface-alpha", String(surfaceAlpha));
    if (root.getAttribute("data-glass") === "on") {
      setGlassClear(glassClearFor(surfaceAlpha));
    }
  }, [surfaceAlpha]);

  // Apply the frameless-chrome preference to the dock window. Like glass, this is a
  // dock-only native apply (the settings window keeps its normal frame) owned by React,
  // while persistence + the cross-window mirror live in the Appearance service. The
  // data-frameless attribute is what surfaces the custom drag region + window controls in
  // styles.css, so it's flipped only once set_window_decorations resolves — the controls
  // never render over a still-framed window, and a failed native call leaves the attribute
  // "off" so we don't strip the only chrome without a working replacement. Serialized
  // through a queue so a fast toggle settles on the latest desired state.
  useEffect(() => {
    let cancelled = false;
    // The sequence (flip the attribute/applied state only after the native call
    // lands; failure forces both off) is applyFrameless (windowOperations,
    // tested there); this effect provides the sinks.
    enqueueFramelessOperation(() =>
      applyFrameless({
        enabled: framelessEnabled,
        isCancelled: () => cancelled,
        setAttr: (value) => document.documentElement.setAttribute("data-frameless", value),
        setApplied: setFramelessApplied,
        onError: (error) =>
          appendDebugEntry({
            kind: "command",
            label: "Frameless window",
            detail: errorMessage(error),
          }),
      }),
    );

    return () => {
      cancelled = true;
    };
  }, [framelessEnabled]);

  // Custom window-chrome drag handle for frameless mode, shared by the boot/recovery
  // screen and the picker below (the matching minimize/close controls are the
  // WindowControls component). Gated on framelessApplied like the controls, so chrome
  // only appears once the native frame is actually gone. data-tauri-drag-region=""
  // adds the drag handle; undefined omits it (the chrome bands only become draggable
  // when frameless).
  const dragRegion = framelessApplied ? "" : undefined;

  return { effectiveOrientation, framelessApplied, resolvedTheme, dragRegion };
}
