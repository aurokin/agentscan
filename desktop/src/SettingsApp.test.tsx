// @vitest-environment jsdom
//
// Mount smoke test for the settings window: under mocked Tauri APIs,
// SettingsApp renders its form without preflightStateAtom/liveStatesAtom ever
// being configured, registers only its own window listeners (focus reconcile,
// close-to-hide), and never touches the dock's native/service paths (no
// summon-hotkey registration, no preflight probe, no live subscription, no
// window sizing). The mirror image of DockApp.test.tsx. One mount per file:
// the Atom runtime and its keepAlive atoms are module-scoped, so a second
// mount would start from warm state.
//
// Atom updates land outside act() (Effect fibers push SubscriptionRef changes
// into React asynchronously), so React logs act() warnings here — that's
// expected; the waitFor polling below is the right way to settle.
import { StrictMode } from "react";
import { expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";

(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;

const mocks = vi.hoisted(() => {
  const invoke = vi.fn((cmd: string): Promise<unknown> => {
    if (cmd === "local_host_label") {
      return Promise.resolve("testhost");
    }
    // Nothing else should fire from the settings window; a rejection here is
    // caught by the Effect/promise paths and surfaced by the assertions below.
    return Promise.reject(new Error(`unexpected invoke from settings: ${cmd}`));
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
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen, emitTo: mocks.emitTo }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => mocks.winStub,
  LogicalSize: class {
    constructor(
      public width: number,
      public height: number,
    ) {}
  },
}));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  // The settings window's label; the services resolve their own mode from it.
  getCurrentWebviewWindow: () => ({ label: "settings" }),
  WebviewWindow: { getByLabel: async () => null },
}));
vi.mock("@tauri-apps/plugin-global-shortcut", () => ({
  register: mocks.register,
  unregister: mocks.unregister,
}));

// The About section's update check calls window.fetch on mount; stub it so the
// smoke test never touches the network. A failed (ok: false) response exercises
// the silent-failure path: the version line renders with no update hint.
window.fetch = vi.fn(async () => ({ ok: false }) as Response);

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

import SettingsApp from "./SettingsApp";

it("boots settings: renders the form and registers only settings-side listeners", async () => {
  const { unmount } = render(
    <StrictMode>
      <SettingsApp />
    </StrictMode>,
  );

  // The form renders synchronously from the localStorage-seeded drafts — no
  // preflight or live data involved.
  screen.getByRole("heading", { name: "Settings" });
  screen.getByLabelText("agentscan binary");
  expect(
    screen
      .getByRole("switch", { name: "Notify when an agent finishes" })
      .getAttribute("aria-checked"),
  ).toBe("false");
  // The dock's UI must not exist here.
  expect(screen.queryByLabelText("Search agents")).toBeNull();

  // The mocked local_host_label resolves through HostIpc → localHostLabelAtom →
  // sourceLabel and lands as the active source's heading. This pins the whole
  // atom path end-to-end. (Depends on the single-profile default of a fresh
  // jsdom localStorage: with extra profiles the label moves to the source-rail
  // card and this heading query would break.)
  await screen.findByRole("heading", { name: "testhost" });

  // Settings-only window listeners bind (focus reconcile + close-to-hide).
  await vi.waitFor(() => {
    expect(mocks.winStub.onFocusChanged).toHaveBeenCalled();
    expect(mocks.winStub.onCloseRequested).toHaveBeenCalled();
  });

  // None of the dock's paths may arm from this window.
  expect(mocks.register).not.toHaveBeenCalled();
  const invoked = mocks.invoke.mock.calls.map((call) => call[0]);
  expect(invoked).not.toContain("preflight_agentscan");
  expect(invoked).not.toContain("start_live_picker");
  expect(invoked).not.toContain("set_window_decorations");
  const listened = mocks.listen.mock.calls.map((call) => call[0]);
  expect(listened).not.toContain("agentscan-live-picker");
  expect(mocks.winStub.setMinSize).not.toHaveBeenCalled();

  unmount();
});
