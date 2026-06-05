import { Effect } from "effect";
import { emitTo } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { PREFS_SYNC_EVENT, type PrefsSync, type ShellMode } from "./prefs";
import type { StorageRead, StorageWrite } from "./profileModel";

// One Vite entry serves both windows; the window label decides which UI/runtime
// this is. Mirrors the resolution in main.tsx. Guarded so service construction
// still succeeds under a non-Tauri host (it is never reached in tests, which mock
// the whole bridge, but the catch keeps a stray construction from throwing).
function resolveMode(): ShellMode {
  try {
    return getCurrentWebviewWindow().label === "settings" ? "settings" : "dock";
  } catch {
    return "dock";
  }
}

// The cross-window boundary every prefs-owning service shares: synchronous
// localStorage access and a best-effort `emit` to the other window. This is the
// single injectable seam — tests swap the whole service with Layer.succeed, exactly
// as the LiveConnection tests swap TauriIpc, so the domain services stay pure logic
// over this interface with no real Tauri/DOM dependency.
//
// Inbound adoption is intentionally NOT modeled here: the only inbound prefs sync
// migrated so far (`{kind:"profiles"}`) is gated on the settings window's unsaved-
// edit flag, which is React-synchronous state. React reads it directly and calls
// the service's reload, so there is no need (yet) to fan the channel into a service
// fiber. The Preflight/Appearance slices will add an `events` stream here when they
// own inbound kinds that don't depend on React-synchronous gating.
export class PrefsBridge extends Effect.Service<PrefsBridge>()("desktop/PrefsBridge", {
  succeed: {
    // Reads are best-effort: a missing/blocked read falls back to defaults, matching
    // the old loadStored* helpers.
    loadRaw: ((key) => {
      try {
        return window.localStorage.getItem(key);
      } catch {
        return null;
      }
    }) as StorageRead,

    // Writes deliberately do NOT swallow errors (unlike loadRaw, and matching the old
    // storeProfiles, which let setItem throw). storeProfileState runs first in
    // Profiles.commit, so a quota/availability failure aborts the commit BEFORE it
    // broadcasts or updates the in-memory ref — otherwise this window would show the
    // change and tell the other window to reload while storage still holds the old
    // snapshot, silently desyncing them and losing the edit on the next reload.
    storeRaw: ((key, value) => {
      window.localStorage.setItem(key, value);
    }) as StorageWrite,

    // Best-effort, fire-and-forget mirror to the other window (matches the old
    // broadcastPrefs); the other window also reconciles on its next focus/reload.
    emit: (payload: PrefsSync) =>
      Effect.sync(() => {
        const otherWindowLabel = resolveMode() === "settings" ? "main" : "settings";
        void emitTo(otherWindowLabel, PREFS_SYNC_EVENT, payload).catch(() => {});
      }),
  },
}) {}
