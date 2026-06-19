// Pure profile/settings model for the desktop shell. No React, no Effect, no
// global `window` access — persistence is parameterized over a read/write pair so
// the Profiles Effect.Service (and its vitest proof) can drive it over an injected
// storage boundary while App.tsx keeps using the same derivations for rendering.
//
// These types mirror the Rust contracts in src-tauri/src/lib.rs (DesktopRunnerSettings)
// and were previously inlined in App.tsx.

export type ProfileKind = "local" | "ssh";

export type AgentscanPreflight = {
  binary: string;
  ok: boolean;
  version: string | null;
  error: string | null;
  // Absolute remote path the dock can offer as a one-click fix when a remote
  // preflight fails because agentscan isn't on the SSH PATH but the user's shell
  // can find it. Null for success, local runners, and unresolvable failures.
  suggestedBinaryPath: string | null;
  // The remote machine's short hostname, probed in the same SSH exec as the
  // version check. Null for local runners, failures, and when the remote
  // hostname is unavailable.
  remoteHostLabel: string | null;
};

export type EnvironmentVariable = {
  name: string;
  value: string;
};

export type RunnerSettings = {
  binaryPath: string;
  env: EnvironmentVariable[];
};

// The wire payload passed verbatim to the Rust commands (tagged union matching
// DesktopRunnerSettings on the backend). App.tsx builds it from the active profile.
export type DesktopRunnerSettings =
  | ({ kind: "local" } & RunnerSettings)
  | ({ kind: "ssh"; host: string; clientTty: string | null } & RunnerSettings);

export type ProfileState = {
  activeProfileId: string;
  profiles: DesktopProfileConfig[];
  // Folder open state for the dock's vertical strip: a source's folder is open
  // (live subscription armed) iff its profile id is listed here. Source order is
  // the profiles array order, which also decides keybind ownership.
  openProfileIds: string[];
};

export type DesktopProfileConfig = LocalProfileConfig | SshProfileConfig;

export type LocalProfileConfig = {
  id: string;
  kind: "local";
  runner: RunnerSettings;
};

export type SshProfileConfig = {
  id: string;
  kind: "ssh";
  host: string;
  clientTty: string;
  runner: RunnerSettings;
  enabled: boolean;
  // Short hostname probed by the last successful preflight of this connection.
  // Display-only label enrichment (never part of the runner identity): probes
  // are event-driven (the active source's preflight, plus a one-shot background
  // probe when a never-probed source comes online), so persisting the result is
  // what keeps every folder and the next launch on the short label. Cleared
  // when the host is edited — the probe described the old machine.
  probedHost?: string;
};

export type DraftValidation = {
  errors: string[];
};

export const LOCAL_PROFILE_ID = "local";
export const SETTINGS_STORAGE_KEY = "agentscan.desktop.localRunnerSettings";
export const PROFILES_STORAGE_KEY = "agentscan.desktop.profiles";

// The minimal synchronous storage surface the model needs. The Profiles service
// supplies these from the PrefsBridge (window.localStorage in the app, an in-memory
// map in tests); App.tsx never touches localStorage for profiles after the migration.
export type StorageRead = (key: string) => string | null;
export type StorageWrite = (key: string, value: string) => void;

export function emptyRunnerSettings(): RunnerSettings {
  return { binaryPath: "", env: [] };
}

export function normalizeRunnerSettings(settings: Partial<RunnerSettings>): RunnerSettings {
  const env = Array.isArray(settings.env)
    ? settings.env
        .map((variable) => ({
          name: String(variable.name ?? "").trim(),
          value: String(variable.value ?? ""),
        }))
        .filter((variable) => variable.name.length > 0)
    : [];

  return {
    binaryPath: String(settings.binaryPath ?? "").trim(),
    env,
  };
}

export function defaultProfileState(runner: RunnerSettings = emptyRunnerSettings()): ProfileState {
  return {
    activeProfileId: LOCAL_PROFILE_ID,
    profiles: [
      {
        id: LOCAL_PROFILE_ID,
        kind: "local",
        runner: normalizeRunnerSettings(runner),
      },
    ],
    openProfileIds: [LOCAL_PROFILE_ID],
  };
}

