import { describe, expect, it } from "vitest";
import {
  activePreflightError,
  dockBootScreenContent,
  dockBootScreenVisible,
  liveTargetsFor,
  matchedPreflight,
  preflightSourceTone,
  preflightStatusText,
  preflightUnusable,
} from "./preflightViewModel";
import type { PreflightState } from "./Preflight";
import type { AgentscanPreflight } from "./profileModel";

const preflight = (overrides: Partial<AgentscanPreflight> = {}): AgentscanPreflight => ({
  ok: true,
  binary: "agentscan",
  version: "0.7.2",
  error: null,
  remoteHostLabel: null,
  suggestedBinaryPath: null,
  ...overrides,
});

const ready = (runnerKey: string, p: Partial<AgentscanPreflight> = {}): PreflightState => ({
  status: "ready",
  runnerKey,
  preflight: preflight(p),
});

const LOADING: PreflightState = { status: "loading" };
const FAILED: PreflightState = { status: "failed", message: "ipc boom" };

describe("matchedPreflight", () => {
  it("returns the ready state only when its runnerKey matches the active runner", () => {
    const state = ready("k1");
    expect(matchedPreflight(state, "k1")).toBe(state);
    // A resolved state from the previous target (probe lags a switch by one
    // async cycle) must not drive decisions for the new runner.
    expect(matchedPreflight(state, "k2")).toBeNull();
    expect(matchedPreflight(LOADING, "k1")).toBeNull();
    expect(matchedPreflight(FAILED, "k1")).toBeNull();
  });
});

describe("preflightStatusText", () => {
  it("reports Checking until the resolved state matches the active profile", () => {
    expect(preflightStatusText(LOADING, "k1", "Local")).toBe("Checking Local CLI");
    expect(preflightStatusText(ready("other"), "k1", "Local")).toBe("Checking Local CLI");
  });

  it("reports the matched verdict and the failed-probe asymmetry", () => {
    expect(preflightStatusText(ready("k1"), "k1", "Local")).toBe("Local CLI ready");
    expect(preflightStatusText(ready("k1", { ok: false }), "k1", "SSH")).toBe(
      "SSH CLI unavailable",
    );
    // "failed" carries no runnerKey: it always describes the active runner.
    expect(preflightStatusText(FAILED, "k1", "Local")).toBe("IPC failed");
  });
});

describe("preflightSourceTone", () => {
  it("treats the switching window as unknown, then maps the matched verdict", () => {
    expect(preflightSourceTone(LOADING, "k1")).toBe("unknown");
    expect(preflightSourceTone(ready("other"), "k1")).toBe("unknown");
    expect(preflightSourceTone(FAILED, "k1")).toBe("unknown");
    expect(preflightSourceTone(ready("k1"), "k1")).toBe("idle");
    expect(preflightSourceTone(ready("k1", { ok: false }), "k1")).toBe("error");
  });
});

describe("preflightUnusable", () => {
  it("is true only for a current-runner probe that reports the CLI unavailable", () => {
    expect(preflightUnusable(ready("k1", { ok: false }), "k1")).toBe(true);
    expect(preflightUnusable(ready("k1"), "k1")).toBe(false);
    // A stale failing probe from a profile still switching is not an error.
    expect(preflightUnusable(ready("other", { ok: false }), "k1")).toBe(false);
    expect(preflightUnusable(FAILED, "k1")).toBe(false);
  });
});

describe("activePreflightError", () => {
  it("never surfaces over an online channel (probes gate starting only)", () => {
    expect(activePreflightError(ready("k1", { ok: false, error: "bad" }), "k1", true, "L")).toBeNull();
    expect(activePreflightError(FAILED, "k1", true, "L")).toBeNull();
  });

  it("surfaces an unusable matched probe with its real error or a fallback", () => {
    expect(
      activePreflightError(ready("k1", { ok: false, error: "no such binary" }), "k1", false, "SSH"),
    ).toBe("no such binary");
    expect(activePreflightError(ready("k1", { ok: false }), "k1", false, "SSH")).toBe(
      "SSH CLI unavailable",
    );
  });

  it("treats a failed probe as the active runner's, and stale/healthy probes as no error", () => {
    expect(activePreflightError(FAILED, "k1", false, "L")).toBe("ipc boom");
    expect(activePreflightError(ready("other", { ok: false }), "k1", false, "L")).toBeNull();
    expect(activePreflightError(ready("k1"), "k1", false, "L")).toBeNull();
    expect(activePreflightError(LOADING, "k1", false, "L")).toBeNull();
  });
});

