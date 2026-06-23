// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SourceSwitcher, type SourceMenuItem } from "./SourceSwitcher";
import type {
  DesktopProfileConfig,
  LocalProfileConfig,
  SshProfileConfig,
} from "../effect/profileModel";

const localProfile: LocalProfileConfig = {
  id: "local",
  kind: "local",
  runner: { binaryPath: "", env: [] },
};

const sshProfile = (id: string, host: string, enabled = true): SshProfileConfig => ({
  id,
  kind: "ssh",
  host,
  clientTty: "",
  runner: { binaryPath: "", env: [] },
  enabled,
});

beforeEach(() => {
  if (!("setPointerCapture" in HTMLElement.prototype)) {
    Object.defineProperty(HTMLElement.prototype, "setPointerCapture", {
      configurable: true,
      value: () => {},
    });
  }
  if (!("releasePointerCapture" in HTMLElement.prototype)) {
    Object.defineProperty(HTMLElement.prototype, "releasePointerCapture", {
      configurable: true,
      value: () => {},
    });
  }
  if (!("hasPointerCapture" in HTMLElement.prototype)) {
    Object.defineProperty(HTMLElement.prototype, "hasPointerCapture", {
      configurable: true,
      value: () => true,
    });
  }
  if (!("elementFromPoint" in document)) {
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: () => null,
    });
  }
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function renderSwitcher({
  sourceMenuItems,
}: {
  sourceMenuItems?: SourceMenuItem[];
} = {}) {
  const reorderProfile = vi.fn();
  const setProfileEnabled = vi.fn();
  const labels = new Map<string, string>([
    ["local", "koopa"],
    ["ssh-1", "mander"],
    ["ssh-2", "luma"],
  ]);
  render(
    <SourceSwitcher
      sourceMenuItems={
        sourceMenuItems ?? [
          { profile: localProfile, enabled: true, canToggle: false, isOwner: true },
          { profile: sshProfile("ssh-1", "mander"), enabled: true, canToggle: true, isOwner: false },
          {
            profile: sshProfile("ssh-2", "luma", false),
            enabled: false,
            canToggle: true,
            isOwner: false,
          },
        ]
      }
      triggerProfile={localProfile}
      triggerShowsSource={false}
      triggerTone="idle"
      triggerTitle="Manage sources"
      orientation="vertical"
      labelFor={(profile: DesktopProfileConfig) => labels.get(profile.id) ?? profile.id}
      selectProfile={vi.fn()}
      reorderProfile={reorderProfile}
      setProfileEnabled={setProfileEnabled}
      onOpenSettings={vi.fn()}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Manage sources" }));
  return { reorderProfile, setProfileEnabled };
}

function rect({
  left = 20,
  top = 0,
  width = 320,
  height = 36,
}: Partial<DOMRect> = {}): DOMRect {
  return {
    x: left,
    y: top,
    left,
    top,
    width,
    height,
    right: left + width,
    bottom: top + height,
    toJSON: () => ({}),
  } as DOMRect;
}

function mockSourceMenuRects() {
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function () {
    const element = this as HTMLElement;
    if (element.dataset.sourceId === "local") {
      return rect({ top: 100 });
    }
    if (element.dataset.sourceId === "ssh-1") {
      return rect({ top: 136 });
    }
    if (element.dataset.sourceId === "ssh-2") {
      return rect({ top: 172 });
    }
    if (element.classList.contains("source-menu")) {
      return rect({ left: 20, top: 90, width: 340, height: 170 });
    }
    return rect({ top: 700, height: 30 });
  });
}