// `fallbackRunner` seeds the implicit local profile when a persisted state has none
// (previously read straight from localStorage; now passed in to keep this pure).
export function normalizeProfileState(
  value: Partial<ProfileState>,
  fallbackRunner: RunnerSettings = emptyRunnerSettings(),
): ProfileState {
  const mapped = Array.isArray(value.profiles)
    ? value.profiles.map(normalizeProfile).filter((profile): profile is DesktopProfileConfig => profile !== null)
    : [];

  // A source's identity IS its connection, so a persisted state (possibly written by
  // an older version that allowed it) keeps only one profile per trimmed SSH host:
  // the first RUNNABLE one, falling back to the first at all — keeping a disabled
  // duplicate over an enabled one would make the connection vanish from the dock.
  // Empty-host drafts collapse the same way: with connection-derived labels,
  // several would render as identical cards the user can't tell apart, and only
  // one draft is ever needed to resume configuring. References to a dropped
  // duplicate (the active id, open ids) remap to the surviving profile of the same
  // connection — the user's selection was the connection, not the duplicate row.
  const sshGroups = new Map<string, DesktopProfileConfig[]>();
  for (const profile of mapped) {
    if (profile.kind !== "ssh") {
      continue;
    }
    const group = sshGroups.get(profile.host);
    if (group) {
      group.push(profile);
    } else {
      sshGroups.set(profile.host, [profile]);
    }
  }
  const remap = new Map<string, string>();
  const survivors = new Set<string>();
  for (const group of sshGroups.values()) {
    const survivor = group.find(isRunnableProfile) ?? group[0];
    survivors.add(survivor.id);
    for (const profile of group) {
      if (profile.id !== survivor.id) {
        remap.set(profile.id, survivor.id);
      }
    }
  }
  const profiles = mapped.filter(
    (profile) => profile.kind !== "ssh" || survivors.has(profile.id),
  );

  if (!profiles.some((profile) => profile.kind === "local")) {
    profiles.unshift(defaultProfileState(fallbackRunner).profiles[0]);
  }

  const fallbackProfile = profiles.find(isRunnableProfile) ?? profiles[0];
  const requestedActiveId =
    typeof value.activeProfileId === "string"
      ? (remap.get(value.activeProfileId) ?? value.activeProfileId)
      : undefined;
  const activeProfileId =
    requestedActiveId !== undefined &&
    profiles.some((profile) => profile.id === requestedActiveId && isRunnableProfile(profile))
      ? requestedActiveId
      : fallbackProfile.id;

  // Open folders, remapped to dedupe survivors and filtered to runnable surviving
  // profiles. Disabled sources stay persisted but cannot own a live subscription
  // or become candidates for open-folder fallback behavior until re-enabled.
  // A state persisted before the folder UI has no openProfileIds: the previously-
  // active profile starts open so the upgrade keeps exactly the old
  // one-subscription behavior.
  const openSource = Array.isArray(value.openProfileIds)
    ? value.openProfileIds
        .filter((id): id is string => typeof id === "string")
        .map((id) => remap.get(id) ?? id)
    : [activeProfileId];
  const openProfileIds = [
    ...new Set(
      openSource.filter((id) =>
        profiles.some((profile) => profile.id === id && isRunnableProfile(profile)),
      ),
    ),
  ];

  return { activeProfileId, profiles, openProfileIds };
}

export function normalizeProfile(value: unknown): DesktopProfileConfig | null {
  if (!value || typeof value !== "object") {
    return null;
  }

  // Rebuilding the profile field-by-field also strips the user-editable `name`
  // older versions persisted; labels are derived from the connection now.
  const profile = value as Partial<DesktopProfileConfig>;
  const id = typeof profile.id === "string" && profile.id.trim() ? profile.id.trim() : "";
  const runner = normalizeRunnerSettings(profile.runner ?? emptyRunnerSettings());

  if (profile.kind === "local") {
    return {
      id: id || LOCAL_PROFILE_ID,
      kind: "local",
      runner,
    };
  }

  if (profile.kind === "ssh") {
    const probedHost = typeof profile.probedHost === "string" ? profile.probedHost.trim() : "";
    return {
      id: id || `ssh-${Date.now()}`,
      kind: "ssh",
      host: typeof profile.host === "string" ? profile.host.trim() : "",
      clientTty: typeof profile.clientTty === "string" ? profile.clientTty.trim() : "",
      runner,
      // Default to enabled so profiles persisted before the `enabled` field (or
      // partial profiles missing it) remain selectable; only an explicit false
      // disables a profile.
      enabled: profile.enabled !== false,
      ...(probedHost ? { probedHost } : {}),
    };
  }

  return null;
}

