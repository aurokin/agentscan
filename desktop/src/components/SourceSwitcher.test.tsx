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
});
