import { describe, expect, it } from "vitest";
import {
  normalizePickerKeyboardKey,
  pickerKeyIntent,
  pickerRowForKeyboardKey,
  type PickerKeyEvent,
} from "./keybinds";
import { keybindOwnerId, type ProfileState } from "./profileModel";
import type { PickerRow } from "./types";

const row = (key: string, paneId: string): PickerRow => ({
  key,
  pane_id: paneId,
  provider: null,
  status: { kind: "idle" },
  display_label: "agent",
  location_tag: "main:1",
  is_active: false,
});

describe("normalizePickerKeyboardKey", () => {
  it("normalizes single alphanumeric keys case-insensitively", () => {
    expect(normalizePickerKeyboardKey("a")).toBe("A");
    expect(normalizePickerKeyboardKey("7")).toBe("7");
  });

  it("rejects modifier and multi-character keys", () => {
    expect(normalizePickerKeyboardKey("Enter")).toBeNull();
    expect(normalizePickerKeyboardKey("!")).toBeNull();
  });
});

describe("pickerRowForKeyboardKey", () => {
  it("matches the row by its configured picker key", () => {
    const rows = [row("1", "%1"), row("J", "%2")];
    expect(pickerRowForKeyboardKey(rows, "j")?.pane_id).toBe("%2");
    expect(pickerRowForKeyboardKey(rows, "9")).toBeUndefined();
  });
});

// The routing contract: Ctrl+<key> resolves ONLY against the keybind owner's rows
// (the topmost open folder); other sources' keys are informational. This mirrors
// how App.tsx feeds pickerRowForKeyboardKey exclusively the owner's rows.
describe("keybind routing to the owner", () => {
  const local = { id: "local", kind: "local" as const, runner: { binaryPath: "", env: [] } };
  const remote = {
    id: "ssh-1",
    kind: "ssh" as const,
    host: "box",
    clientTty: "",
    runner: { binaryPath: "", env: [] },
    enabled: true,
  };
  const rowsBySource: Record<string, PickerRow[]> = {
    local: [row("1", "%local-1")],
    "ssh-1": [row("2", "%remote-1")],
  };

  const resolve = (state: ProfileState, key: string) => {
    const owner = keybindOwnerId(state);
    return owner === null ? undefined : pickerRowForKeyboardKey(rowsBySource[owner], key);
  };

  it("answers only with the owner's rows; the other source's key is inert", () => {
    const state: ProfileState = {
      activeProfileId: "local",
      profiles: [local, remote],
      openProfileIds: ["local", "ssh-1"],
    };
    expect(resolve(state, "1")?.pane_id).toBe("%local-1");
    expect(resolve(state, "2")).toBeUndefined();
  });

  it("ownership (and routing) follows the source order", () => {
    const state: ProfileState = {
      activeProfileId: "local",
      profiles: [remote, local],
      openProfileIds: ["local", "ssh-1"],
    };
    expect(resolve(state, "2")?.pane_id).toBe("%remote-1");
    expect(resolve(state, "1")).toBeUndefined();
  });

  it("resolves nothing when no folder is open", () => {
    const state: ProfileState = {
      activeProfileId: "local",
      profiles: [local, remote],
      openProfileIds: [],
    };
    expect(resolve(state, "1")).toBeUndefined();
  });
});