export function getActiveProfile(state: ProfileState): DesktopProfileConfig {
  return (
    state.profiles.find(
      (profile) => profile.id === state.activeProfileId && isRunnableProfile(profile),
    ) ??
    state.profiles.find(isRunnableProfile) ??
    state.profiles[0]
  );
}

export function isRunnableProfile(profile: DesktopProfileConfig): boolean {
  return profile.kind === "local" || profile.enabled;
}

// A source the dock can render as a host folder (and subscribe to): the local
// runner, or an enabled remote with a configured connection. A still-unconfigured
// (empty-host) remote lives only in Settings until it gets a host.
export function isFolderProfile(profile: DesktopProfileConfig): boolean {
  return (
    isRunnableProfile(profile) && (profile.kind === "local" || profile.host.trim().length > 0)
  );
}

export function folderProfiles(state: ProfileState): DesktopProfileConfig[] {
  return state.profiles.filter(isFolderProfile);
}

// Row keybinds (Ctrl+<key>) are owned by exactly one source: the FIRST OPEN
// folder in the user's source order. Null when no folder is open.
export function keybindOwnerId(state: ProfileState): string | null {
  return (
    state.profiles.find(
      (profile) => isFolderProfile(profile) && state.openProfileIds.includes(profile.id),
    )?.id ?? null
  );
}

// One folder-eligible source as the dock consumes it: the profile plus its
// runner identity, open state, keybind ownership, and committed-profile
// validity (the arm gate for non-active sources, whose preflight is never
// probed).
export type LiveSource = {
  profile: DesktopProfileConfig;
  runnerKey: string;
  settings: DesktopRunnerSettings;
  isOpen: boolean;
  isOwner: boolean;
  valid: boolean;
};

// The folder-eligible sources in user order. isOwner marks exactly the first
// OPEN folder (keybindOwnerId's rule); valid judges each profile's COMMITTED
// values, never form drafts.
export function liveSourcesFor(state: ProfileState): LiveSource[] {
  const ownerId = keybindOwnerId(state);
  return folderProfiles(state).map((profile) => ({
    profile,
    runnerKey: runnerKeyForProfile(profile),
    settings: runnerSettingsForProfile(profile),
    isOpen: state.openProfileIds.includes(profile.id),
    isOwner: profile.id === ownerId,
    valid: committedProfileValidation(profile, state.profiles).errors.length === 0,
  }));
}

// Open/close one source's folder. Returns the SAME state for an unknown id so
// callers can skip a no-op commit.
export function toggleProfileOpen(state: ProfileState, id: string): ProfileState {
  if (!state.profiles.some((profile) => profile.id === id)) {
    return state;
  }
  return {
    ...state,
    openProfileIds: state.openProfileIds.includes(id)
      ? state.openProfileIds.filter((openId) => openId !== id)
      : [...state.openProfileIds, id],
  };
}

// Enable/disable one SSH source. Disabled sources remain persisted but leave the
// open/live set; re-enabling opens the source again so the footer checkbox acts
// like "show this source in the dock" without re-entering connection settings.
export function setProfileEnabled(
  state: ProfileState,
  id: string,
  enabled: boolean,
): ProfileState {
  const index = state.profiles.findIndex((profile) => profile.id === id);
  const profile = index === -1 ? undefined : state.profiles[index];
  if (!profile || profile.kind !== "ssh" || profile.enabled === enabled) {
    return state;
  }

  const profiles = [...state.profiles];
  profiles[index] = { ...profile, enabled };
  const openProfileIds = enabled
    ? state.openProfileIds.includes(id)
      ? state.openProfileIds
      : [...state.openProfileIds, id]
    : state.openProfileIds.filter((openId) => openId !== id);
  return normalizeProfileState({
    activeProfileId: state.activeProfileId,
    profiles,
    openProfileIds,
  });
}

