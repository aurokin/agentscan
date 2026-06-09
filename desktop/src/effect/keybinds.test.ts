import { describe, expect, it } from "vitest";
import { normalizePickerKeyboardKey, pickerRowForKeyboardKey } from "./keybinds";
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
