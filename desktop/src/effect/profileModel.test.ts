import { describe, expect, it } from "vitest";
import {
  committedProfileValidation,
  defaultProfileState,
  keybindOwnerId,
  liveSourcesFor,
  normalizeProfileState,
  profileDraftDirty,
  recordProbedHost,
  reorderProfile,
  runnerKeyForProfile,
  sourceLabel,
  toggleProfileOpen,
  updateProfileSettings,
  validateProfileDraft,
  type DesktopProfileConfig,
  type LocalProfileConfig,
  type PreflightLabelSource,
  type ProfileState,
  type SshProfileConfig,
} from "./profileModel";

const localProfile: LocalProfileConfig = {
  id: "local",
  kind: "local",
  runner: { binaryPath: "", env: [] },
};

const sshProfile = (id: string, host: string, enabled = true): SshProfileConfig => ({
  id,
  kind: "ssh",
  host,
  clientTty: "",
  runner: { binaryPath: "", env: [] },
  enabled,
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

  it("prefers a runnable survivor when deduplicating a host", () => {
    // A disabled duplicate sitting first must not win over an enabled one: the
    // kept profile is what the dock can actually show, and references remap to it.
    const state = normalizeProfileState({
      activeProfileId: "ssh-disabled",
      profiles: [localProfile, sshProfile("ssh-disabled", "box", false), sshProfile("ssh-enabled", "box")],
      openProfileIds: ["ssh-disabled"],
    });
    expect(state.profiles.map((profile) => profile.id)).toEqual(["local", "ssh-enabled"]);
    expect(state.activeProfileId).toBe("ssh-enabled");
    expect(state.openProfileIds).toEqual(["ssh-enabled"]);
  });

  it("remaps a dropped duplicate's active and open references to the survivor", () => {
    // The user's selection was the connection, not the duplicate row: an upgrade
    // must not flip the settings target to local or close the connection's folder.
    const state = normalizeProfileState({
      activeProfileId: "ssh-2",
      profiles: [localProfile, sshProfile("ssh-1", "box"), sshProfile("ssh-2", "box")],
      openProfileIds: ["ssh-2"],
    });
    expect(state.profiles.map((profile) => profile.id)).toEqual(["local", "ssh-1"]);
    expect(state.activeProfileId).toBe("ssh-1");
    expect(state.openProfileIds).toEqual(["ssh-1"]);
  });

  it("collapses multiple still-unconfigured (empty-host) drafts to the first", () => {
    // Labels derive from the connection, so identical "Remote" cards would be
    // indistinguishable; one draft is all that's needed to resume configuring.
    const state = normalizeProfileState({
      activeProfileId: "local",
      profiles: [localProfile, sshProfile("ssh-1", ""), sshProfile("ssh-2", "")],
    });
    expect(state.profiles.map((profile) => profile.id)).toEqual(["local", "ssh-1"]);
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

  it("migrates a state persisted before the folder UI to open the active profile", () => {
    const state = normalizeProfileState({
      activeProfileId: "ssh-1",
      profiles: [localProfile, sshProfile("ssh-1", "box")],
    });
    expect(state.openProfileIds).toEqual(["ssh-1"]);
  });

  it("keeps persisted open ids, dropping unknown profiles and duplicates", () => {
    const state = normalizeProfileState({
      activeProfileId: "local",
      profiles: [localProfile, sshProfile("ssh-1", "box")],
      openProfileIds: ["ssh-1", "ssh-1", "ghost"],
    });
    expect(state.openProfileIds).toEqual(["ssh-1"]);
  });

  it("preserves an explicit all-closed state (no migration on an empty list)", () => {
    const state = normalizeProfileState({
      activeProfileId: "local",
      profiles: [localProfile, sshProfile("ssh-1", "box")],
      openProfileIds: [],
    });
    expect(state.openProfileIds).toEqual([]);
  });

  it("defaults a fresh state to an open local folder", () => {
    expect(defaultProfileState().openProfileIds).toEqual(["local"]);
  });
});

const stateOf = (
  openProfileIds: string[],
  ...profiles: DesktopProfileConfig[]
): ProfileState => ({
  activeProfileId: profiles[0]?.id ?? "local",
  profiles,
  openProfileIds,
});

describe("keybindOwnerId", () => {
  it("picks the FIRST OPEN profile in source order", () => {
    const state = stateOf(
      ["ssh-1", "local"],
      localProfile,
      sshProfile("ssh-1", "box"),
      sshProfile("ssh-2", "other"),
    );
    expect(keybindOwnerId(state)).toBe("local");
  });

  it("returns null when no folder is open", () => {
    expect(keybindOwnerId(stateOf([], localProfile, sshProfile("ssh-1", "box")))).toBeNull();
  });

  it("skips a closed earlier source", () => {
    const state = stateOf(["ssh-1"], localProfile, sshProfile("ssh-1", "box"));
    expect(keybindOwnerId(state)).toBe("ssh-1");
  });

  it("ignores open ids that are not folder-eligible (empty host, disabled)", () => {
    const disabled: SshProfileConfig = { ...sshProfile("ssh-2", "other"), enabled: false };
    const state = stateOf(
      ["ssh-empty", "ssh-2", "ssh-1"],
      sshProfile("ssh-empty", ""),
      disabled,
      sshProfile("ssh-1", "box"),
      localProfile,
    );
    expect(keybindOwnerId(state)).toBe("ssh-1");
  });

  it("follows a reorder, and passes to the next open folder when the owner closes", () => {
    let state = stateOf(["local", "ssh-1"], localProfile, sshProfile("ssh-1", "box"));
    expect(keybindOwnerId(state)).toBe("local");

    state = reorderProfile(state, "ssh-1", "local");
    expect(keybindOwnerId(state)).toBe("ssh-1");

    state = toggleProfileOpen(state, "ssh-1");
    expect(keybindOwnerId(state)).toBe("local");
  });
});

describe("liveSourcesFor", () => {
  it("marks exactly the FIRST open folder in user order as owner", () => {
    const state = stateOf(
      ["ssh-1", "local"],
      localProfile,
      sshProfile("ssh-1", "box"),
      sshProfile("ssh-2", "other"),
    );
    const sources = liveSourcesFor(state);
    expect(sources.map((source) => source.profile.id)).toEqual(["local", "ssh-1", "ssh-2"]);
    expect(sources.map((source) => source.isOwner)).toEqual([true, false, false]);
    expect(sources.map((source) => source.isOpen)).toEqual([true, true, false]);
  });

  it("marks no owner when every folder is closed", () => {
    const state = stateOf([], localProfile, sshProfile("ssh-1", "box"));
    expect(liveSourcesFor(state).every((source) => !source.isOwner)).toBe(true);
  });

  it("judges validity on committed values", () => {
    // A null byte in the committed binary path fails committedProfileValidation;
    // the sibling stays valid.
    const broken: SshProfileConfig = {
      ...sshProfile("ssh-1", "box"),
      runner: { binaryPath: "agent\0scan", env: [] },
    };
    const state = stateOf(["local"], localProfile, broken);
    const sources = liveSourcesFor(state);
    expect(sources.find((source) => source.profile.id === "local")?.valid).toBe(true);
    expect(sources.find((source) => source.profile.id === "ssh-1")?.valid).toBe(false);
  });

  it("carries each source's runner identity and settings", () => {
    const state = stateOf(["local"], localProfile, sshProfile("ssh-1", "box"));
    for (const source of liveSourcesFor(state)) {
      expect(source.runnerKey).toBe(runnerKeyForProfile(source.profile));
    }
  });
});

describe("committedProfileValidation", () => {
  it("validates committed host/tty, ignoring any notion of drafts", () => {
    const profiles: DesktopProfileConfig[] = [localProfile, sshProfile("ssh-1", "")];
    const validation = committedProfileValidation(profiles[1], profiles);
    expect(validation.errors).toContain("SSH host is required.");
    expect(committedProfileValidation(localProfile, profiles).errors).toEqual([]);
  });
});

describe("profileDraftDirty", () => {
  const committed: SshProfileConfig = {
    ...sshProfile("ssh-1", "box"),
    clientTty: "/dev/ttys001",
    runner: { binaryPath: "agentscan", env: [] },
  };

  it("ignores surrounding whitespace on host and tty (commits always trim)", () => {
    expect(profileDraftDirty(committed, committed.runner, "  box  ", " /dev/ttys001 ")).toBe(
      false,
    );
  });

  it("treats a whitespace-only binary path change as dirty (runner fields compare verbatim)", () => {
    expect(
      profileDraftDirty(
        committed,
        { binaryPath: " agentscan ", env: [] },
        "box",
        "/dev/ttys001",
      ),
    ).toBe(true);
  });

  it("flags host, tty, and env edits", () => {
    expect(profileDraftDirty(committed, committed.runner, "elsewhere", "/dev/ttys001")).toBe(
      true,
    );
    expect(profileDraftDirty(committed, committed.runner, "box", "/dev/ttys002")).toBe(true);
    expect(
      profileDraftDirty(
        committed,
        { binaryPath: "agentscan", env: [{ name: "A", value: "1" }] },
        "box",
        "/dev/ttys001",
      ),
    ).toBe(true);
  });

  it("compares an empty draft against a local profile's empty host/tty", () => {
    expect(profileDraftDirty(localProfile, localProfile.runner, "", "")).toBe(false);
  });
});

describe("toggleProfileOpen", () => {
  it("opens a closed folder and closes an open one", () => {
    const state = stateOf(["local"], localProfile, sshProfile("ssh-1", "box"));
    const opened = toggleProfileOpen(state, "ssh-1");
    expect(opened.openProfileIds).toEqual(["local", "ssh-1"]);
    expect(toggleProfileOpen(opened, "local").openProfileIds).toEqual(["ssh-1"]);
  });

  it("returns the same state for an unknown id", () => {
    const state = stateOf(["local"], localProfile);
    expect(toggleProfileOpen(state, "ghost")).toBe(state);
  });
});

describe("reorderProfile", () => {
  const base = () =>
    stateOf(
      ["local"],
      localProfile,
      sshProfile("ssh-1", "box"),
      sshProfile("ssh-2", "other"),
    );

  it("dragging down lands after the target", () => {
    const state = reorderProfile(base(), "local", "ssh-1");
    expect(state.profiles.map((profile) => profile.id)).toEqual(["ssh-1", "local", "ssh-2"]);
  });

  it("dragging up lands before the target", () => {
    const state = reorderProfile(base(), "ssh-2", "local");
    expect(state.profiles.map((profile) => profile.id)).toEqual(["ssh-2", "local", "ssh-1"]);
  });

  it("returns the same state for a no-op or unknown move", () => {
    const state = base();
    expect(reorderProfile(state, "local", "local")).toBe(state);
    expect(reorderProfile(state, "ghost", "local")).toBe(state);
    expect(reorderProfile(state, "local", "ghost")).toBe(state);
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

  it("drops a probed hostname that duplicates a sibling source's label", () => {
    const profile = sshProfile("ssh-1", "alias-box");
    const preflight: PreflightLabelSource = {
      runnerKey: runnerKeyForProfile(profile),
      preflight: { remoteHostLabel: "box" },
    };
    // Another source already reaches the same machine by its direct host: showing
    // the probed "box" twice would make the pick lists ambiguous.
    const siblings = [profile, sshProfile("ssh-2", "box")];
    expect(sourceLabel(profile, "mymac", preflight, siblings)).toBe("alias-box");
    // A probe matching the LOCAL source's hostname is dropped the same way.
    const localPreflight: PreflightLabelSource = {
      runnerKey: runnerKeyForProfile(profile),
      preflight: { remoteHostLabel: "mymac" },
    };
    expect(sourceLabel(profile, "mymac", localPreflight, [profile, localProfile])).toBe(
      "alias-box",
    );
  });

  it("drops a probed hostname when a sibling targets the same machine via another identity", () => {
    // "alice@box" and "bob@box" differ only by SSH identity — rewriting one to the
    // probed "box" would erase the only visible distinction between them.
    const profile = sshProfile("ssh-1", "alice@box");
    const preflight: PreflightLabelSource = {
      runnerKey: runnerKeyForProfile(profile),
      preflight: { remoteHostLabel: "box" },
    };
    const siblings = [profile, sshProfile("ssh-2", "bob@box")];
    expect(sourceLabel(profile, "mymac", preflight, siblings)).toBe("alice@box");
    // The machine part also matches across FQDN spellings of the same host.
    const fqdnSiblings = [profile, sshProfile("ssh-2", "bob@box.home.arpa")];
    expect(sourceLabel(profile, "mymac", preflight, fqdnSiblings)).toBe("alice@box");
  });

  it("keeps a probed hostname that is unique among sibling sources", () => {
    const profile = sshProfile("ssh-1", "user@box");
    const preflight: PreflightLabelSource = {
      runnerKey: runnerKeyForProfile(profile),
      preflight: { remoteHostLabel: "koopa" },
    };
    const siblings = [profile, localProfile, sshProfile("ssh-2", "other")];
    expect(sourceLabel(profile, "mymac", preflight, siblings)).toBe("koopa");
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

  it("falls back to the stored probed hostname when no live preflight matches", () => {
    // Only the active source is probed; a non-active folder keeps the short
    // label through the value persisted from its last successful preflight.
    const profile = { ...sshProfile("ssh-1", "auro@mander.home.arpa"), probedHost: "mander" };
    expect(sourceLabel(profile, "mymac")).toBe("mander");
    const otherRunner: PreflightLabelSource = {
      runnerKey: runnerKeyForProfile(sshProfile("ssh-2", "other")),
      preflight: { remoteHostLabel: "koopa" },
    };
    expect(sourceLabel(profile, "mymac", otherRunner)).toBe("mander");
  });

  it("prefers a matching live probe over the stored probed hostname", () => {
    const profile = { ...sshProfile("ssh-1", "user@box"), probedHost: "old-name" };
    const preflight: PreflightLabelSource = {
      runnerKey: runnerKeyForProfile(profile),
      preflight: { remoteHostLabel: "new-name" },
    };
    expect(sourceLabel(profile, "mymac", preflight)).toBe("new-name");
  });

  it("drops a stored probed hostname that collides with a sibling", () => {
    // Same machine via the sibling's configured target...
    const profile = { ...sshProfile("ssh-1", "alice@box"), probedHost: "box" };
    expect(sourceLabel(profile, "mymac", null, [profile, sshProfile("ssh-2", "bob@box")])).toBe(
      "alice@box",
    );
    // ...or via the sibling's own stored probe (configured host spelled differently).
    const sibling = { ...sshProfile("ssh-2", "alias-elsewhere"), probedHost: "box" };
    expect(sourceLabel(profile, "mymac", null, [profile, sibling])).toBe("alice@box");
  });
});

describe("recordProbedHost", () => {
  const state = (profiles: DesktopProfileConfig[]): ProfileState => ({
    activeProfileId: profiles[0].id,
    profiles,
    openProfileIds: [profiles[0].id],
  });

  it("stores the trimmed probe on the targeted ssh profile", () => {
    const target = sshProfile("ssh-1", "user@box");
    const before = state([localProfile, target]);
    const next = recordProbedHost(before, "ssh-1", "  mander  ", runnerKeyForProfile(target));
    expect(next.profiles[1]).toMatchObject({ id: "ssh-1", probedHost: "mander" });
  });

  it("returns the same state for no-ops", () => {
    const probed = { ...sshProfile("ssh-1", "user@box"), probedHost: "mander" };
    const key = runnerKeyForProfile(probed);
    const before = state([localProfile, probed]);
    // Unchanged value, empty value, unknown id, and a local target all skip.
    expect(recordProbedHost(before, "ssh-1", "mander", key)).toBe(before);
    expect(recordProbedHost(before, "ssh-1", "   ", key)).toBe(before);
    expect(recordProbedHost(before, "ssh-9", "mander", key)).toBe(before);
    expect(recordProbedHost(before, "local", "mander", runnerKeyForProfile(localProfile))).toBe(
      before,
    );
  });

  it("drops a probe whose runner no longer matches the profile", () => {
    // The probe raced a host edit: recording would label the NEW connection
    // with the OLD machine's hostname.
    const original = sshProfile("ssh-1", "user@box");
    const retargeted = state([localProfile, sshProfile("ssh-1", "user@other")]);
    expect(
      recordProbedHost(retargeted, "ssh-1", "box", runnerKeyForProfile(original)),
    ).toBe(retargeted);
  });
});

describe("probedHost persistence", () => {
  it("survives normalization and drops a blank value", () => {
    const stored = {
      activeProfileId: "ssh-1",
      profiles: [
        localProfile,
        { ...sshProfile("ssh-1", "user@box"), probedHost: " mander " },
        { ...sshProfile("ssh-2", "user@other"), probedHost: "   " },
      ],
      openProfileIds: ["ssh-1"],
    };
    const normalized = normalizeProfileState(stored);
    const first = normalized.profiles.find((profile) => profile.id === "ssh-1");
    const second = normalized.profiles.find((profile) => profile.id === "ssh-2");
    expect(first).toMatchObject({ probedHost: "mander" });
    expect(second).not.toHaveProperty("probedHost");
  });

  it("is cleared by a host edit and kept by a same-host edit", () => {
    const profile = { ...sshProfile("ssh-1", "user@box"), probedHost: "mander" };
    const retargeted = updateProfileSettings(profile, profile.runner, "user@other", "");
    expect(retargeted).not.toHaveProperty("probedHost");
    const sameHost = updateProfileSettings(profile, profile.runner, " user@box ", "/dev/ttys001");
    expect(sameHost).toMatchObject({ probedHost: "mander" });
  });
});