// Move the dragged profile onto the target's position (after it when dragging
// down, before it when dragging up — the usual list-reorder feel). Keybind
// ownership is derived from this order. Returns the SAME state when nothing moves.
export function reorderProfile(state: ProfileState, id: string, targetId: string): ProfileState {
  const fromIndex = state.profiles.findIndex((profile) => profile.id === id);
  const targetIndex = state.profiles.findIndex((profile) => profile.id === targetId);
  if (fromIndex < 0 || targetIndex < 0 || fromIndex === targetIndex) {
    return state;
  }
  const moved = state.profiles[fromIndex];
  const profiles = state.profiles.filter((profile) => profile.id !== id);
  // Inserting at the target's ORIGINAL index lands after it when dragging down
  // (the removal shifted it left by one) and before it when dragging up.
  profiles.splice(targetIndex, 0, moved);
  return { ...state, profiles };
}

// Record the hostname a successful preflight probed for one SSH source.
// `runnerKey` is the identity of the runner that was actually probed: a probe
// is async, and the profile may have been retargeted while it was in flight —
// recording then would write the OLD machine's hostname onto the NEW
// connection (updateProfileSettings just cleared it for exactly that reason),
// so a key mismatch drops the stale result. Returns the SAME state when
// nothing changes (unknown id, non-ssh profile, stale runner, empty or
// already-stored value) so callers can skip a no-op commit.
export function recordProbedHost(
  state: ProfileState,
  id: string,
  probedHost: string,
  runnerKey: string,
): ProfileState {
  const trimmed = probedHost.trim();
  if (!trimmed) {
    return state;
  }
  const index = state.profiles.findIndex((profile) => profile.id === id);
  const profile = index === -1 ? undefined : state.profiles[index];
  if (
    !profile ||
    profile.kind !== "ssh" ||
    runnerKeyForProfile(profile) !== runnerKey ||
    profile.probedHost === trimmed
  ) {
    return state;
  }
  const profiles = [...state.profiles];
  profiles[index] = { ...profile, probedHost: trimmed };
  return { ...state, profiles };
}

export function updateProfileSettingsById(
  state: ProfileState,
  id: string,
  runner: RunnerSettings,
  sshHost: string,
  sshClientTty: string,
): ProfileState {
  // A missing id maps to a no-op, which cleanly handles applying an edit whose target
  // profile was deleted elsewhere (the edit is simply dropped onto the latest state).
  return {
    ...state,
    profiles: state.profiles.map((profile) =>
      profile.id === id ? updateProfileSettings(profile, runner, sshHost, sshClientTty) : profile,
    ),
  };
}

export function updateProfileSettings(
  profile: DesktopProfileConfig,
  runner: RunnerSettings,
  sshHost: string,
  sshClientTty: string,
): DesktopProfileConfig {
  const normalizedRunner = normalizeRunnerSettings(runner);

  if (profile.kind === "ssh") {
    const host = sshHost.trim();
    const next: SshProfileConfig = {
      ...profile,
      host,
      clientTty: sshClientTty.trim(),
      runner: normalizedRunner,
      enabled: true,
    };
    if (host !== profile.host) {
      // The stored probe described the old target; a retargeted host must not
      // wear it. Cleared even when only the SSH identity changed (alice@box ->
      // bob@box): machine-part equality is a heuristic (ssh config can resolve
      // equal-looking targets to different machines), and heuristics here only
      // ever SUPPRESS a probed label, never retain one. The edited profile is
      // the settings-active one, so its runnerKey change re-fires preflight and
      // re-records the label one round-trip later.
      delete next.probedHost;
    }
    return next;
  }

  return { ...profile, runner: normalizedRunner };
}

export function runnerSummary(settings: RunnerSettings): string {
  return settings.binaryPath.trim() || "auto-detected agentscan";
}

// Stable string identity of a profile's full runner configuration. Used to
// invalidate resolved preflight/picker state when the target changes, including
// same-profile settings edits (which keep the same id).
export function runnerKeyForProfile(profile: DesktopProfileConfig): string {
  return JSON.stringify(runnerSettingsForProfile(profile));
}

export function runnerSettingsForProfile(profile: DesktopProfileConfig): DesktopRunnerSettings {
  if (profile.kind === "ssh") {
    return {
      kind: "ssh",
      host: profile.host,
      clientTty: profile.clientTty.trim() || null,
      ...profile.runner,
    };
  }

  return {
    kind: "local",
    ...profile.runner,
  };
}

export function commandPrefix(profile: DesktopProfileConfig): string {
  const binary = profile.runner.binaryPath.trim() || "agentscan";

  if (profile.kind === "ssh") {
    return `ssh ${profile.host || "<host>"} -- ${binary}`;
  }

  return binary;
}

