import { describe, expect, it } from "vitest";
import {
  deriveSourceViews,
  filterPickerRows,
  focusedPaneIdOf,
  footerTriggerView,
  groupRowsByProject,
  mruVisiblePaneId,
  pickerStateFromLive,
  reconcileSelection,
  statusTone,
  type PickerActivation,
} from "./pickerViewModel";
import type { LiveStates } from "./LiveConnection";
import type { LiveState, PickerRow } from "./types";

const row = (overrides: Partial<PickerRow> & { pane_id: string }): PickerRow => ({
  key: "1",
  provider: "claude",
  status: { kind: "idle" },
  display_label: "agent",
  location_tag: "main:1",
  is_active: false,
  ...overrides,
});

const liveOnline = (rows: PickerRow[], rowsRunnerKey: string | null): LiveState => ({
  connection: { status: "online", message: "ok", snapshot: { paneCount: rows.length, generatedAt: null, sourceKind: "tmux" } },
  rows,
  rowsRunnerKey,
});

const IDLE: PickerActivation = { status: "idle" };

describe("statusTone", () => {
  it("renders waiting distinctly and keeps unknown kinds tolerant", () => {
    expect(statusTone("busy")).toBe("busy");
    expect(statusTone("idle")).toBe("idle");
    expect(statusTone("waiting")).toBe("waiting");
    expect(statusTone("future-status")).toBe("unknown");
  });
});

describe("pickerStateFromLive", () => {
  it("keeps showing the last rows while reconnecting (no skeleton flash)", () => {
    const live: LiveState = {
      connection: { status: "reconnecting", message: "Reconnecting to agentscan" },
      rows: [row({ pane_id: "%1" })],
      rowsRunnerKey: "k1",
    };
    expect(pickerStateFromLive(live, "k1")).toEqual({
      status: "ready",
      rows: live.rows,
    });
  });

  it("shows a loading skeleton while (re)connecting with no rows yet", () => {
    const live: LiveState = {
      connection: { status: "connecting", message: "Connecting" },
      rows: [],
      rowsRunnerKey: null,
    };
    expect(pickerStateFromLive(live, "k1")).toEqual({ status: "loading" });
  });

  it("rejects rows produced by a different runner", () => {
    const live = liveOnline([row({ pane_id: "%1" })], "other-runner");
    expect(pickerStateFromLive(live, "k1")).toEqual({ status: "ready", rows: [] });
  });

  it("surfaces a fatal connection as failed only once rows are gone", () => {
    const fatal: LiveState = {
      connection: { status: "fatal", message: "boom" },
      rows: [],
      rowsRunnerKey: null,
    };
    expect(pickerStateFromLive(fatal, "k1")).toEqual({ status: "failed", message: "boom" });

    // Matching rows win over the fatal status (failure shows only when the
    // fatal state actually cleared the rows).
    const fatalWithRows: LiveState = { ...fatal, rows: [row({ pane_id: "%1" })], rowsRunnerKey: "k1" };
    expect(pickerStateFromLive(fatalWithRows, "k1").status).toBe("ready");
  });
});

describe("focusedPaneIdOf", () => {
  it("prefers the collapsed is_focused signal", () => {
    const rows = [
      row({ pane_id: "%1", is_focused: false, is_active: true }),
      row({ pane_id: "%2", is_focused: true }),
    ];
    expect(focusedPaneIdOf(rows)).toBe("%2");
  });

  it("falls back to the first is_active pane when no row carries is_focused (schema < 5)", () => {
    const rows = [row({ pane_id: "%1" }), row({ pane_id: "%2", is_active: true })];
    expect(focusedPaneIdOf(rows)).toBe("%2");
  });

  it("reports an honest no-focus when rows carry the field but none is focused", () => {
    const rows = [
      row({ pane_id: "%1", is_focused: false, is_active: true }),
      row({ pane_id: "%2", is_focused: false }),
    ];
    expect(focusedPaneIdOf(rows)).toBeNull();
  });
});

