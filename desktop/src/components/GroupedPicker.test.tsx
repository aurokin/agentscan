// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { GroupedPicker } from "./GroupedPicker";
import type { PickerActivation, PickerGroup, PickerState } from "../effect/pickerViewModel";
import type { PickerRow } from "../effect/types";

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

  it("exposes the picker as a listbox whose active descendant tracks selection", () => {
    const row = (key: string, paneId: string): PickerRow => ({
      key,
      pane_id: paneId,
      provider: "claude",
      status: { kind: "idle" },
      display_label: `agent ${key}`,
      location_tag: "main:1",
      is_active: false,
    });
    const groups: PickerGroup[] = [
      { key: "proj-a", project: "proj-a", rows: [row("1", "%1")] },
      { key: "proj-b", project: "proj-b", rows: [row("2", "%2")] },
    ];
    const state: PickerState = { status: "ready", rows: [] };
    const { container } = renderPicker({ groups, state, totalRows: 2, selectedPaneId: "%2" });

    // One listbox spans the project groups (selection crosses group bounds),
    // groups carry their project name, rows are options with stable ids.
    const listbox = container.querySelector('[role="listbox"]');
    expect(listbox).not.toBeNull();
    expect(listbox?.getAttribute("tabindex")).toBe("0");
    const options = container.querySelectorAll('[role="option"]');
    expect(options.length).toBe(2);
    const selected = container.querySelector('[role="option"][aria-selected="true"]');
    expect(selected?.id).not.toBe("");
    // The listbox points assistive tech at the selected option.
    expect(listbox?.getAttribute("aria-activedescendant")).toBe(selected?.id);
    expect(container.querySelector('[role="group"][aria-label="proj-b"]')).not.toBeNull();
  });

  it("derives collision-free option ids and keeps non-owner pickers out of the tab order", () => {
    const groups: PickerGroup[] = [
      {
        key: "proj-a",
        project: "proj-a",
        rows: [
          {
            key: "1",
            pane_id: "%1",
            provider: "claude",
            status: { kind: "idle" },
            display_label: "agent 1",
            location_tag: "main:1",
            is_active: false,
          },
        ],
      },
    ];
    const state: PickerState = { status: "ready", rows: [] };

    // Source keys differing only in a sanitized character must not produce the
    // same option id for the same pane number (the escape is injective).
    const a = renderPicker({ groups, state, totalRows: 1, sourceKey: "ssh-a.b" });
    const b = renderPicker({ groups, state, totalRows: 1, sourceKey: "ssh-a-b" });
    const idA = a.container.querySelector('[role="option"]')?.id;
    const idB = b.container.querySelector('[role="option"]')?.id;
    expect(idA).toBeTruthy();
    expect(idA).not.toBe(idB);

    // Keys always drive the keybind owner's selection, so only the owner's
    // listbox is a tab stop; a non-owner picker must not take keyboard focus.
    const nonOwner = renderPicker({ groups, state, totalRows: 1, keybindsOwned: false });
    expect(
      nonOwner.container.querySelector('[role="listbox"]')?.hasAttribute("tabindex"),
    ).toBe(false);
  });

  it("omits aria-activedescendant when the selection lives in another source", () => {
    const groups: PickerGroup[] = [
      {
        key: "proj-a",
        project: "proj-a",
        rows: [
          {
            key: "1",
            pane_id: "%1",
            provider: "claude",
            status: { kind: "idle" },
            display_label: "agent 1",
            location_tag: "main:1",
            is_active: false,
          },
        ],
      },
    ];
    const state: PickerState = { status: "ready", rows: [] };
    // Selected pane id belongs to a different source's picker (ids collide
    // across hosts) — this listbox must not claim it as its own descendant.
    const { container } = renderPicker({ groups, state, totalRows: 1, selectedPaneId: "%9" });

    expect(container.querySelector('[role="listbox"]')?.hasAttribute("aria-activedescendant")).toBe(
      false,
    );
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