export function focusCommandLabel(profile: DesktopProfileConfig, paneId: string): string {
  const base = `${commandPrefix(profile)} focus`;
  if (profile.kind === "ssh" && profile.clientTty.trim()) {
    return `${base} --client-tty ${profile.clientTty.trim()} ${paneId}`;
  }

  return `${base} ${paneId}`;
}

export function profileKindLabel(profile: DesktopProfileConfig): string {
  return profile.kind === "ssh" ? "SSH" : "Local";
}

// The connection is the source's identity, so two profiles can't share a host.
// Shared by form validation and commit-time re-checks: load-time dedupe drops a
// persisted duplicate, so a commit that lets one through silently deletes a source.
export function sshHostCollides(
  profiles: DesktopProfileConfig[],
  id: string,
  sshHost: string,
): boolean {
  const host = sshHost.trim();
  return (
    host.length > 0 &&
    profiles.some((other) => other.id !== id && other.kind === "ssh" && other.host.trim() === host)
  );
}

export function validateProfileDraft(
  profile: DesktopProfileConfig,
  runner: RunnerSettings,
  sshHost: string,
  sshClientTty: string,
  profiles: DesktopProfileConfig[],
): DraftValidation {
  const errors: string[] = [];

  if (runner.binaryPath.includes("\0")) {
    errors.push("agentscan binary cannot contain a null byte.");
  }

  if (profile.kind === "ssh") {
    const host = sshHost.trim();
    if (!host) {
      errors.push("SSH host is required.");
    } else if (host.startsWith("-") || /\s/.test(host) || host.includes("\0")) {
      errors.push("SSH host must be a single host alias and cannot start with '-'.");
    } else if (sshHostCollides(profiles, profile.id, host)) {
      errors.push("A source for this connection already exists.");
    }

    const clientTty = sshClientTty.trim();
    if (clientTty && (/\s/.test(clientTty) || clientTty.includes("\0"))) {
      errors.push("Remote client tty must be a single tty path.");
    }
  }

  const seenNames = new Set<string>();
  runner.env.forEach((variable, index) => {
    const variableName = variable.name.trim();
    if (!variableName) {
      errors.push(`Environment row ${index + 1} needs a name.`);
      return;
    }

    // Must be a POSIX shell identifier: names are interpolated unquoted into
    // the remote SSH command, so spaces/hyphens/metacharacters are rejected.
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(variableName)) {
      errors.push(`Environment row ${index + 1} name must be a valid shell identifier.`);
      return;
    }

    if (seenNames.has(variableName)) {
      errors.push(`Environment variable ${variableName} is duplicated.`);
    }
    seenNames.add(variableName);
  });

  return { errors };
}

// Validation of a profile's COMMITTED values (never form drafts): the draft
// validator fed with what's already stored, so the picker gate and the
// preflight short-circuit judge the profile as persisted.
export function committedProfileValidation(
  profile: DesktopProfileConfig,
  profiles: DesktopProfileConfig[],
): DraftValidation {
  return validateProfileDraft(
    profile,
    profile.runner,
    profile.kind === "ssh" ? profile.host : "",
    profile.kind === "ssh" ? profile.clientTty : "",
    profiles,
  );
}

export function runnerSettingsEqual(left: RunnerSettings, right: RunnerSettings): boolean {
  if (left.binaryPath !== right.binaryPath || left.env.length !== right.env.length) {
    return false;
  }

  return left.env.every(
    (variable, index) =>
      variable.name === right.env[index]?.name && variable.value === right.env[index]?.value,
  );
}

// Whether the settings form drafts differ from the active profile's committed
// values. Host/tty compare TRIMMED — commits always trim them, so a draft
// differing only by surrounding whitespace is not a real change — while runner
// fields (binary path, env) compare verbatim.
export function profileDraftDirty(
  activeProfile: DesktopProfileConfig,
  settingsDraft: RunnerSettings,
  sshHostDraft: string,
  sshClientTtyDraft: string,
): boolean {
  return (
    sshHostDraft.trim() !== (activeProfile.kind === "ssh" ? activeProfile.host : "") ||
    sshClientTtyDraft.trim() !== (activeProfile.kind === "ssh" ? activeProfile.clientTty : "") ||
    !runnerSettingsEqual(settingsDraft, activeProfile.runner)
  );
}

