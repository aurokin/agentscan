import { describe, expect, it } from "vitest";
import {
  normalizeProfileState,
  runnerKeyForProfile,
  sourceLabel,
  validateProfileDraft,
  type DesktopProfileConfig,
  type LocalProfileConfig,
  type PreflightLabelSource,
  type SshProfileConfig,
} from "./profileModel";

const localProfile: LocalProfileConfig = {
  id: "local",
  kind: "local",
  runner: { binaryPath: "", env: [] },
};

const sshProfile = (id: string, host: string): SshProfileConfig => ({
  id,
  kind: "ssh",
  host,
  clientTty: "",
  runner: { binaryPath: "", env: [] },
  enabled: true,
});

describe("validateProfileDraft", () => {
  it("rejects an SSH host already used by another profile", () => {
    const profiles: DesktopProfileConfig[] = [localProfile, sshProfile("ssh-1", "box")];
    const draft = sshProfile("ssh-2", "");
    const validation = validateProfileDraft(
      draft,
      draft.runner,
      "  box  ",
      "",
      [...profiles, draft],
    );
    expect(validation.errors).toContain("A source for this connection already exists.");
  });

  it("accepts a profile keeping its own host", () => {
    const draft = sshProfile("ssh-1", "box");
    const validation = validateProfileDraft(draft, draft.runner, "box", "", [
      localProfile,
      draft,
    ]);
    expect(validation.errors).toEqual([]);
  });
});

describe("normalizeProfileState", () => {
  it("drops later profiles that duplicate an earlier trimmed host", () => {
    const state = normalizeProfileState({
      activeProfileId: "local",
      profiles: [
        localProfile,
        sshProfile("ssh-1", "box"),
        sshProfile("ssh-2", " box "),
        sshProfile("ssh-3", "other"),
      ],
    });
    expect(state.profiles.map((profile) => profile.id)).toEqual(["local", "ssh-1", "ssh-3"]);
  });

  it("keeps multiple still-unconfigured (empty-host) remotes", () => {
    const state = normalizeProfileState({
      activeProfileId: "local",
      profiles: [localProfile, sshProfile("ssh-1", ""), sshProfile("ssh-2", "")],
    });
    expect(state.profiles.map((profile) => profile.id)).toEqual(["local", "ssh-1", "ssh-2"]);
  });

  it("strips the legacy user-editable name persisted by older versions", () => {
    const state = normalizeProfileState({
      activeProfileId: "local",
      profiles: [
        { ...localProfile, name: "Default" },
        { ...sshProfile("ssh-1", "box"), name: "My Box" },
      ] as unknown as DesktopProfileConfig[],
    });
    expect(state.profiles).toHaveLength(2);
    for (const profile of state.profiles) {
      expect(profile).not.toHaveProperty("name");
    }
  });
});

describe("sourceLabel", () => {
  it("labels a remote by its trimmed SSH host", () => {
    expect(sourceLabel(sshProfile("ssh-1", " user@box "), "mymac")).toBe("user@box");
  });

  it("falls back to a generic label for an empty remote host", () => {
    expect(sourceLabel(sshProfile("ssh-1", ""), "mymac")).toBe("Remote");
  });

  it("labels the local source by the machine hostname, with a generic fallback", () => {
    expect(sourceLabel(localProfile, "mymac")).toBe("mymac");
    expect(sourceLabel(localProfile, "")).toBe("agentscan");
  });

  it("prefers the probed remote hostname when the preflight matches the profile's runner", () => {
    const profile = sshProfile("ssh-1", "user@box");
    const preflight: PreflightLabelSource = {
      runnerKey: runnerKeyForProfile(profile),
      preflight: { remoteHostLabel: "koopa" },
    };
    expect(sourceLabel(profile, "mymac", preflight)).toBe("koopa");
  });

  it("ignores a probed hostname whose runnerKey does not match the profile", () => {
    const profile = sshProfile("ssh-1", "user@box");
    const stale: PreflightLabelSource = {
      runnerKey: runnerKeyForProfile(sshProfile("ssh-2", "user@other")),
      preflight: { remoteHostLabel: "koopa" },
    };
    expect(sourceLabel(profile, "mymac", stale)).toBe("user@box");
  });

  it("falls back to the configured host when the matching preflight probed no hostname", () => {
    const profile = sshProfile("ssh-1", "user@box");
    const preflight: PreflightLabelSource = {
      runnerKey: runnerKeyForProfile(profile),
      preflight: { remoteHostLabel: null },
    };
    expect(sourceLabel(profile, "mymac", preflight)).toBe("user@box");
    expect(sourceLabel(profile, "mymac", { ...preflight, preflight: null })).toBe("user@box");
  });

  it("never applies a probed hostname to the local source", () => {
    const preflight: PreflightLabelSource = {
      runnerKey: runnerKeyForProfile(localProfile),
      preflight: { remoteHostLabel: "koopa" },
    };
    expect(sourceLabel(localProfile, "mymac", preflight)).toBe("mymac");
  });
});