describe("pickerKeyIntent", () => {
  const key = (k: string, mods: Partial<PickerKeyEvent> = {}): PickerKeyEvent => ({
    key: k,
    ctrlKey: false,
    metaKey: false,
    altKey: false,
    shiftKey: false,
    ...mods,
  });
  const ctrl = (k: string, mods: Partial<PickerKeyEvent> = {}) =>
    key(k, { ctrlKey: true, ...mods });
  const base = {
    bootScreenVisible: false,
    hasOwner: true,
    isInteractiveTarget: false,
    isMac: true,
    rows: [row("J", "%1"), row("2", "%2")],
    filterActive: false,
  };

  it("gates every key while the boot screen shows", () => {
    expect(pickerKeyIntent(ctrl("j"), { ...base, bootScreenVisible: true })).toBeNull();
    expect(pickerKeyIntent(key("Enter"), { ...base, bootScreenVisible: true })).toBeNull();
  });

  it("resolves Ctrl+<key> to an activate intent, Control alone only", () => {
    expect(pickerKeyIntent(ctrl("j"), base)).toEqual({ kind: "activate", row: base.rows[0] });
    // Ctrl+⌘ / Ctrl+Shift combos never activate — but "j" still falls into the
    // movement branch, which ignores modifiers (pre-extraction behavior).
    expect(pickerKeyIntent(ctrl("j", { metaKey: true }), base)).toEqual({
      kind: "move",
      delta: 1,
    });
    expect(pickerKeyIntent(ctrl("j", { shiftKey: true }), base)).toEqual({
      kind: "move",
      delta: 1,
    });
  });

  it("requires an owner for Ctrl activation; the miss still moves", () => {
    expect(pickerKeyIntent(ctrl("j"), { ...base, hasOwner: false })).toEqual({
      kind: "move",
      delta: 1,
    });
  });

  it("falls through a Ctrl miss into movement on a non-interactive target", () => {
    // "x" matches no row key; the press is NOT swallowed as null.
    expect(pickerKeyIntent(ctrl("k"), base)).toEqual({ kind: "move", delta: -1 });
    expect(pickerKeyIntent(ctrl("x"), base)).toBeNull();
  });

  it("on mac, Ctrl bypasses the interactive gate for a MATCH but a miss stops there", () => {
    const interactive = { ...base, isInteractiveTarget: true };
    expect(pickerKeyIntent(ctrl("j"), interactive)).toEqual({
      kind: "activate",
      row: base.rows[0],
    });
    // The miss falls through to the interactive gate — movement never runs on
    // an interactive target.
    expect(pickerKeyIntent(ctrl("k"), interactive)).toBeNull();
  });

  it("on non-mac, an interactive target never enters the Ctrl branch", () => {
    const interactive = { ...base, isMac: false, isInteractiveTarget: true };
    // Native clipboard/find/undo wins: even a key that WOULD match is inert.
    expect(pickerKeyIntent(ctrl("j"), interactive)).toBeNull();
    // Non-interactive still activates.
    expect(pickerKeyIntent(ctrl("j"), { ...base, isMac: false })).toEqual({
      kind: "activate",
      row: base.rows[0],
    });
  });

  it("maps movement and ignores other keys on interactive targets", () => {
    expect(pickerKeyIntent(key("ArrowDown"), base)).toEqual({ kind: "move", delta: 1 });
    expect(pickerKeyIntent(key("ArrowUp"), base)).toEqual({ kind: "move", delta: -1 });
    expect(pickerKeyIntent(key("j"), { ...base, isInteractiveTarget: true })).toBeNull();
  });

  it("maps Home/End to direct selection, null on empty rows", () => {
    expect(pickerKeyIntent(key("Home"), base)).toEqual({ kind: "select", paneId: "%1" });
    expect(pickerKeyIntent(key("End"), base)).toEqual({ kind: "select", paneId: "%2" });
    expect(pickerKeyIntent(key("Home"), { ...base, rows: [] })).toEqual({
      kind: "select",
      paneId: null,
    });
  });

  it("maps Enter to activateSelection unconditionally after the gates", () => {
    expect(pickerKeyIntent(key("Enter"), base)).toEqual({ kind: "activateSelection" });
    expect(pickerKeyIntent(key("Enter"), { ...base, rows: [], hasOwner: false })).toEqual({
      kind: "activateSelection",
    });
  });

  it("maps Escape with and without an active filter", () => {
    expect(pickerKeyIntent(key("Escape"), { ...base, filterActive: true })).toEqual({
      kind: "escape",
      clearFilter: true,
    });
    // No filter: still an escape intent (the applier's collapse-search side
    // effect must run), just nothing to clear / no preventDefault.
    expect(pickerKeyIntent(key("Escape"), base)).toEqual({
      kind: "escape",
      clearFilter: false,
    });
  });

  it("returns null for unhandled keys", () => {
    expect(pickerKeyIntent(key("a"), base)).toBeNull();
    expect(pickerKeyIntent(key("Tab"), base)).toBeNull();
  });
});
