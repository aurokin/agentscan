import { describe, expect, it } from "vitest";
import { hostnameProbeCandidates, sourceRunnerKeys } from "./enrichmentModel";
import type { LiveStates } from "./LiveConnection";
import { loadProfileState, runnerKeyForProfile, type ProfileState } from "./profileModel";
import type { LiveState } from "./types";

const ssh = (id: string, host: string, probedHost?: string) => ({
  id,
  kind: "ssh" as const,
  host,
  clientTty: "",
  runner: { binaryPath: "", env: [] },
  enabled: true,
  ...(probedHost ? { probedHost } : {}),
});

const LOCAL = { id: "local", kind: "local" as const, runner: { binaryPath: "", env: [] } };

// Build through the real loader so fixtures carry the same normalization the
// service sees (openProfileIds, trimmed fields).
const stateOf = (activeProfileId: string, profiles: unknown[]): ProfileState =>
  loadProfileState((key) =>
    key === "agentscan.desktop.profiles" ? JSON.stringify({ activeProfileId, profiles }) : null,
  );

const online = (runnerKey: string): LiveState => ({
  connection: {
    status: "online",
    message: "ok",
    snapshot: { paneCount: 0, generatedAt: null, sourceKind: "tmux" },
  },
  rows: [],
  rowsRunnerKey: runnerKey,
});

const NONE: ReadonlySet<string> = new Set();

describe("hostnameProbeCandidates", () => {
  it("offers exactly the never-probed, non-active, online ssh sources", () => {
    const state = stateOf("local", [LOCAL, ssh("s1", "box"), ssh("s2", "other")]);
    const k1 = runnerKeyForProfile(state.profiles[1]);
    const liveStates: LiveStates = new Map([[k1, online(k1)]]);

    // s2 is offline (absent key reads as connecting), local is not ssh.
    const candidates = hostnameProbeCandidates(state, liveStates, NONE);
    expect(candidates).toEqual([
      {
        profileId: "s1",
        host: "box",
        runnerKey: k1,
        settings: { kind: "ssh", host: "box", clientTty: null, binaryPath: "", env: [] },
      },
    ]);
  });

  it("skips the active source (the preflight driver already probes it)", () => {
    const state = stateOf("s1", [LOCAL, ssh("s1", "box")]);
    const k1 = runnerKeyForProfile(state.profiles[1]);
    expect(hostnameProbeCandidates(state, new Map([[k1, online(k1)]]), NONE)).toEqual([]);
  });

  it("skips already-probed and already-attempted sources", () => {
    const state = stateOf("local", [LOCAL, ssh("s1", "box", "boxy"), ssh("s2", "other")]);
    const k2 = runnerKeyForProfile(state.profiles[2]);
    const liveStates: LiveStates = new Map([
      [runnerKeyForProfile(state.profiles[1]), online("a")],
      [k2, online(k2)],
    ]);
    // s1 has a recorded probedHost; s2 was attempted this session.
    expect(hostnameProbeCandidates(state, liveStates, new Set([k2]))).toEqual([]);
  });

  it("requires the channel to be online, not merely configured", () => {
    const state = stateOf("local", [LOCAL, ssh("s1", "box")]);
    const k1 = runnerKeyForProfile(state.profiles[1]);
    const reconnecting: LiveState = {
      connection: { status: "reconnecting", message: "Reconnecting" },
      rows: [],
      rowsRunnerKey: null,
    };
    expect(hostnameProbeCandidates(state, new Map([[k1, reconnecting]]), NONE)).toEqual([]);
  });
});

describe("sourceRunnerKeys", () => {
  it("returns the folder-eligible sources' runner identities (the attempt-prune set)", () => {
    const state = stateOf("local", [LOCAL, ssh("s1", "box"), ssh("s2", "")]);
    const keys = sourceRunnerKeys(state);
    // The empty-host ssh profile is not folder-eligible, so its (degenerate)
    // key must not shield an attempt from pruning.
    expect(keys.has(runnerKeyForProfile(state.profiles[1]))).toBe(true);
    expect(keys.size).toBe(2); // local + s1
  });
});
