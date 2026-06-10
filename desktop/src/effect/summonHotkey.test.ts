import { describe, expect, it } from "vitest";
import { summonHotkeyFailureMessage } from "./summonHotkey";

describe("summonHotkeyFailureMessage", () => {
  it("explains an in-use key from the macOS RegisterEventHotKey failure", () => {
    const message = summonHotkeyFailureMessage(
      new Error("Unable to register hotkey: RegisterEventHotKey failed for KeyA"),
    );
    expect(message).toBe(
      "⌘⇧A is in use — another agentscan instance may be running. Retrying until it frees up.",
    );
  });

  it("explains an in-use key from the plugin's already-registered error", () => {
    const message = summonHotkeyFailureMessage(new Error("HotKey already registereD"));
    expect(message).toContain("is in use");
  });

  it("falls back to the raw detail for other errors", () => {
    expect(summonHotkeyFailureMessage(new Error("permission denied"))).toBe(
      "Unable to register ⌘⇧A: permission denied",
    );
  });

  it("stringifies non-Error values", () => {
    expect(summonHotkeyFailureMessage("boom")).toBe("Unable to register ⌘⇧A: boom");
  });
});
