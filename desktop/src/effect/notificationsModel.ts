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

export type StatusNotification = StatusTransition & {
  notification: "finished" | "waiting";
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

const isIdleTransition = ({ from, to }: StatusTransition): boolean =>
  (from === "busy" || from === "waiting") && to === "idle";

const isWaitingTransition = ({ from, to }: StatusTransition): boolean =>
  from !== "waiting" && to === "waiting";

export const idleTransitions = (transitions: StatusTransition[]): StatusTransition[] =>
  transitions.filter(isIdleTransition);

export const waitingTransitions = (transitions: StatusTransition[]): StatusTransition[] =>
  transitions.filter(isWaitingTransition);

export function notificationTransitions(
  transitions: StatusTransition[],
  enabled: boolean,
): StatusNotification[] {
  if (!enabled) {
    return [];
  }

  return transitions.flatMap((transition): StatusNotification[] => {
    if (isWaitingTransition(transition)) {
      return [{ ...transition, notification: "waiting" }];
    }
    if (isIdleTransition(transition)) {
      return [{ ...transition, notification: "finished" }];
    }
    return [];
  });
}
