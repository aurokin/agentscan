import { describe, expect, it } from "vitest";
import { settingsPreflightCard } from "./settingsViewModel";
import type { SyncedPreflight } from "./Preflight";
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

const synced = (overrides: Partial<SyncedPreflight> = {}): SyncedPreflight => ({
  status: "ready",
  runnerKey: "k1",
  preflight: preflight(),
  ...overrides,
});

describe("settingsPreflightCard", () => {
  it("reads Checking before any mirror arrives", () => {
    expect(settingsPreflightCard(null, "k1")).toEqual({
      tone: "unknown",
      label: "Checking",
      detail: "Probing agentscan…",
    });
  });

  it("distrusts a mirror whose runnerKey mismatches — even a failed one", () => {
    // Mid-switch the mirror still describes the PREVIOUS source; a stale
    // failure must read as Checking, never as Unreachable.
    expect(
      settingsPreflightCard(synced({ status: "failed", preflight: null }), "other"),
    ).toEqual({
      tone: "unknown",
      label: "Checking",
      detail: "Probing agentscan…",
    });
  });

  it("reads Unreachable on a matched failed dock status", () => {
    expect(settingsPreflightCard(synced({ status: "failed", preflight: null }), "k1")).toEqual({
      tone: "error",
      label: "Unreachable",
      detail: "Can’t reach agentscan",
    });
  });

  it("reads Ready with binary · version, falling back to ready", () => {
    expect(settingsPreflightCard(synced(), "k1")).toEqual({
      tone: "idle",
      label: "Ready",
      detail: "agentscan · 0.7.2",
    });
    expect(
      settingsPreflightCard(synced({ preflight: preflight({ version: null }) }), "k1").detail,
    ).toBe("agentscan · ready");
  });

  it("reads Unavailable with the probe's error, falling back to a generic line", () => {
    expect(
      settingsPreflightCard(
        synced({ preflight: preflight({ ok: false, error: "not found" }) }),
        "k1",
      ),
    ).toEqual({ tone: "error", label: "Unavailable", detail: "not found" });
    expect(
      settingsPreflightCard(synced({ preflight: preflight({ ok: false }) }), "k1").detail,
    ).toBe("agentscan unavailable");
  });
});