describe("footerTriggerView", () => {
  const owner = { id: "ssh-1" };
  const active = { id: "local" };
  const base = {
    ownerProfile: owner,
    activeProfile: active,
    ownerConnection: { status: "fatal", message: "Live client exited" } as const,
    sourceStatusTone: "idle",
    statusText: "Local CLI ready",
    orientation: "vertical" as const,
    sourceCount: 1,
  };

  it("presents the active profile with its preflight tone when it is the owner (or no owner)", () => {
    const view = footerTriggerView({ ...base, ownerProfile: null, ownerConnection: null });
    expect(view).toEqual({
      profile: active,
      showsSource: true,
      tone: "idle",
      title: "Local CLI ready",
    });
    expect(
      footerTriggerView({ ...base, ownerProfile: active, ownerConnection: null }).tone,
    ).toBe("idle");
  });

  it("presents a non-active owner by its keyed live connection, never the preflight", () => {
    // A non-active owner is never probed.
    const view = footerTriggerView(base);
    expect(view.profile).toBe(owner);
    expect(view.tone).toBe("error");
    expect(view.title).toBe("Live client exited");
  });

  it("falls back to unknown/statusText when the owner has no connection state yet", () => {
    const view = footerTriggerView({ ...base, ownerConnection: null });
    expect(view.tone).toBe("unknown");
    expect(view.title).toBe("Local CLI ready");
  });

  it("shows the source label only horizontally or with a single source", () => {
    expect(footerTriggerView(base).showsSource).toBe(true);
    expect(footerTriggerView({ ...base, sourceCount: 2 }).showsSource).toBe(false);
    expect(
      footerTriggerView({ ...base, sourceCount: 2, orientation: "horizontal" }).showsSource,
    ).toBe(true);
  });
});

describe("mruVisiblePaneId", () => {
  it("returns the visible row with the greatest defined last_focus_seq", () => {
    const rows = [
      row({ pane_id: "%1", last_focus_seq: 3 }),
      row({ pane_id: "%2" }),
      row({ pane_id: "%3", last_focus_seq: 8 }),
    ];
    expect(mruVisiblePaneId(rows)).toBe("%3");
  });

  it("returns null when no row carries the field (schema < 6 / never focused)", () => {
    expect(mruVisiblePaneId([row({ pane_id: "%1" }), row({ pane_id: "%2" })])).toBeNull();
    expect(mruVisiblePaneId([])).toBeNull();
  });
});