describe("liveTargetsFor", () => {
  const source = (
    runnerKey: string,
    overrides: Partial<{ isOpen: boolean; valid: boolean }> = {},
  ) => ({
    runnerKey,
    settings: null,
    isOpen: true,
    valid: true,
    ...overrides,
  });

  const enabledOf = (
    sources: ReturnType<typeof source>[],
    runnerKey: string,
    online: boolean,
    state: PreflightState,
    activeValid = true,
  ) => liveTargetsFor(sources, runnerKey, online, state, activeValid).map((t) => t.enabled);

  it("filters closed sources out entirely", () => {
    const targets = liveTargetsFor(
      [source("k1"), source("k2", { isOpen: false })],
      "k1",
      false,
      ready("k1"),
      true,
    );
    expect(targets.map((t) => t.runnerKey)).toEqual(["k1"]);
  });

  it("resolves an unmatched probe for the active key to carry, never a gate-off", () => {
    // The probe lags a switch by one async cycle; bouncing here would tear
    // down a healthy subscription the user merely re-selected.
    expect(enabledOf([source("k1")], "k1", false, ready("other"))).toEqual(["carry"]);
    expect(enabledOf([source("k1")], "k1", false, LOADING)).toEqual(["carry"]);
    expect(enabledOf([source("k1")], "k1", false, FAILED)).toEqual(["carry"]);
  });

  it("lets the online latch win over a failed matched probe", () => {
    // Probes gate STARTING; an online channel is never killed by a verdict.
    expect(enabledOf([source("k1")], "k1", true, ready("k1", { ok: false }))).toEqual([true]);
  });

  it("gates the active key on the matched verdict and committed validity", () => {
    expect(enabledOf([source("k1")], "k1", false, ready("k1"))).toEqual([true]);
    expect(enabledOf([source("k1")], "k1", false, ready("k1", { ok: false }))).toEqual([false]);
    expect(enabledOf([source("k1")], "k1", false, ready("k1"), false)).toEqual([false]);
  });

  it("arms non-active sources on their committed validity verbatim", () => {
    // Never probed: their enabled is the valid flag, regardless of the active
    // runner's preflight.
    expect(
      enabledOf(
        [source("k1"), source("k2"), source("k3", { valid: false })],
        "k1",
        false,
        ready("k1", { ok: false }),
      ),
    ).toEqual([false, true, false]);
  });
});

describe("dockBootScreenVisible", () => {
  const base = {
    isDock: true,
    activeLiveOnline: false,
    activeFolderOpen: true,
    hasOpenFolderBeyondActive: false,
  };

  it("shows for loading, failed, and current-runner-unusable states", () => {
    expect(dockBootScreenVisible(LOADING, "k1", base)).toBe(true);
    expect(dockBootScreenVisible(FAILED, "k1", base)).toBe(true);
    expect(dockBootScreenVisible(ready("k1", { ok: false }), "k1", base)).toBe(true);
  });

  it("never shows over a healthy matched probe or an online channel", () => {
    expect(dockBootScreenVisible(ready("k1"), "k1", base)).toBe(false);
    // The stream is ground truth while it runs: no probe verdict blanks an
    // online dock.
    expect(
      dockBootScreenVisible(ready("k1", { ok: false }), "k1", { ...base, activeLiveOnline: true }),
    ).toBe(false);
  });

  it("a stale unusable probe (runner switching) does not blank the dock", () => {
    expect(dockBootScreenVisible(ready("other", { ok: false }), "k1", base)).toBe(false);
  });

  it("stays inside the folder model: suppressed by other open folders, a closed active folder, or the settings window", () => {
    expect(
      dockBootScreenVisible(LOADING, "k1", { ...base, hasOpenFolderBeyondActive: true }),
    ).toBe(false);
    expect(dockBootScreenVisible(LOADING, "k1", { ...base, activeFolderOpen: false })).toBe(false);
    expect(dockBootScreenVisible(LOADING, "k1", { ...base, isDock: false })).toBe(false);
  });
});

describe("dockBootScreenContent", () => {
  it("maps each state to its copy and offers the one-click binary fix only when unusable", () => {
    expect(dockBootScreenContent(LOADING, "Local")).toEqual({
      probing: true,
      detail: "Waiting for the daemon…",
      suggestedBinaryPath: null,
    });
    expect(dockBootScreenContent(FAILED, "Local")).toEqual({
      probing: false,
      detail: "ipc boom",
      suggestedBinaryPath: null,
    });
    expect(
      dockBootScreenContent(ready("k1", { ok: false, error: "not found", suggestedBinaryPath: "/opt/bin/agentscan" }), "SSH"),
    ).toEqual({
      probing: false,
      detail: "not found",
      suggestedBinaryPath: "/opt/bin/agentscan",
    });
    expect(dockBootScreenContent(ready("k1", { ok: false }), "SSH").detail).toBe(
      "SSH CLI unavailable",
    );
  });
});
