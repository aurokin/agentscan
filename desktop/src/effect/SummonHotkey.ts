// The dock's global summon hotkey, owned end-to-end: the OS registration, the
// in-use retry loop (the holder — usually a second agentscan instance — can quit
// at any time without any release signal, so polling is the only way to reclaim
// the key without a restart), and the standing failure state the dock renders as
// its banner. App.tsx just points the service at the summon action via configure
// and observes `state`; the press callback is captured at configure time so the
// registration survives React re-renders untouched.

import { Duration, Effect, Fiber, SubscriptionRef } from "effect";
import { register, unregister } from "@tauri-apps/plugin-global-shortcut";
import { IpcError } from "./TauriIpc";

const messageOf = (error: unknown) => (error instanceof Error ? error.message : String(error));

// The summon combo. SUMMON_HOTKEY_LABEL below is its Mac-first display form.
const SUMMON_HOTKEY = "CommandOrControl+Shift+A";

// The OS grants a global hotkey to one process. macOS reports a combo held by
// another process as a RegisterEventHotKey failure; the plugin reports a combo
// it already holds in this process as "already registered". Both mean "someone
// owns the key", and in practice that someone is usually a second agentscan
// instance (e.g. a dev build alongside the installed app).
const HOTKEY_IN_USE_PATTERN = /RegisterEventHotKey failed|already registered/i;

const SUMMON_HOTKEY_LABEL = "⌘⇧A";

// In-use failures are the only ones recoverable by waiting (the holder can
// quit at any time), so they alone justify a registration retry loop.
export function summonHotkeyInUse(error: unknown): boolean {
  return HOTKEY_IN_USE_PATTERN.test(failureDetail(error));
}

export function summonHotkeyFailureMessage(error: unknown): string {
  if (summonHotkeyInUse(error)) {
    return `${SUMMON_HOTKEY_LABEL} is in use — another agentscan instance may be running. Retrying until it frees up.`;
  }
  return `Unable to register ${SUMMON_HOTKEY_LABEL}: ${failureDetail(error)}`;
}

function failureDetail(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

// The global-shortcut plugin boundary, injected as a service so the SummonHotkey
// logic stays pure over it and a vitest layer can script in-use/terminal failures
// without a real Tauri host. The press handler is a plain callback because the
// plugin delivers events outside any Effect runtime.
export class HotkeyIpc extends Effect.Service<HotkeyIpc>()("desktop/HotkeyIpc", {
  succeed: {
    register: (shortcut: string, onEvent: (state: "Pressed" | "Released") => void) =>
      Effect.tryPromise({
        try: () => register(shortcut, (event) => onEvent(event.state)),
        catch: (error) => new IpcError({ op: "register_shortcut", message: messageOf(error) }),
      }),
    unregister: (shortcut: string) =>
      Effect.tryPromise({
        try: () => unregister(shortcut),
        catch: (error) => new IpcError({ op: "unregister_shortcut", message: messageOf(error) }),
      }),
  },
}) {}

// Re-registration cadence while the key is held by someone else, injected as a
// service so tests can zero it out (keeping them event-driven, no wall-clock).
export class SummonHotkeyConfig extends Effect.Service<SummonHotkeyConfig>()(
  "desktop/SummonHotkeyConfig",
  {
    succeed: {
      retryBackoff: Duration.seconds(5),
    },
  },
) {}

// "failed" is a STANDING condition (unlike a failed activation's one-shot
// feedback): it describes the registration right now and clears only when a
// retry lands or the dock deconfigures.
export type SummonHotkeyState =
  | { readonly status: "inactive" }
  | { readonly status: "registered" }
  | { readonly status: "failed"; readonly message: string };

const INACTIVE: SummonHotkeyState = { status: "inactive" };

// Owns the summon-hotkey registration lifecycle, replacing the old App.tsx
// effect + module-level promise queue. One supervised fiber holds the
// registration; configure interrupts it (awaiting the paired unregister, so a
// re-register can't race our own still-held key) before arming the next one.
export class SummonHotkey extends Effect.Service<SummonHotkey>()("desktop/SummonHotkey", {
  dependencies: [HotkeyIpc.Default, SummonHotkeyConfig.Default],
  scoped: Effect.gen(function* () {
    const ipc = yield* HotkeyIpc;
    const { retryBackoff } = yield* SummonHotkeyConfig;
    const stateRef = yield* SubscriptionRef.make<SummonHotkeyState>(INACTIVE);
    // The service scope: the registration fiber forked by configure is owned by
    // it, so the hotkey is released when the runtime dies.
    const scope = yield* Effect.scope;
    // Serializes configures so two can't interleave their interrupt/fork pairs.
    const mutex = yield* Effect.makeSemaphore(1);
    // The running registration fiber. Mutated only under `mutex`.
    let fiber: Fiber.RuntimeFiber<never> | null = null;

    // Hold the registration until interrupted, retrying only in-use failures.
    // acquireUseRelease pairs register with unregister so the key is ALWAYS
    // released — and acquire is uninterruptible: if configure interrupts us while
    // the register invoke is in flight, the registration completes and THEN
    // release unregisters it, instead of leaking a held key with no owner.
    const runHotkey = (onPress: () => void): Effect.Effect<never> =>
      Effect.gen(function* () {
        while (true) {
          const failure = yield* Effect.acquireUseRelease(
            ipc.register(SUMMON_HOTKEY, (state) => {
              if (state === "Pressed") {
                onPress();
              }
            }),
            // Registered: hold the key (and clear any standing failure) until the
            // next configure interrupts us.
            () =>
              SubscriptionRef.set(stateRef, { status: "registered" }).pipe(
                Effect.zipRight(Effect.never),
              ),
            () => ipc.unregister(SUMMON_HOTKEY).pipe(Effect.ignore),
          ).pipe(Effect.catchAll((error) => Effect.succeed(error)));

          // Only a register failure reaches here (the use branch never returns).
          yield* SubscriptionRef.set(stateRef, {
            status: "failed",
            message: summonHotkeyFailureMessage(failure),
          });
          if (!summonHotkeyInUse(failure)) {
            // Terminal (e.g. a permission or backend error): polling would never
            // reclaim it, so surface once and park until a re-configure.
            return yield* Effect.never;
          }
          yield* Effect.sleep(retryBackoff);
        }
      });

    return {
      state: stateRef,
      // Arm the hotkey with a press action, or release it with null (the
      // non-dock windows never configure, so they never bind the shortcut).
      configure: (onPress: (() => void) | null) =>
        mutex.withPermits(1)(
          Effect.gen(function* () {
            if (fiber !== null) {
              // Await the interrupt so the dying fiber's unregister completes
              // before a replacement registers the same combo.
              yield* Fiber.interrupt(fiber);
              fiber = null;
            }
            if (onPress === null) {
              yield* SubscriptionRef.set(stateRef, INACTIVE);
              return;
            }
            fiber = yield* runHotkey(onPress).pipe(Effect.forkIn(scope));
          }),
        ),
    };
  }),
}) {}