describe("reconcileSelection", () => {
  const rows = (...paneIds: string[]) => paneIds.map((pane_id) => row({ pane_id }));
  const base = {
    status: "ready" as const,
    allRowsCount: 2,
    rows: rows("%1", "%2"),
    selectedPaneId: null,
    focusedPaneId: null,
    prevFocusedPaneId: null,
  };

  it("does nothing while loading", () => {
    expect(reconcileSelection({ ...base, status: "loading" })).toEqual({});
  });

  it("clears selection and the follow marker when no data exists", () => {
    expect(
      reconcileSelection({ ...base, allRowsCount: 0, rows: [], selectedPaneId: "%1" }),
    ).toEqual({ selectedPaneId: null, prevFocusedPaneId: null });
  });

  it("omits the selection key when no-data finds it already null", () => {
    const step = reconcileSelection({ ...base, allRowsCount: 0, rows: [] });
    expect(step).toEqual({ prevFocusedPaneId: null });
    expect("selectedPaneId" in step).toBe(false);
  });

  it("leaves everything untouched when the filter matched nothing", () => {
    // Clearing the filter must restore the prior selection.
    expect(reconcileSelection({ ...base, rows: [], selectedPaneId: "%1" })).toEqual({});
  });

  it("follows a genuine focus move to a visible pane", () => {
    expect(
      reconcileSelection({
        ...base,
        selectedPaneId: "%1",
        focusedPaneId: "%2",
        prevFocusedPaneId: "%1",
      }),
    ).toEqual({ prevFocusedPaneId: "%2", selectedPaneId: "%2" });
  });

  it("initializes the follow marker on first observation without clobbering a manual pick", () => {
    // prev null is first observation / re-init, NOT a move.
    const step = reconcileSelection({
      ...base,
      selectedPaneId: "%1",
      focusedPaneId: "%2",
    });
    expect(step).toEqual({ prevFocusedPaneId: "%2" });
    expect("selectedPaneId" in step).toBe(false);
  });

  it("repairs an invalid selection to the visible focused pane", () => {
    expect(
      reconcileSelection({
        ...base,
        selectedPaneId: "%gone",
        focusedPaneId: "%2",
        prevFocusedPaneId: "%2",
      }),
    ).toEqual({ prevFocusedPaneId: "%2", selectedPaneId: "%2" });
  });

  it("repairs an invalid selection to the first row when focus is hidden or unknown", () => {
    const step = reconcileSelection({ ...base, selectedPaneId: "%gone" });
    expect(step).toEqual({ selectedPaneId: "%1" });
    // The marker is left alone so a pending move is still followed when the
    // focused pane reappears.
    expect("prevFocusedPaneId" in step).toBe(false);

    const hidden = reconcileSelection({
      ...base,
      selectedPaneId: "%gone",
      focusedPaneId: "%hidden",
      prevFocusedPaneId: "%hidden",
    });
    expect(hidden).toEqual({ selectedPaneId: "%1" });
  });

  it("repairs an invalid selection to the most recently focused pane before first row", () => {
    const recencyRows = [
      row({ pane_id: "%1" }),
      row({ pane_id: "%2", last_focus_seq: 7 }),
      row({ pane_id: "%3", last_focus_seq: 2 }),
    ];
    // Focus hidden or unknown: recency beats first-row.
    expect(
      reconcileSelection({ ...base, rows: recencyRows, selectedPaneId: "%gone" }),
    ).toEqual({ selectedPaneId: "%2" });
    // A visible focused pane still outranks recency.
    expect(
      reconcileSelection({
        ...base,
        rows: recencyRows,
        selectedPaneId: "%gone",
        focusedPaneId: "%3",
        prevFocusedPaneId: "%3",
      }),
    ).toEqual({ prevFocusedPaneId: "%3", selectedPaneId: "%3" });
    // All-undefined seqs fall back to the first row.
    expect(reconcileSelection({ ...base, selectedPaneId: "%gone" })).toEqual({
      selectedPaneId: "%1",
    });
  });

  it("keeps a valid selection and the marker while the focused pane is filtered out", () => {
    // Focus moved while hidden: nothing happens until the pane is visible again.
    expect(
      reconcileSelection({
        ...base,
        selectedPaneId: "%1",
        focusedPaneId: "%hidden",
        prevFocusedPaneId: "%other",
      }),
    ).toEqual({});
  });
});

describe("groupRowsByProject", () => {
  it("preserves first-seen order across groups and within each group", () => {
    const rows = [
      row({ pane_id: "%1", workspace: { id: "b", label: "Beta" } }),
      row({ pane_id: "%2", workspace: { id: "a", label: "Alpha" } }),
      row({ pane_id: "%3", workspace: { id: "b", label: "Beta" } }),
    ];
    const groups = groupRowsByProject(rows);
    expect(groups.map((g) => g.project)).toEqual(["Beta", "Alpha"]);
    expect(groups[0].rows.map((r) => r.pane_id)).toEqual(["%1", "%3"]);
  });

  it("keys by workspace id when present and falls back to the session label", () => {
    const rows = [
      row({ pane_id: "%1", workspace: { id: "w1", label: "Same" } }),
      row({ pane_id: "%2", workspace: { id: "w2", label: "Same" } }),
      row({ pane_id: "%3", location_tag: "sess:0" }),
    ];
    const groups = groupRowsByProject(rows);
    expect(groups.map((g) => g.key)).toEqual(["w1", "w2", "sess"]);
    expect(groups[2].project).toBe("sess");
  });
});

