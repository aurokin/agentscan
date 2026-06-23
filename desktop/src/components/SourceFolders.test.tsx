// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SourceFolders } from "./SourceFolders";
import { deriveSourceViews } from "../effect/pickerViewModel";
import type { LiveStates } from "../effect/LiveConnection";
import type { LiveState, PickerRow } from "../effect/types";
import type { DesktopProfileConfig, LocalProfileConfig, SshProfileConfig } from "../effect/profileModel";

const localProfile: LocalProfileConfig = {
  id: "local",
  kind: "local",
  runner: { binaryPath: "", env: [] },
};

const sshProfile: SshProfileConfig = {
  id: "ssh-1",
  kind: "ssh",
  host: "mander",
  clientTty: "",
  runner: { binaryPath: "", env: [] },
  enabled: true,
};

const row = (overrides: Partial<PickerRow> & { pane_id: string }): PickerRow => ({
  key: "1",
  provider: "claude",
  status: { kind: "idle" },
  display_label: "agent",
  location_tag: "main:1",
  is_active: false,
  ...overrides,
});

const liveOnline = (rows: PickerRow[], runnerKey: string): LiveState => ({
  connection: {
    status: "online",
    message: "ok",
    snapshot: { paneCount: rows.length, generatedAt: null, sourceKind: "tmux" },
  },
  rows,
  rowsRunnerKey: runnerKey,
});

function renderFolders(sources: { profile: DesktopProfileConfig; runnerKey: string; isOpen: boolean; isOwner: boolean }[], liveStates: LiveStates) {
  const sourceViews = deriveSourceViews(sources, liveStates, "", { status: "idle" });
  render(
    <SourceFolders
      sourceViews={sourceViews}
      activation={{ status: "idle" }}
      pickerFilter=""
      selectedPaneId={null}
      resolvedTheme="dark"
      runnerKey="local"
      preflightError={null}
      activeProfile={localProfile}
      labelFor={(profile) => (profile.id === "local" ? "koopa" : "mander")}
      onOpenSettings={vi.fn()}
      onToggleFolder={vi.fn()}
      onActivate={vi.fn()}
      onSelect={vi.fn()}
      onStart={vi.fn()}
      onReconnect={vi.fn()}
      onClearFilter={vi.fn()}
    />,
  );
}

afterEach(() => {
  cleanup();
});

describe("SourceFolders multi-client badge", () => {
  it("badges an open folder whose server has >1 client, and suppresses it on a closed folder", () => {
    // Both servers report >1 client; only the OPEN folder should badge. The
    // closed folder carries a (here artificially populated) count to prove the
    // isOpen gate suppresses it rather than just an incidentally empty row list.
    renderFolders(
      [
        { profile: localProfile, runnerKey: "local", isOpen: true, isOwner: true },
        { profile: sshProfile, runnerKey: "ssh-1", isOpen: false, isOwner: false },
      ],
      new Map<string, LiveState>([
        ["local", liveOnline([row({ pane_id: "%1", attached_client_count: 2 })], "local")],
        ["ssh-1", liveOnline([row({ pane_id: "%2", attached_client_count: 3 })], "ssh-1")],
      ]),
    );

    // Open folder: the badge suffixes the host name with a separator (not glued,
    // e.g. not "koopa2 viewers") in the header button's accessible name.
    expect(screen.getByRole("button", { name: "koopa, 2 viewers" })).not.toBeNull();
    // Closed folder: header names the host only — no badge despite its rows
    // carrying attached_client_count: 3.
    expect(screen.getByRole("button", { name: "mander" })).not.toBeNull();
    expect(screen.queryByRole("button", { name: "mander, 3 viewers" })).toBeNull();
  });

  it("does not badge an open folder whose server has a single client", () => {
    renderFolders(
      [{ profile: localProfile, runnerKey: "local", isOpen: true, isOwner: true }],
      new Map<string, LiveState>([
        ["local", liveOnline([row({ pane_id: "%1", attached_client_count: 1 })], "local")],
      ]),
    );

    // Single client: header names the host only, with no viewers suffix.
    expect(screen.getByRole("button", { name: "koopa" })).not.toBeNull();
    expect(screen.queryByRole("button", { name: /viewers/ })).toBeNull();
  });
});
