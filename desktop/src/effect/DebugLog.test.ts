import { Effect, SubscriptionRef } from "effect";
import { describe, expect, it } from "vitest";
import { DEBUG_LOG_LIMIT, DebugLog } from "./DebugLog";

describe("DebugLog", () => {
  it("prepends stamped entries newest-first", () =>
    Effect.gen(function* () {
      const log = yield* DebugLog;
      yield* log.append({ kind: "command", label: "first", detail: "a" });
      yield* log.append({ kind: "settings", label: "second", detail: "b" });
      const entries = yield* SubscriptionRef.get(log.state);
      expect(entries.map((entry) => entry.label)).toEqual(["second", "first"]);
      expect(entries[0].kind).toBe("settings");
      expect(entries[0].detail).toBe("b");
      expect(typeof entries[0].id).toBe("number");
      expect(entries[0].time).not.toBe("");
      // Distinct in practice (millis + random). Deliberately weak: the formula
      // is kept for parity with the old App.tsx state and guarantees nothing.
      expect(entries[0].id).not.toBe(entries[1].id);
    }).pipe(Effect.provide(DebugLog.Default), Effect.runPromise));

  it("caps the log at DEBUG_LOG_LIMIT, dropping the oldest", () =>
    Effect.gen(function* () {
      const log = yield* DebugLog;
      for (let index = 0; index < DEBUG_LOG_LIMIT + 5; index++) {
        yield* log.append({ kind: "command", label: `entry ${index}`, detail: "" });
      }
      const entries = yield* SubscriptionRef.get(log.state);
      expect(entries).toHaveLength(DEBUG_LOG_LIMIT);
      expect(entries[0].label).toBe(`entry ${DEBUG_LOG_LIMIT + 4}`);
      expect(entries[entries.length - 1].label).toBe("entry 5");
    }).pipe(Effect.provide(DebugLog.Default), Effect.runPromise));

  it("clear empties the log", () =>
    Effect.gen(function* () {
      const log = yield* DebugLog;
      yield* log.append({ kind: "stream", label: "x", detail: "y" });
      yield* log.clear;
      expect(yield* SubscriptionRef.get(log.state)).toEqual([]);
    }).pipe(Effect.provide(DebugLog.Default), Effect.runPromise));
});