describe("filterPickerRows", () => {
  it("returns the same array for an empty query and requires every term to match", () => {
    const rows = [
      row({ pane_id: "%1", provider: "claude", display_label: "claude web", location_tag: "api:1" }),
      row({ pane_id: "%2", provider: "codex", display_label: "codex web", location_tag: "api:2" }),
    ];
    expect(filterPickerRows(rows, "  ")).toBe(rows);
    expect(filterPickerRows(rows, "web api").map((r) => r.pane_id)).toEqual(["%1", "%2"]);
    expect(filterPickerRows(rows, "claude api").map((r) => r.pane_id)).toEqual(["%1"]);
    expect(filterPickerRows(rows, "claude codex")).toEqual([]);
  });
});

describe("deriveSourceViews", () => {
  const source = (runnerKey: string) => ({ runnerKey, name: runnerKey });

  it("masks ONLY the failed source's list to loading while its client recovers", () => {
    const failed: PickerActivation = { status: "failed", message: "focus failed", sourceKey: "k1" };
    const reconnecting: LiveState = {
      connection: { status: "reconnecting", message: "Reconnecting" },
      rows: [row({ pane_id: "%dead" })],
      rowsRunnerKey: "k1",
    };
    const states: LiveStates = new Map([
      ["k1", reconnecting],
      ["k2", liveOnline([row({ pane_id: "%2" })], "k2")],
    ]);

    const [k1, k2] = deriveSourceViews([source("k1"), source("k2")], states, "", failed);
    // The known-dead row must not stay clickable while recovery is in flight,
    // even though reconnecting preserved it in the keyed rows.
    expect(k1.state).toEqual({ status: "loading" });
    expect(k1.rows).toEqual([]);
    // A sibling source is untouched by the mask.
    expect(k2.state.status).toBe("ready");
    expect(k2.rows.map((r) => r.pane_id)).toEqual(["%2"]);
  });

  it("lifts the mask once the failed source's connection settles", () => {
    const failed: PickerActivation = { status: "failed", message: "focus failed", sourceKey: "k1" };
    const states: LiveStates = new Map([["k1", liveOnline([row({ pane_id: "%1" })], "k1")]]);
    const [k1] = deriveSourceViews([source("k1")], states, "", failed);
    expect(k1.state.status).toBe("ready");
  });

  it("derives filtered rows in group order and the per-source focus marker from unfiltered rows", () => {
    const rows = [
      row({ pane_id: "%1", display_label: "alpha", workspace: { id: "b", label: "B" } }),
      row({ pane_id: "%2", display_label: "match", workspace: { id: "a", label: "A" } }),
      row({ pane_id: "%3", display_label: "match", workspace: { id: "b", label: "B" }, is_focused: true }),
    ];
    const states: LiveStates = new Map([["k1", liveOnline(rows, "k1")]]);

    const [view] = deriveSourceViews([source("k1")], states, "match", IDLE);
    // Group order is first-seen over the FILTERED rows (%2 in A before %3 in
    // B), so a filter can reorder groups relative to the unfiltered list.
    expect(view.rows.map((r) => r.pane_id)).toEqual(["%2", "%3"]);
    expect(view.groups.map((g) => g.project)).toEqual(["A", "B"]);
    // allRows and the focus marker ignore the filter.
    expect(view.allRows.map((r) => r.pane_id)).toEqual(["%1", "%2", "%3"]);
    expect(view.focusedPaneId).toBe("%3");
    // Source fields ride along untouched.
    expect(view.name).toBe("k1");
  });

  it("resolves an unconfigured key to the initial connecting state", () => {
    const [view] = deriveSourceViews([source("k1")], new Map(), "", IDLE);
    expect(view.live.connection.status).toBe("connecting");
    expect(view.state).toEqual({ status: "loading" });
  });
});
