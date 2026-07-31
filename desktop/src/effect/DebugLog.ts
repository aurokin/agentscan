// The debug log the settings window renders (and the dock writes), extracted
// from App.tsx's per-window React state. Each webview's runtime builds its own
// instance, preserving the old per-window useState isolation: dock lifecycle
// services append command entries to a log nothing renders there today
// (pre-existing behavior, kept), while the settings window's UI adapters append
// and render their own. Other UI/native logging still uses the atom adapter.

import { Context, Effect, Layer, SubscriptionRef } from "effect";

// Newest-first, capped: appends beyond the limit drop the oldest entries.
export const DEBUG_LOG_LIMIT = 80;

export type DebugEntry = {
  readonly id: number;
  readonly time: string;
  readonly kind: "command" | "stream" | "settings";
  readonly label: string;
  readonly detail: string;
};

// What call sites supply; the service stamps id + time at append.
export type DebugEntryInput = Omit<DebugEntry, "id" | "time">;

export class DebugLog extends Context.Service<DebugLog>()("desktop/DebugLog", {
  make: Effect.gen(function* () {
    const stateRef = yield* SubscriptionRef.make<ReadonlyArray<DebugEntry>>([]);
    return {
      state: stateRef,
      // Stamp at append time (wall-clock-millis-plus-random id, locale time
      // string — the exact formula of the old App.tsx setState). The id
      // doubles as the React list key; the formula has no hard uniqueness
      // guarantee, matching the old behavior rather than strengthening it.
      append: (entry: DebugEntryInput) =>
        Effect.suspend(() => {
          const stamped: DebugEntry = {
            ...entry,
            id: Date.now() + Math.random(),
            time: new Date().toLocaleTimeString(),
          };
          return SubscriptionRef.update(stateRef, (current) =>
            [stamped, ...current].slice(0, DEBUG_LOG_LIMIT),
          );
        }),
      clear: SubscriptionRef.set(stateRef, []),
    };
  }),
}) {}

export const layer = Layer.effect(DebugLog, DebugLog.make);
