// Pure desktop-notification preference and live-status transition model. No React,
// Effect, Tauri, or global storage access: callers inject persistence and native I/O.

import type { LiveStates } from "./LiveConnection";
import type { LiveState } from "./types";
import type { StorageWrite } from "./profileModel";

export const NOTIFY_ON_IDLE_STORAGE_KEY = "agentscan.desktop.notifyOnIdle";

export function parseNotifyOnIdle(raw: string | null): boolean {
  // Default OFF on a first run; once toggled, only the stored true value enables it.
  return raw === "true";
}

export const storeNotifyOnIdle = (write: StorageWrite, value: boolean) =>
  write(NOTIFY_ON_IDLE_STORAGE_KEY, value ? "true" : "false");

export type StatusTransition = {
  paneId: string;
  provider: string | null;
  label: string;
  from: string;
  to: string;
};

export function detectStatusTransitions(
  prev: ReadonlyMap<string, LiveState> | LiveStates,
  next: LiveStates,
): StatusTransition[] {
  const transitions: StatusTransition[] = [];

  for (const [runnerKey, nextState] of next) {
    const prevState = prev.get(runnerKey);
    if (prevState === undefined) {
      continue;
    }

    const previousRows = new Map(prevState.rows.map((row) => [row.pane_id, row]));
    for (const row of nextState.rows) {
      const previous = previousRows.get(row.pane_id);
      if (previous === undefined || previous.status.kind === row.status.kind) {
        continue;
      }
      transitions.push({
        paneId: row.pane_id,
        provider: row.provider,
        label: row.display_label,
        from: previous.status.kind,
        to: row.status.kind,
      });
    }
  }

  return transitions;
}

export const idleTransitions = (transitions: StatusTransition[]): StatusTransition[] =>
  transitions.filter(({ from, to }) => from === "busy" && to === "idle");
