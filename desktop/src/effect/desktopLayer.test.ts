import { Effect, Layer } from "effect";
import { expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => {
  const prefsUnlisten = vi.fn();
  return {
    invoke: vi.fn((_command: string): Promise<unknown> => Promise.resolve(undefined)),
    listen: vi.fn(
      async (event: string, _callback: (event: { payload: unknown }) => void) =>
        event === "agentscan:prefs-sync" ? prefsUnlisten : vi.fn(),
    ),
    emitTo: vi.fn(async () => {}),
    register: vi.fn(async () => {}),
    unregister: vi.fn(async () => {}),
    prefsUnlisten,
  };
});

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen, emitTo: mocks.emitTo }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: "main" }),
}));
vi.mock("@tauri-apps/plugin-global-shortcut", () => ({
  register: mocks.register,
  unregister: mocks.unregister,
}));

import { desktopLayer } from "./desktopLayer";
import { PREFS_SYNC_EVENT } from "./prefs";

it("constructs one inert desktop service graph and releases its shared prefs listener", async () => {
  await Effect.runPromise(
    Effect.scoped(
      Layer.build(desktopLayer).pipe(
        Effect.tap(() =>
          Effect.sync(() => {
            const listenedEvents = mocks.listen.mock.calls.map(([event]) => event);
            expect(listenedEvents.filter((event) => event === PREFS_SYNC_EVENT)).toHaveLength(1);
            expect(listenedEvents).not.toContain("agentscan-live-picker");
            expect(mocks.invoke).not.toHaveBeenCalled();
            expect(mocks.register).not.toHaveBeenCalled();
            expect(mocks.prefsUnlisten).not.toHaveBeenCalled();
          }),
        ),
      ),
    ),
  );

  expect(mocks.prefsUnlisten).toHaveBeenCalledTimes(1);
});
