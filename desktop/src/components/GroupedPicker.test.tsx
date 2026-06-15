// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { GroupedPicker } from "./GroupedPicker";
import type { PickerActivation, PickerState } from "../effect/pickerViewModel";

const IDLE: PickerActivation = { status: "idle" };

function renderPicker(overrides: Partial<Parameters<typeof GroupedPicker>[0]> = {}) {
  const state: PickerState = { status: "ready", rows: [] };
  return render(
    <GroupedPicker
      activation={IDLE}
      filterQuery=""
      focusedPaneId={null}
      groups={[]}
      keybindsOwned={true}
      logoTheme="dark"
      selectedPaneId={null}
      sourceKey="local"
      state={state}
      totalRows={0}
      onActivate={vi.fn()}
      onClearFilter={vi.fn()}
      onSelect={vi.fn()}
      {...overrides}
    />,
  );
}

describe("GroupedPicker", () => {
  it("renders the resolved empty state without the brand logo", () => {
    const { container } = renderPicker();

    expect(screen.getByRole("status").textContent).toBe("No agents here");
    expect(container.querySelector(".empty-logo")).toBeNull();
  });

  it("suppresses the empty placeholder when the connection is offline", () => {
    // A LiveStrip (e.g. "No daemon / Start agentscan") renders above in this case,
    // so a second "No agents here" would wrongly imply a successful empty scan.
    // Scoped to this render's container: the suite has no cleanup hook, so the
    // first test's mount still lives in document.body when this one runs.
    const { container } = renderPicker({ connectionOffline: true });

    expect(container.querySelector(".empty-detected")).toBeNull();
    expect(container.textContent).not.toContain("No agents here");
  });
});
