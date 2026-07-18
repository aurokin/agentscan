import { Effect, Stream, SubscriptionRef } from "effect";
import { PrefsBridge } from "./PrefsBridge";
import {
  NOTIFY_ON_IDLE_STORAGE_KEY,
  parseNotifyOnIdle,
  storeNotifyOnIdle,
} from "./notificationsModel";

export type NotificationsState = { readonly notifyOnIdle: boolean };

// Owns the persisted notification preference and its cross-window mirror. Live status
// stays dock-owned; this service deliberately depends only on the shared prefs bridge.
export class Notifications extends Effect.Service<Notifications>()("desktop/Notifications", {
  dependencies: [PrefsBridge.Default],
  scoped: Effect.gen(function* () {
    const bridge = yield* PrefsBridge;
    const stateRef = yield* SubscriptionRef.make<NotificationsState>({
      notifyOnIdle: parseNotifyOnIdle(bridge.loadRaw(NOTIFY_ON_IDLE_STORAGE_KEY)),
    });
    const persist = (write: () => void) => Effect.try(write).pipe(Effect.ignore);

    const setNotifyOnIdle = (notifyOnIdle: boolean) =>
      Effect.gen(function* () {
        yield* persist(() => storeNotifyOnIdle(bridge.storeRaw, notifyOnIdle));
        yield* SubscriptionRef.set(stateRef, { notifyOnIdle });
        yield* bridge.emit({ kind: "notifyOnIdle", enabled: notifyOnIdle });
      });

    // Adopt remote changes with the same best-effort persistence heal as Appearance,
    // without re-emitting and creating a cross-window loop.
    yield* bridge.events.pipe(
      Stream.runForEach((payload) =>
        payload.kind === "notifyOnIdle"
          ? Effect.gen(function* () {
              yield* persist(() => storeNotifyOnIdle(bridge.storeRaw, payload.enabled));
              yield* SubscriptionRef.set(stateRef, { notifyOnIdle: payload.enabled });
            })
          : Effect.void,
      ),
      Effect.forkScoped,
    );

    return { state: stateRef, setNotifyOnIdle };
  }),
}) {}
