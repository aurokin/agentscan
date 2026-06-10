// The debug log the settings window renders (and the dock writes), extracted
// from App.tsx's per-window React state. Each webview's runtime builds its own
// instance, preserving the old per-window useState isolation: the dock appends
// its command-lifecycle entries to a log nothing renders there today
// (pre-existing behavior, kept), while the settings window appends and renders
// its own. The win over useState is a registry-stable append setter: the old
// appendDebugEntry closure was recreated every render, forcing dep-list
// omissions in every effect that logged.

import { Effect, SubscriptionRef } from "effect";

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

export class DebugLog extends Effect.Service<DebugLog>()("desktop/DebugLog", {
  effect: Effect.gen(function* () {
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
