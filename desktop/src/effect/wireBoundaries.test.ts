import { Effect, Fiber, Option, Queue, Stream } from "effect";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => {
  const listeners = new Map<string, (event: { payload: unknown }) => void>();
  return {
    invoke: vi.fn<(op: string, args?: Record<string, unknown>) => Promise<unknown>>(),
    listen: vi.fn(
      async (event: string, callback: (event: { payload: unknown }) => void) => {
        listeners.set(event, callback);
        return () => listeners.delete(event);
      },
    ),
    listeners,
  };
});

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: mocks.listen,
  emitTo: vi.fn(() => Promise.resolve()),
}));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: "main" }),
}));

import { HostIpc } from "./HostIpc";
import { PreflightIpc } from "./Preflight";
import { PrefsBridge } from "./PrefsBridge";
import { IpcError, TauriIpc } from "./TauriIpc";

const SETTINGS = { kind: "local" as const, binaryPath: "", env: [] };

const malformedError = <A, E>(effect: Effect.Effect<A, E>) =>
  Effect.runPromise(effect.pipe(Effect.flip));

describe("decoded command boundaries", () => {
  beforeEach(() => {
    mocks.invoke.mockReset();
    mocks.listen.mockClear();
    mocks.listeners.clear();
  });

  it("maps malformed local-host responses to IpcError", async () => {
    mocks.invoke.mockResolvedValue({ label: "koopa" });

    const error = await malformedError(
      HostIpc.make.pipe(Effect.flatMap((ipc) => ipc.localHostLabel)),
    );

    expect(error).toBeInstanceOf(IpcError);
    expect(error).toMatchObject({ op: "local_host_label" });
  });

  it("maps malformed preflight responses to IpcError", async () => {
    mocks.invoke.mockResolvedValue({ binary: "agentscan", ok: "yes" });

    const error = await malformedError(
      PreflightIpc.make.pipe(Effect.flatMap((ipc) => ipc.probe(SETTINGS))),
    );

    expect(error).toBeInstanceOf(IpcError);
    expect(error).toMatchObject({ op: "preflight_agentscan" });
  });

  it.each([
    ["loadPickerRows", "load_picker_rows", { rows: [] }],
    ["pollDaemonStatus", "poll_daemon_status", { reachable: "yes" }],
  ] as const)("maps malformed %s responses to IpcError", async (method, op, response) => {
    mocks.invoke.mockResolvedValue(response);

    const error = await malformedError(
      TauriIpc.make.pipe(
        Effect.flatMap((ipc) =>
          method === "loadPickerRows"
            ? ipc.loadPickerRows(SETTINGS)
            : ipc.pollDaemonStatus(SETTINGS),
        ),
      ),
    );

    expect(error).toBeInstanceOf(IpcError);
    expect(error).toMatchObject({ op });
  });
});

describe("best-effort decoded listeners", () => {
  beforeEach(() => {
    mocks.invoke.mockReset();
    mocks.listen.mockClear();
    mocks.listeners.clear();
  });

  it("drops malformed and unknown live events while keeping the source alive", () =>
    Effect.runPromise(
      Effect.scoped(
        Effect.gen(function* () {
          const ipc = yield* TauriIpc.make;
          const queue = yield* ipc.liveEvents("local");
          const listener = mocks.listeners.get("agentscan-live-picker");
          expect(listener).toBeDefined();

          listener?.({ payload: { sourceKey: "local", epoch: 1, kind: "future-event" } });
          listener?.({
            payload: {
              sourceKey: "local",
              epoch: 1,
              kind: "connecting",
              message: "Connecting",
            },
          });

          const event = yield* Queue.take(queue);
          expect(event).toEqual({
            sourceKey: "local",
            epoch: 1,
            kind: "connecting",
            message: "Connecting",
          });
        }),
      ),
    ));

  it("drops malformed prefs events and publishes the next valid event", () =>
    Effect.runPromise(
      Effect.scoped(
        Effect.gen(function* () {
          const bridge = yield* PrefsBridge.make;
          const next = yield* Stream.runHead(bridge.events).pipe(
            Effect.forkChild({ startImmediately: true }),
          );
          const listener = mocks.listeners.get("agentscan:prefs-sync");
          expect(listener).toBeDefined();

          listener?.({ payload: { kind: "theme", theme: "sepia" } });
          listener?.({ payload: { kind: "theme", theme: "dark" } });

          const event = yield* Fiber.join(next);
          expect(Option.getOrUndefined(event)).toEqual({ kind: "theme", theme: "dark" });
        }),
      ),
    ));
});
