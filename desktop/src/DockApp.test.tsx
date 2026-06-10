// @vitest-environment jsdom
//
// Mount smoke test for the dock window: under mocked Tauri APIs, DockApp boots
// past the recovery screen on a canned successful preflight, arms its dock-only
// native/service paths (summon hotkey, preflight probe, window sizing, the live
// picker listener), and never registers the settings window's listeners. This
// pins THE SPLIT's contract from the dock side; SettingsApp.test.tsx pins the
// mirror image. One mount per file: the Atom runtime and its keepAlive atoms
// are module-scoped, so a second mount would start from warm state.
//
// Atom updates land outside act() (Effect fibers push SubscriptionRef changes
// into React asynchronously), so React logs act() warnings here — that's
// expected; the findBy/waitFor polling below is the right way to settle, and
// wrapping the mount in fake timers would break it.
import { StrictMode } from "react";
import { expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";

(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;

const mocks = vi.hoisted(() => {
  const invoke = vi.fn((cmd: string): Promise<unknown> => {
    switch (cmd) {
      case "preflight_agentscan":
        // ok:true so the boot screen clears; remoteHostLabel null so hostname
        // enrichment records nothing (a non-null label would commit a profile
        // change mid-test).
        return Promise.resolve({
          binary: "agentscan",
          ok: true,
          version: "test",
          error: null,
          suggestedBinaryPath: null,
          remoteHostLabel: null,
        });
      case "local_host_label":
        return Promise.resolve("testhost");
      case "start_live_picker":
        // A start rejection parks the live connection as fatal with no retry
        // timers, keeping the mount deterministic.
        return Promise.reject(new Error("test: no daemon"));
      case "poll_daemon_status":
        return Promise.resolve({ reachable: false });
      default:
        // place_picker_window / place_bar_window / set_window_decorations /
        // stop_live_picker: benign no-ops.
        return Promise.resolve(undefined);
    }
  });
  const listen = vi.fn(async () => () => {});
  const emitTo = vi.fn(async () => {});
  const register = vi.fn(async () => {});
  const unregister = vi.fn(async () => {});
  const winStub = {
    onFocusChanged: vi.fn(async () => () => {}),
    onCloseRequested: vi.fn(async () => () => {}),
    hide: vi.fn(async () => {}),
    unminimize: vi.fn(async () => {}),
    show: vi.fn(async () => {}),
    setFocus: vi.fn(async () => {}),
    minimize: vi.fn(async () => {}),
    setMinSize: vi.fn(async () => {}),
    setMaxSize: vi.fn(async () => {}),
  };
  return { invoke, listen, emitTo, register, unregister, winStub };
});

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
// emitTo must exist: PrefsBridge imports it and Preflight broadcasts every
// resolved probe through it.
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen, emitTo: mocks.emitTo }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => mocks.winStub,
  // Real constructor: the sizing effect news these synchronously.
  LogicalSize: class {
    constructor(
      public width: number,
      public height: number,
    ) {}
  },
}));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  // The dock window's label; the services resolve their own mode from it.
  getCurrentWebviewWindow: () => ({ label: "main" }),
  WebviewWindow: { getByLabel: async () => null },
}));
vi.mock("@tauri-apps/plugin-global-shortcut", () => ({
  register: mocks.register,
  unregister: mocks.unregister,
}));

// jsdom has no matchMedia; the theme effect calls it unguarded.
window.matchMedia = ((media: string) => ({
  matches: false,
  media,
  onchange: null,
  addEventListener: () => {},
  removeEventListener: () => {},
  addListener: () => {},
  removeListener: () => {},
  dispatchEvent: () => false,
})) as unknown as typeof window.matchMedia;

import DockApp from "./DockApp";

it("boots the dock: clears the recovery screen and arms only dock-side paths", async () => {
  const { unmount } = render(
    <StrictMode>
      <DockApp />
    </StrictMode>,
  );

  // The canned ok preflight clears the boot screen; the topbar search control
  // is the dock sentinel (rendered by both orientations).
  await screen.findByLabelText("Search agents");
  // The settings window's UI must not exist here.
  expect(screen.queryByRole("heading", { name: "Settings" })).toBeNull();

  // Dock-only arming. StrictMode legitimately re-runs configures, so assert
  // "was called with", never call counts.
  expect(mocks.register).toHaveBeenCalledWith("CommandOrControl+Shift+A", expect.any(Function));
  expect(mocks.invoke).toHaveBeenCalledWith("preflight_agentscan", expect.anything());
  expect(mocks.winStub.setMinSize).toHaveBeenCalled();
  // The live subscription listener arms only after the preflight enables the
  // active target, so settle on it.
  await vi.waitFor(() => {
    expect(mocks.listen.mock.calls.map((call) => call[0])).toContain("agentscan-live-picker");
  });

  // Settings-only listeners must never bind in the dock.
  expect(mocks.winStub.onFocusChanged).not.toHaveBeenCalled();

  unmount();
});
