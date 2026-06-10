// The picker's per-source view derivation, extracted from App.tsx so its
// contracts — the recovering mask, the rows-while-reconnecting projection, the
// schema<5 focus fallback, first-seen group order — are unit-testable instead
// of living only in comments. App.tsx keeps the useMemo wrapper (and its dep
// list) and all rendering; everything here is pure.

import { liveStateFor, type LiveStates } from "./LiveConnection";
import type { ConnectionStatus, LiveState, PickerRow } from "./types";

export type PickerGroup = {
  key: string;
  project: string;
  rows: PickerRow[];
};

export type PickerState =
  | { status: "loading" }
  | { status: "ready"; rows: PickerRow[] }
  | { status: "failed"; message: string };

// Activations are tagged with the runnerKey of the row's OWN source so the
// running pulse / failure recovery scope to that source's folder (pane ids like
// %1 collide across hosts). A null sourceKey marks a source-less failure (the
// summon-hotkey registration error reuses this banner).
export type PickerActivation =
  | { status: "idle" }
  | { status: "running"; paneId: string; sourceKey: string }
  | { status: "failed"; message: string; sourceKey: string | null };

export type SourceView<S> = S & {
  live: LiveState;
  state: PickerState;
  allRows: PickerRow[];
  rows: PickerRow[];
  groups: PickerGroup[];
  focusedPaneId: string | null;
};

// Per-source folder views: each source's keyed live state, the PickerState
// projection of it, and the query-filtered workspace groups. The filter applies
// across all open folders. A failed focus re-arms that source's live client
// (activateRow's catch) to drop the now-dead pane; until the fresh snapshot
// lands the keyed rows still carry it — reconnecting preserves rows to avoid a
// flicker on a healthy manual reconnect — so THAT source's list is gated to
// "loading" during the recovery (scoped by activation.sourceKey) instead of
// leaving the known-dead row clickable and instantly re-triggerable.
export function deriveSourceViews<S extends { readonly runnerKey: string }>(
  sources: readonly S[],
  liveStates: LiveStates,
  filter: string,
  activation: PickerActivation,
): SourceView<S>[] {
  return sources.map((source) => {
    const live = liveStateFor(liveStates, source.runnerKey);
    const recovering =
      activation.status === "failed" &&
      activation.sourceKey === source.runnerKey &&
      (live.connection.status === "connecting" ||
        live.connection.status === "reconnecting");
    const state: PickerState = recovering
      ? { status: "loading" }
      : pickerStateFromLive(live, source.runnerKey);
    const allRows = state.status === "ready" ? state.rows : [];
    const rows = groupRowsByProject(filterPickerRows(allRows, filter)).flatMap(
      (group) => group.rows,
    );
    return {
      ...source,
      live,
      state,
      allRows,
      rows,
      groups: groupRowsByProject(rows),
      focusedPaneId: focusedPaneIdOf(allRows),
    };
  });
}

// The live-pane marker, from a source's UNFILTERED rows (the search filter must
// not change the focus signal). Prefer the collapsed `is_focused` signal. If no
// row carries the field — an older or remote `agentscan` (schema < 5) that
// doesn't emit it — fall back to the first `is_active` pane so the picker still
// defaults to/highlights a live pane instead of going dark. When rows DO carry
// the field but none is focused, that's an honest "no focus", not a fallback
// case.
export function focusedPaneIdOf(rows: readonly PickerRow[]): string | null {
  return (
    rows.find((row) => row.is_focused)?.pane_id ??
    (rows.some((row) => row.is_focused !== undefined)
      ? null
      : (rows.find((row) => row.is_active)?.pane_id ?? null))
  );
}

// Project one source's keyed live state onto the PickerState its folder renders.
// The service is the single owner of rows + connection status; this just picks the
// view: keep showing the last rows while (re)connecting so the list doesn't flash
// a skeleton on a brief blip, show the failure only when a fatal state has
// actually cleared the rows, and otherwise a loading skeleton.
//
// Rows are trusted only when their producing runner (rowsRunnerKey) matches the
// key being rendered. Within a keyed entry that always holds (frames are routed by
// sourceKey), so this is a defensive guard kept from the single-target days.
export function pickerStateFromLive(live: LiveState, runnerKey: string): PickerState {
  const { connection } = live;
  const rows = live.rowsRunnerKey === runnerKey ? live.rows : [];
  if (rows.length > 0) {
    return { status: "ready", rows };
  }
  if (connection.status === "fatal") {
    return { status: "failed", message: connection.message };
  }
  if (connection.status === "connecting" || connection.status === "reconnecting") {
    return { status: "loading" };
  }
  // online or noDaemon with no (matching) rows → an empty (but resolved) list.
  return { status: "ready", rows };
}

function projectOf(row: PickerRow): string {
  const workspaceLabel = row.workspace?.label?.trim();
  if (workspaceLabel) {
    return workspaceLabel;
  }

  const tag = row.location_tag.trim();
  const session = tag.split(":", 1)[0]?.trim();
  return session || "ungrouped";
}

function projectKeyOf(row: PickerRow): string {
  const workspaceId = row.workspace?.id?.trim();
  if (workspaceId) {
    return workspaceId;
  }

  return projectOf(row);
}

export function paneSuffix(row: PickerRow): string {
  const tag = row.location_tag.trim();
  if (row.workspace?.source && row.workspace.source !== "session") {
    return tag;
  }

  const colon = tag.indexOf(":");
  return colon >= 0 ? tag.slice(colon + 1) : "";
}

// Group rows by backend workspace context, preserving first-seen order both
// across groups and within each group so keyboard nav matches what's rendered.
export function groupRowsByProject(rows: PickerRow[]): PickerGroup[] {
  const groups: PickerGroup[] = [];
  const byProject = new Map<string, PickerGroup>();

  for (const row of rows) {
    const projectKey = projectKeyOf(row);
    const project = projectOf(row);
    let group = byProject.get(projectKey);
    if (!group) {
      group = { key: projectKey, project, rows: [] };
      byProject.set(projectKey, group);
      groups.push(group);
    }
    group.rows.push(row);
  }

  return groups;
}

export function statusTone(kind: string): string {
  switch (kind) {
    case "busy":
      return "busy";
    case "idle":
      return "idle";
    case "error":
      return "error";
    default:
      return "unknown";
  }
}

// Tone for a source's folder/footer status dot, from its keyed live connection.
export function connectionTone(connection: ConnectionStatus): string {
  switch (connection.status) {
    case "online":
      return "idle";
    case "fatal":
      return "error";
    case "noDaemon":
      return "busy";
    case "connecting":
    case "reconnecting":
      return "unknown";
  }
}

export function filterPickerRows(rows: PickerRow[], query: string) {
  const terms = query
    .trim()
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean);

  if (terms.length === 0) {
    return rows;
  }

  return rows.filter((row) => {
    const searchable = [
      row.key,
      row.pane_id,
      row.provider ?? "unknown",
      row.status.kind,
      row.display_label,
      row.location_tag,
      row.workspace?.label ?? "",
      row.workspace?.source ?? "",
    ]
      .join(" ")
      .toLowerCase();

    return terms.every((term) => searchable.includes(term));
  });
}

export function liveStateLabel(status: ConnectionStatus) {
  switch (status.status) {
    case "online":
      return "Live";
    case "reconnecting":
      return "Reconnecting";
    case "noDaemon":
      return "No daemon";
    case "fatal":
      return "Live client failed";
    case "connecting":
      return "Connecting";
  }
}