describe("SourceSwitcher", () => {
  it("shows disabled sources in the menu and toggles them back on", () => {
    const { setProfileEnabled } = renderSwitcher();

    const localCheckbox = screen.getByLabelText("Hide koopa in dock") as HTMLInputElement;
    expect(localCheckbox.disabled).toBe(true);
    expect(localCheckbox.checked).toBe(true);

    const disabledSource = screen.getByLabelText("Show luma in dock") as HTMLInputElement;
    expect(disabledSource.checked).toBe(false);
    fireEvent.click(disabledSource);

    expect(setProfileEnabled).toHaveBeenCalledWith({ id: "ssh-2", enabled: true });
  });

  it("reorders from the grip pointer release target", () => {
    const { reorderProfile } = renderSwitcher();
    const target = screen.getByText("koopa");
    vi.spyOn(document, "elementFromPoint").mockReturnValue(target);
    const grip = screen.getByLabelText("Drag luma to reorder");

    fireEvent.pointerDown(grip, { button: 0, pointerId: 1 });
    fireEvent.pointerUp(grip, { clientX: 110, clientY: 620, pointerId: 1 });

    expect(reorderProfile).toHaveBeenCalledWith({ id: "ssh-2", targetId: "local" });
  });

  it("moves mander below luma when released on luma", () => {
    const { reorderProfile } = renderSwitcher();
    const target = screen.getByText("luma");
    vi.spyOn(document, "elementFromPoint").mockReturnValue(target);
    const grip = screen.getByLabelText("Drag mander to reorder");

    fireEvent.pointerDown(grip, { button: 0, pointerId: 1 });
    fireEvent.pointerUp(grip, { clientX: 110, clientY: 653, pointerId: 1 });

    expect(reorderProfile).toHaveBeenCalledWith({ id: "ssh-1", targetId: "ssh-2" });
  });

  it("shows a drag ghost and insertion marker while reordering", () => {
    mockSourceMenuRects();
    const { reorderProfile } = renderSwitcher();
    const grip = screen.getByLabelText("Drag mander to reorder");

    fireEvent.pointerDown(grip, { button: 0, pointerId: 1, clientX: 330, clientY: 150 });
    fireEvent.pointerMove(grip, { pointerId: 1, clientX: 330, clientY: 205 });

    expect(document.body.querySelector(".source-drag-ghost")).not.toBeNull();
    expect(document.body.querySelector(".source-drop-marker")).not.toBeNull();

    fireEvent.pointerUp(grip, { pointerId: 1, clientX: 330, clientY: 205 });

    expect(reorderProfile).toHaveBeenCalledWith({ id: "ssh-1", targetId: "ssh-2" });
    expect(document.body.querySelector(".source-drag-ghost")).toBeNull();
    expect(document.body.querySelector(".source-drop-marker")).toBeNull();
  });

  it("does not commit a stale insertion marker when released back over the original row", () => {
    mockSourceMenuRects();
    const { reorderProfile } = renderSwitcher();
    const grip = screen.getByLabelText("Drag mander to reorder");

    fireEvent.pointerDown(grip, { button: 0, pointerId: 1, clientX: 330, clientY: 150 });
    fireEvent.pointerMove(grip, { pointerId: 1, clientX: 330, clientY: 205 });
    fireEvent.pointerUp(grip, { pointerId: 1, clientX: 330, clientY: 150 });

    expect(reorderProfile).not.toHaveBeenCalled();
    expect(document.body.querySelector(".source-drag-ghost")).toBeNull();
    expect(document.body.querySelector(".source-drop-marker")).toBeNull();
  });

  it("does not let the release-target fallback override a geometry no-op", () => {
    mockSourceMenuRects();
    const { reorderProfile } = renderSwitcher();
    vi.spyOn(document, "elementFromPoint").mockReturnValue(screen.getByText("luma"));
    const grip = screen.getByLabelText("Drag mander to reorder");

    fireEvent.pointerDown(grip, { button: 0, pointerId: 1, clientX: 330, clientY: 150 });
    fireEvent.pointerMove(grip, { pointerId: 1, clientX: 330, clientY: 180 });
    expect(document.body.querySelector(".source-drop-marker")).toBeNull();

    fireEvent.pointerUp(grip, { pointerId: 1, clientX: 330, clientY: 180 });

    expect(reorderProfile).not.toHaveBeenCalled();
    expect(document.body.querySelector(".source-drag-ghost")).toBeNull();
  });

  it("badges the trigger only in the horizontal bar when it names a source", () => {
    const baseProps = {
      sourceMenuItems: [
        { profile: localProfile, enabled: true, canToggle: false, isOwner: true },
      ],
      triggerProfile: localProfile,
      triggerTone: "idle",
      triggerTitle: "koopa",
      labelFor: (profile: DesktopProfileConfig) => (profile.id === "local" ? "koopa" : profile.id),
      selectProfile: vi.fn(),
      reorderProfile: vi.fn(),
      setProfileEnabled: vi.fn(),
      onOpenSettings: vi.fn(),
    };

    // Horizontal bar, trigger naming a source, >1 client: badge shows and the
    // count flows through to the label.
    const { rerender } = render(
      <SourceSwitcher {...baseProps} triggerShowsSource orientation="horizontal" attachedClientCount={2} />,
    );
    expect(screen.queryByLabelText(", 2 viewers")).not.toBeNull();

    // Vertical strip carries it per folder header instead — no trigger badge.
    rerender(
      <SourceSwitcher {...baseProps} triggerShowsSource orientation="vertical" attachedClientCount={2} />,
    );
    expect(screen.queryByRole("img")).toBeNull();

    // Generic "Manage sources" trigger names no source — no badge even horizontal.
    rerender(
      <SourceSwitcher
        {...baseProps}
        triggerShowsSource={false}
        orientation="horizontal"
        attachedClientCount={2}
      />,
    );
    expect(screen.queryByRole("img")).toBeNull();
  });
});