export function newProfileId(prefix: string): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return `${prefix}-${crypto.randomUUID()}`;
  }

  return `${prefix}-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

// A resolved preflight as the label derivation needs it: the probed hostname keyed
// by the runner it actually probed. Structural so both the dock's PreflightState
// ("ready" arm) and the settings window's SyncedPreflight satisfy it.
export type PreflightLabelSource = {
  runnerKey: string;
  preflight: Pick<AgentscanPreflight, "remoteHostLabel"> | null;
};

// The machine part of a configured SSH target, for comparing against a probed
// short hostname: "alice@box.lan", "bob@box", and "box" all reach machine "box".
function sshHostMachine(host: string): string {
  const target = host.trim();
  const machine = target.slice(target.lastIndexOf("@") + 1);
  const dot = machine.indexOf(".");
  return dot === -1 ? machine : machine.slice(0, dot);
}

// Display label for an agentscan source, derived from its connection: the local
// machine keyed by its hostname, a remote keyed by its SSH host (each falling back
// to a generic label when its host isn't known). A remote upgrades to a probed
// hostname — the live preflight's when its runnerKey matches this exact profile (a
// label must never come from a stale, different-runner probe), else the one
// persisted from this connection's last successful preflight — and only when no
// sibling source reaches the same machine (compared by its own stored probe and
// the machine part of its configured target: "alice@box" and "bob@box" differ only
// by SSH identity, which the probed "box" would erase; the configured connection
// string is the honest disambiguator in lists the user picks from).
export function sourceLabel(
  profile: DesktopProfileConfig,
  localHostLabel: string,
  preflight?: PreflightLabelSource | null,
  siblings?: ReadonlyArray<DesktopProfileConfig>,
): string {
  if (profile.kind === "ssh") {
    const live =
      preflight && preflight.runnerKey === runnerKeyForProfile(profile)
        ? preflight.preflight?.remoteHostLabel
        : null;
    // A matching probe with NO hostname (a failed preflight, or `hostname`
    // unavailable on the remote) is absence of evidence, not contradiction:
    // the stored value still describes this exact unchanged connection, and
    // deferring to it keeps the label stable across transient probe gaps. A
    // contradicting probe replaces it here (live wins) and is then
    // re-recorded; editing the connection clears it (updateProfileSettings).
    const probed = live || profile.probedHost || null;
    const ambiguous =
      !!probed &&
      (siblings ?? []).some((other) => {
        if (other.id === profile.id) {
          return false;
        }
        if (other.kind !== "ssh") {
          return (localHostLabel || "agentscan") === probed;
        }
        return other.probedHost === probed || sshHostMachine(other.host) === probed;
      });
    return (ambiguous ? null : probed) || profile.host.trim() || "Remote";
  }
  return localHostLabel || "agentscan";
}

// Read the local profile's runner settings from storage (the `agentscan.desktop.
// localRunnerSettings` mirror), used to seed an implicit local profile.
export function loadRunnerSettings(read: StorageRead): RunnerSettings {
  try {
    const value = read(SETTINGS_STORAGE_KEY);
    if (!value) {
      return emptyRunnerSettings();
    }

    return normalizeRunnerSettings(JSON.parse(value) as Partial<RunnerSettings>);
  } catch {
    return emptyRunnerSettings();
  }
}

// Load + normalize the full profile state from storage, falling back to a default
// single-local-profile state on a miss or parse error.
export function loadProfileState(read: StorageRead): ProfileState {
  const fallbackRunner = loadRunnerSettings(read);
  try {
    const value = read(PROFILES_STORAGE_KEY);
    if (!value) {
      return defaultProfileState(fallbackRunner);
    }

    return normalizeProfileState(JSON.parse(value) as Partial<ProfileState>, fallbackRunner);
  } catch {
    return defaultProfileState(fallbackRunner);
  }
}

// Persist the profile state, mirroring the local profile's runner into the
// `localRunnerSettings` key so a fresh install (no profiles key) still seeds it.
export function storeProfileState(write: StorageWrite, state: ProfileState): void {
  write(PROFILES_STORAGE_KEY, JSON.stringify(state));
  const localProfile = state.profiles.find((profile) => profile.kind === "local");
  if (localProfile) {
    write(SETTINGS_STORAGE_KEY, JSON.stringify(localProfile.runner));
  }
}
