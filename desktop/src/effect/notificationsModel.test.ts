import { describe, expect, it, vi } from "vitest";
import type { LiveStates } from "./LiveConnection";
import {
  detectStatusTransitions,
  idleTransitions,
  notificationTransitions,
  NOTIFY_ON_IDLE_STORAGE_KEY,
  parseNotifyOnIdle,
  storeNotifyOnIdle,
  type StatusTransition,
  waitingTransitions,
} from "./notificationsModel";
import type { LiveState, PickerRow } from "./types";

const row = (
  paneId: string,
  status: string,
  provider: string | null = "claude",
): PickerRow => ({
  key: paneId,
  pane_id: paneId,
  provider,
  status: { kind: status },
  display_label: `session ${paneId}`,
  location_tag: "local",
  is_active: false,
});

const state = (runnerKey: string, rows: PickerRow[]): LiveState => ({
  connection: {
    status: "online",
    message: "ok",
    snapshot: { paneCount: rows.length, generatedAt: null, sourceKind: "tmux" },
  },
  rows,
  rowsRunnerKey: runnerKey,
});

const states = (...entries: Array<[string, PickerRow[]]>): LiveStates =>
  new Map(entries.map(([key, rows]) => [key, state(key, rows)]));

describe("notification preference persistence", () => {
  it("defaults off and accepts only the stored true value", () => {
    expect(parseNotifyOnIdle(null)).toBe(false);
    expect(parseNotifyOnIdle("false")).toBe(false);
    expect(parseNotifyOnIdle("invalid")).toBe(false);
    expect(parseNotifyOnIdle("true")).toBe(true);
  });

  it("stores explicit boolean strings", () => {
    const write = vi.fn();
    storeNotifyOnIdle(write, true);
    storeNotifyOnIdle(write, false);
    expect(write.mock.calls).toEqual([
      [NOTIFY_ON_IDLE_STORAGE_KEY, "true"],
      [NOTIFY_ON_IDLE_STORAGE_KEY, "false"],
    ]);
  });
});

describe("detectStatusTransitions", () => {
  it("detects busy to idle", () => {
    expect(
      detectStatusTransitions(states(["local", [row("%1", "busy")]]), states(["local", [row("%1", "idle")]])),
    ).toEqual([
      {
        paneId: "%1",
        provider: "claude",
        label: "session %1",
        from: "busy",
        to: "idle",
      },
    ]);
  });

  it("detects idle to busy generically", () => {
    expect(
      detectStatusTransitions(states(["local", [row("%1", "idle")]]), states(["local", [row("%1", "busy")]])),
    ).toMatchObject([{ paneId: "%1", from: "idle", to: "busy" }]);
  });

  it("detects transitions into waiting generically", () => {
    expect(
      detectStatusTransitions(
        states(["local", [row("%1", "busy")]]),
        states(["local", [row("%1", "waiting")]]),
      ),
    ).toMatchObject([{ paneId: "%1", from: "busy", to: "waiting" }]);
  });

  it("emits nothing for an unchanged status", () => {
    expect(
      detectStatusTransitions(states(["local", [row("%1", "busy")]]), states(["local", [row("%1", "busy")]])),
    ).toEqual([]);
  });

  it("emits nothing for a new pane", () => {
    expect(detectStatusTransitions(states(["local", []]), states(["local", [row("%1", "idle")]]))).toEqual([]);
  });

  it("emits nothing for a new runner key", () => {
    expect(detectStatusTransitions(new Map(), states(["remote", [row("%1", "idle")]]))).toEqual([]);
  });

  it("tracks identical pane ids independently per runner key", () => {
    const prev = states(
      ["local", [row("%1", "busy", "claude")]],
      ["remote", [row("%1", "idle", "codex")]],
    );
    const next = states(
      ["local", [row("%1", "idle", "claude")]],
      ["remote", [row("%1", "busy", "codex")]],
    );
    expect(detectStatusTransitions(prev, next)).toMatchObject([
      { paneId: "%1", provider: "claude", from: "busy", to: "idle" },
      { paneId: "%1", provider: "codex", from: "idle", to: "busy" },
    ]);
  });

  it("emits nothing when a pane disappears", () => {
    expect(detectStatusTransitions(states(["local", [row("%1", "busy")]]), states(["local", []]))).toEqual([]);
  });
});

describe("idleTransitions", () => {
  it("keeps busy or waiting to idle transitions", () => {
    const transition = (from: string, to: string): StatusTransition => ({
      paneId: `${from}-${to}`,
      provider: null,
      label: "agent",
      from,
      to,
    });
    expect(
      idleTransitions([
        transition("busy", "idle"),
        transition("waiting", "idle"),
        transition("idle", "busy"),
        transition("busy", "waiting"),
      ]),
    ).toEqual([transition("busy", "idle"), transition("waiting", "idle")]);
  });
});

describe("waitingTransitions", () => {
  it("keeps transitions into waiting from any other observed status", () => {
    const transition = (from: string, to: string): StatusTransition => ({
      paneId: `${from}-${to}`,
      provider: null,
      label: "agent",
      from,
      to,
    });
    expect(
      waitingTransitions([
        transition("busy", "waiting"),
        transition("idle", "waiting"),
        transition("unknown", "waiting"),
        transition("waiting", "idle"),
      ]),
    ).toEqual([
      transition("busy", "waiting"),
      transition("idle", "waiting"),
      transition("unknown", "waiting"),
    ]);
  });
});

describe("notificationTransitions", () => {
  const transition = (from: string, to: string): StatusTransition => ({
    paneId: `${from}-${to}`,
    provider: "claude",
    label: "agent",
    from,
    to,
  });

  it("selects waiting and completion notifications without notifying on resumed work", () => {
    expect(
      notificationTransitions(
        [
          transition("busy", "waiting"),
          transition("waiting", "idle"),
          transition("waiting", "busy"),
        ],
        true,
      ),
    ).toEqual([
      { ...transition("busy", "waiting"), notification: "waiting" },
      { ...transition("waiting", "idle"), notification: "finished" },
    ]);
  });

  it("suppresses all notifications when the existing preference is off", () => {
    expect(
      notificationTransitions(
        [transition("busy", "waiting"), transition("waiting", "idle")],
        false,
      ),
    ).toEqual([]);
  });
});
