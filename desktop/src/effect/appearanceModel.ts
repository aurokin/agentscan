// Pure appearance model for the desktop shell (theme, dock-layout orientation, the macOS
// glass toggle + tint, and the frameless-chrome toggle). No React, no Effect, no global
// `window` access — persistence is
// parameterized over a read/write pair so the Appearance Effect.Service (and its vitest
// proof) can drive it over an injected storage boundary while App.tsx keeps using the
// same parsers/serializers to seed its first paint.

import type { StorageRead, StorageWrite } from "./profileModel";
import type { OrientationPreference, ThemePreference } from "./prefs";

// Storage keys. THEME_STORAGE_KEY is also read by the pre-paint theme script in
// index.html (so the first frame isn't a flash of the wrong theme); keep them in sync.
export const THEME_STORAGE_KEY = "agentscan.desktop.theme";
export const ORIENTATION_STORAGE_KEY = "agentscan.desktop.orientation";
export const GLASS_STORAGE_KEY = "agentscan.desktop.glass";
export const SURFACE_ALPHA_STORAGE_KEY = "agentscan.desktop.surfaceAlpha";
// Frameless dock chrome: drop the native titlebar for a borderless ribbon with custom
// drag/min/close controls. Off by default (a framed window on first run).
export const FRAMELESS_STORAGE_KEY = "agentscan.desktop.frameless";

// Tint alpha floor of 0.20 caps transparency at 80% (the slider reads 1 - alpha): the
// surface always keeps a little tint over the native vibrancy frost, so the UI never
// washes out fully even at the most transparent setting.
export const SURFACE_ALPHA_MIN = 0.2;
export const SURFACE_ALPHA_MAX = 1;
// First-run default: 0.50 alpha == 50% transparency — a balanced frosted look.
export const SURFACE_ALPHA_DEFAULT = 0.5;

// "How see-through is the surface" as a 0..1 scalar (0 frosted/solid, 1 fully clear) that
// adaptive tokens interpolate against. Mirrors the slider math.
export const glassClearFor = (alpha: number) =>
  (SURFACE_ALPHA_MAX - alpha) / (SURFACE_ALPHA_MAX - SURFACE_ALPHA_MIN);

export type AppearanceState = {
  readonly themePref: ThemePreference;
  readonly orientationPref: OrientationPreference;
  readonly glassEnabled: boolean;
  readonly surfaceAlpha: number;
  readonly framelessEnabled: boolean;
};

export function parseThemePref(raw: string | null): ThemePreference {
  // Anything unrecognized follows the OS appearance.
  return raw === "dark" || raw === "light" || raw === "system" ? raw : "system";
}

export function parseOrientationPref(raw: string | null): OrientationPreference {
  return raw === "auto" || raw === "vertical" || raw === "horizontal" ? raw : "auto";
}

export function parseGlassEnabled(raw: string | null): boolean {
  // Default glass ON for a first run (no stored choice); once toggled, "on"/"off" is
  // respected. Deliberately platform-agnostic: native vibrancy is macOS-only, but the
  // suppression for other platforms lives at the apply/UI layer (the glass effect and
  // settings controls are gated on IS_MAC), so on non-macOS this value is a dormant,
  // never-applied preference rather than something the model needs to know about.
  return raw === null ? true : raw === "on";
}

export function parseFrameless(raw: string | null): boolean {
  // Default OFF (framed window with the native titlebar) on a first run; once the user
  // toggles it, "on"/"off" is respected. The decorations toggle is cross-platform, so —
  // unlike glass — this value is live on every platform, not a dormant macOS-only pref.
  return raw === "on";
}

export function parseSurfaceAlpha(raw: string | null): number {
  // Guard the missing/empty case explicitly: Number(null) and Number("") are both 0
  // (finite), which would otherwise clamp first-time users to the most transparent
  // setting instead of the frosted default.
  if (raw !== null && raw.trim() !== "") {
    const parsed = Number(raw);
    if (Number.isFinite(parsed)) {
      return Math.min(SURFACE_ALPHA_MAX, Math.max(SURFACE_ALPHA_MIN, parsed));
    }
  }
  return SURFACE_ALPHA_DEFAULT;
}

// Seed the full appearance state from storage. The read is best-effort (the injected
// reader returns null on any failure), so every field falls back to its default.
export function loadAppearance(read: StorageRead): AppearanceState {
  return {
    themePref: parseThemePref(read(THEME_STORAGE_KEY)),
    orientationPref: parseOrientationPref(read(ORIENTATION_STORAGE_KEY)),
    glassEnabled: parseGlassEnabled(read(GLASS_STORAGE_KEY)),
    surfaceAlpha: parseSurfaceAlpha(read(SURFACE_ALPHA_STORAGE_KEY)),
    framelessEnabled: parseFrameless(read(FRAMELESS_STORAGE_KEY)),
  };
}

// Per-field serializers — the single source of the on-disk encoding, matching the old
// App.tsx persistence lines exactly (raw string for theme/orientation, "on"/"off" for
// glass, two-decimal for alpha).
export const storeThemePref = (write: StorageWrite, value: ThemePreference) =>
  write(THEME_STORAGE_KEY, value);
export const storeOrientationPref = (write: StorageWrite, value: OrientationPreference) =>
  write(ORIENTATION_STORAGE_KEY, value);
export const storeGlassEnabled = (write: StorageWrite, value: boolean) =>
  write(GLASS_STORAGE_KEY, value ? "on" : "off");
export const storeSurfaceAlpha = (write: StorageWrite, value: number) =>
  write(SURFACE_ALPHA_STORAGE_KEY, value.toFixed(2));
export const storeFrameless = (write: StorageWrite, value: boolean) =>
  write(FRAMELESS_STORAGE_KEY, value ? "on" : "off");

export const appearanceEqual = (a: AppearanceState, b: AppearanceState): boolean =>
  a.themePref === b.themePref &&
  a.orientationPref === b.orientationPref &&
  a.glassEnabled === b.glassEnabled &&
  a.surfaceAlpha === b.surfaceAlpha &&
  a.framelessEnabled === b.framelessEnabled;
