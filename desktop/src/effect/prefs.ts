// The cross-window prefs channel contract. Separate webview windows (the dock,
// label "main"; the settings window, label "settings") don't share React state or
// the localStorage "storage" event, so prefs the user changes in one window are
// mirrored to the other over this single Tauri event channel. Only user actions /
// the owning service emit; listeners apply remote changes without re-emitting, so
// dock -> settings -> dock can't loop.
//
// The payload union spans every migrated concern (profiles, preflight, and the
// appearance prefs). Each Effect service owns the kinds it cares about and ignores
// the rest; the PrefsBridge fans the raw stream out to all of them.

import type { AgentscanPreflight } from "./profileModel";

export const PREFS_SYNC_EVENT = "agentscan:prefs-sync";

// Which window this React tree / runtime drives.
export type ShellMode = "dock" | "settings";

// Appearance preference wire types (the values mirrored across windows). The
// Appearance service owns the apply logic; these string shapes are the contract.
export type ThemePreference = "dark" | "light" | "system";
export type Orientation = "vertical" | "horizontal";
export type OrientationPreference = "auto" | Orientation;

// The dock's resolved-preflight discriminant, mirrored to the settings card so it
// can reproduce the dock's tones without probing itself.
export type PreflightStatus = "loading" | "ready" | "failed";

export type PrefsSync =
  | { kind: "theme"; theme: ThemePreference }
  | { kind: "orientation"; orientation: OrientationPreference }
  | { kind: "glass"; enabled: boolean; alpha: number }
  // Frameless dock chrome toggle, mirrored so the settings window's switch drives the
  // dock's decorations (the dock owns the apply).
  | { kind: "frameless"; enabled: boolean }
  // Agent-finished/needs-you notification toggle, mirrored between dock and settings.
  | { kind: "notifyOnIdle"; enabled: boolean }
  // Signal-only: the receiver re-reads persisted profiles. Carries no data so it
  // can never clobber an in-progress edit with a stale snapshot.
  | { kind: "profiles" }
  // Dock -> settings: the dock's resolved preflight (with the runnerKey it
  // describes) so the settings card can reuse it instead of probing itself.
  // `status` mirrors the dock's discriminant so the card can reproduce its tones:
  // "ready" carries `preflight`; "loading" reads as Checking; "failed" (dock IPC
  // error) reads as Unreachable. `preflight` is non-null only on "ready".
  | {
      kind: "preflight";
      status: PreflightStatus;
      runnerKey: string;
      preflight: AgentscanPreflight | null;
    }
  // Settings -> dock: ask the dock to re-emit its current preflight. emitTo has no
  // replay, so a settings window shown after the dock probed would otherwise miss
  // the result; it requests one on show to reconcile.
  | { kind: "preflight-request" };
