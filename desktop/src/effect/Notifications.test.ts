import { Duration, Effect, Layer, Option, Queue, Stream, SubscriptionRef } from "effect";
import { describe, expect, it } from "vitest";
import {
  Notifications,
  layerWithoutDependencies as notificationsLayer,
} from "./Notifications";
import { PrefsBridge } from "./PrefsBridge";
import { NOTIFY_ON_IDLE_STORAGE_KEY } from "./notificationsModel";
import type { PrefsSync } from "./prefs";

const awaitEnabled = (stream: Stream.Stream<{ readonly notifyOnIdle: boolean }>) =>
  stream.pipe(
    Stream.filter((state) => state.notifyOnIdle),
    Stream.runHead,
    Effect.flatMap(
      Option.match({
        onNone: () => Effect.die("state stream ended early"),
        onSome: Effect.succeed,
      }),
    ),
  );

describe("Notifications", () => {
  it("sets locally and adopts inbound changes with persistence but no re-emit", () =>
    Effect.gen(function* () {
      const store = new Map<string, string>();
      const emitted = yield* Queue.unbounded<PrefsSync>();
      const inbound = yield* Queue.unbounded<PrefsSync>();
      const bridge = Layer.succeed(PrefsBridge, {
        mode: "settings" as const,
        loadRaw: (key: string) => store.get(key) ?? null,
        storeRaw: (key: string, value: string) => store.set(key, value),
        emit: (payload: PrefsSync) => Queue.offer(emitted, payload).pipe(Effect.asVoid),
        events: Stream.fromQueue(inbound),
      });
      const layer = notificationsLayer.pipe(Layer.provide(bridge));

      yield* Effect.gen(function* () {
        const notifications = yield* Notifications;
        expect((yield* SubscriptionRef.get(notifications.state)).notifyOnIdle).toBe(false);

        yield* notifications.setNotifyOnIdle(true);
        expect(store.get(NOTIFY_ON_IDLE_STORAGE_KEY)).toBe("true");
        expect(yield* Queue.take(emitted)).toEqual({ kind: "notifyOnIdle", enabled: true });

        yield* notifications.setNotifyOnIdle(false);
        yield* Queue.take(emitted);
        yield* Queue.offer(inbound, { kind: "notifyOnIdle", enabled: true });
        expect(
          (yield* awaitEnabled(SubscriptionRef.changes(notifications.state))).notifyOnIdle,
        ).toBe(true);
        expect(store.get(NOTIFY_ON_IDLE_STORAGE_KEY)).toBe("true");
        expect(yield* Queue.size(emitted)).toBe(0);
      }).pipe(Effect.provide(layer, { local: true }));
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));
});
