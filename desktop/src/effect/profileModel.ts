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
  // an older version that allowed it) keeps only the first profile per trimmed SSH
  // host. Empty-host drafts collapse to the first too: with connection-derived
  // labels, several would render as identical cards the user can't tell apart, and
  // only one draft is ever needed to resume configuring.
  const seenHosts = new Set<string>();
  const profiles = mapped.filter((profile) => {
    if (profile.kind !== "ssh") {
      return true;
    }
    if (seenHosts.has(profile.host)) {
      return false;
    }
    seenHosts.add(profile.host);
    return true;
  });

  if (!profiles.some((profile) => profile.kind === "local")) {
    profiles.unshift(defaultProfileState(fallbackRunner).profiles[0]);
  }

  const fallbackProfile = profiles.find(isRunnableProfile) ?? profiles[0];
  const activeProfileId =
    typeof value.activeProfileId === "string" &&
    profiles.some((profile) => profile.id === value.activeProfileId && isRunnableProfile(profile))
      ? value.activeProfileId
      : fallbackProfile.id;

  // Open folders, filtered to surviving profiles. A state persisted before the
  // folder UI has no openProfileIds: the previously-active profile starts open so
  // the upgrade keeps exactly the old one-subscription behavior.
  const openSource = Array.isArray(value.openProfileIds)
    ? value.openProfileIds.filter((id): id is string => typeof id === "string")
    : [activeProfileId];
  const openProfileIds = [
    ...new Set(openSource.filter((id) => profiles.some((profile) => profile.id === id))),
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
    return {
      ...profile,
      host: sshHost.trim(),
      clientTty: sshClientTty.trim(),
      runner: normalizedRunner,
      enabled: true,
    };
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

export function runnerSettingsEqual(left: RunnerSettings, right: RunnerSettings): boolean {
  if (left.binaryPath !== right.binaryPath || left.env.length !== right.env.length) {
    return false;
  }

  return left.env.every(
    (variable, index) =>
      variable.name === right.env[index]?.name && variable.value === right.env[index]?.value,
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

// Display label for an agentscan source, derived from its connection: the local
// machine keyed by its hostname, a remote keyed by its SSH host (each falling back
// to a generic label when its host isn't known). A remote upgrades to the hostname
// probed by its preflight, but only when that preflight's runnerKey matches this
// exact profile — a label must never come from a stale (different-runner) probe —
// and only when the probed name wouldn't duplicate a sibling source's label (an
// alias and a direct entry can reach the same machine; the configured connection
// string is the honest disambiguator in lists the user picks from).
export function sourceLabel(
  profile: DesktopProfileConfig,
  localHostLabel: string,
  preflight?: PreflightLabelSource | null,
  siblings?: ReadonlyArray<DesktopProfileConfig>,
): string {
  if (profile.kind === "ssh") {
    const probed =
      preflight && preflight.runnerKey === runnerKeyForProfile(profile)
        ? preflight.preflight?.remoteHostLabel
        : null;
    const ambiguous =
      !!probed &&
      (siblings ?? []).some((other) => {
        if (other.id === profile.id) {
          return false;
        }
        return other.kind === "ssh"
          ? other.host.trim() === probed
          : (localHostLabel || "agentscan") === probed;
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
