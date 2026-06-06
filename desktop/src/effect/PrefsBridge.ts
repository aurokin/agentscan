import { Effect, PubSub, Runtime, Stream } from "effect";
import { emitTo, listen, type UnlistenFn } from "@tauri-apps/api/event";
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

// The cross-window boundary every prefs-owning service shares: which window this is
// (`mode`), synchronous localStorage access, a best-effort `emit` to the other window,
// and a single inbound `events` stream fanned out from one `agentscan:prefs-sync`
// listener. This is the injectable seam — tests swap the whole service with
// Layer.succeed, exactly as the LiveConnection tests swap TauriIpc, so the domain
// services stay pure logic over this interface with no real Tauri/DOM dependency.
//
// `events` carries every inbound sync kind to every subscriber (each service ignores
// the kinds it doesn't own). The Preflight service consumes it for the dock<->settings
// preflight protocol. App.tsx still runs its OWN listener for the kinds not yet
// migrated (theme/orientation/glass, and the React-synchronously-gated `profiles`
// adoption); the two listeners coexist because Tauri delivers each event to every
// registered listener. As those concerns migrate, App's listener retires and they move
// onto this stream.
export class PrefsBridge extends Effect.Service<PrefsBridge>()("desktop/PrefsBridge", {
  scoped: Effect.gen(function* () {
    // resolveMode wraps getCurrentWebviewWindow() in try/catch (see above), so this never
    // throws on a non-Tauri host (no __TAURI_INTERNALS__) — it falls back to "dock" and
    // construction proceeds to the equally-guarded listener below. Safe to call eagerly here.
    const mode = resolveMode();

    // One listener for the whole channel, published to a PubSub so multiple services
    // can each observe the full stream. orElseSucceed keeps construction resilient: a
    // failed `listen` (non-Tauri host) yields a no-op unlisten rather than failing the
    // layer and stranding every service that shares this bridge.
    const inbound = yield* PubSub.unbounded<PrefsSync>();
    const runFork = Runtime.runFork(yield* Effect.runtime<never>());
    yield* Effect.acquireRelease(
      Effect.tryPromise({
        try: () =>
          listen<PrefsSync>(PREFS_SYNC_EVENT, (event) => {
            runFork(PubSub.publish(inbound, event.payload));
          }),
        catch: (error) => error,
      }).pipe(Effect.orElseSucceed((): UnlistenFn => () => {})),
      (unlisten) => Effect.sync(() => unlisten()),
    );

    return {
      // Which window this runtime drives. The owning service uses it to gate
      // producer-vs-consumer behavior over the shared channel.
      mode,

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
          const otherWindowLabel = mode === "settings" ? "main" : "settings";
          void emitTo(otherWindowLabel, PREFS_SYNC_EVENT, payload).catch(() => {});
        }),

      // The inbound cross-window stream, fanned out to every subscriber.
      events: Stream.fromPubSub(inbound),
    };
  }),
}) {}
