// Pure profile/settings model for the desktop shell. No React, no Effect, no
// global `window` access — persistence is parameterized over a read/write pair so
// the Profiles Effect.Service (and its vitest proof) can drive it over an injected
// storage boundary while App.tsx keeps using the same derivations for rendering.
//
// These types mirror the Rust contracts in src-tauri/src/lib.rs (DesktopRunnerSettings,
// DesktopProfile) and were previously inlined in App.tsx.

export type ProfileKind = "local" | "ssh";

export type DesktopProfile = {
  id: string;
  name: string;
  kind: ProfileKind;
};

export type AgentscanPreflight = {
  binary: string;
  ok: boolean;
  version: string | null;
  error: string | null;
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
};

export type DesktopProfileConfig = LocalProfileConfig | SshProfileConfig;

export type LocalProfileConfig = {
  id: string;
  name: string;
  kind: "local";
  runner: RunnerSettings;
};

export type SshProfileConfig = {
  id: string;
  name: string;
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
        name: "Default",
        kind: "local",
        runner: normalizeRunnerSettings(runner),
      },
    ],
  };
}

// `fallbackRunner` seeds the implicit local profile when a persisted state has none
// (previously read straight from localStorage; now passed in to keep this pure).
export function normalizeProfileState(
  value: Partial<ProfileState>,
  fallbackRunner: RunnerSettings = emptyRunnerSettings(),
): ProfileState {
  const profiles = Array.isArray(value.profiles)
    ? value.profiles.map(normalizeProfile).filter((profile): profile is DesktopProfileConfig => profile !== null)
    : [];

  if (!profiles.some((profile) => profile.kind === "local")) {
    profiles.unshift(defaultProfileState(fallbackRunner).profiles[0]);
  }

  const fallbackProfile = profiles.find(isRunnableProfile) ?? profiles[0];
  const activeProfileId =
    typeof value.activeProfileId === "string" &&
    profiles.some((profile) => profile.id === value.activeProfileId && isRunnableProfile(profile))
      ? value.activeProfileId
      : fallbackProfile.id;

  return { activeProfileId, profiles };
}

export function normalizeProfile(value: unknown): DesktopProfileConfig | null {
  if (!value || typeof value !== "object") {
    return null;
  }

  const profile = value as Partial<DesktopProfileConfig>;
  const id = typeof profile.id === "string" && profile.id.trim() ? profile.id.trim() : "";
  const name = typeof profile.name === "string" && profile.name.trim() ? profile.name.trim() : "";
  const runner = normalizeRunnerSettings(profile.runner ?? emptyRunnerSettings());

  if (profile.kind === "local") {
    return {
      id: id || LOCAL_PROFILE_ID,
      name: name || "Default",
      kind: "local",
      runner,
    };
  }

  if (profile.kind === "ssh") {
    return {
      id: id || `ssh-${Date.now()}`,
      name: name || "Remote",
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

export function updateProfileSettingsById(
  state: ProfileState,
  id: string,
  name: string,
  runner: RunnerSettings,
  sshHost: string,
  sshClientTty: string,
): ProfileState {
  // A missing id maps to a no-op, which cleanly handles applying an edit whose target
  // profile was deleted elsewhere (the edit is simply dropped onto the latest state).
  return {
    ...state,
    profiles: state.profiles.map((profile) =>
      profile.id === id
        ? updateProfileSettings(profile, name, runner, sshHost, sshClientTty)
        : profile,
    ),
  };
}

export function updateProfileSettings(
  profile: DesktopProfileConfig,
  name: string,
  runner: RunnerSettings,
  sshHost: string,
  sshClientTty: string,
): DesktopProfileConfig {
  const normalizedRunner = normalizeRunnerSettings(runner);
  const normalizedName = name.trim() || profile.name;

  if (profile.kind === "ssh") {
    return {
      ...profile,
      name: normalizedName,
      host: sshHost.trim(),
      clientTty: sshClientTty.trim(),
      runner: normalizedRunner,
      enabled: true,
    };
  }

  return { ...profile, name: normalizedName, runner: normalizedRunner };
}

export function profileSummary(profile: DesktopProfileConfig): DesktopProfile {
  return {
    id: profile.id,
    name: profile.name,
    kind: profile.kind,
  };
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

export function validateProfileDraft(
  profile: DesktopProfileConfig,
  name: string,
  runner: RunnerSettings,
  sshHost: string,
  sshClientTty: string,
): DraftValidation {
  const errors: string[] = [];

  if (!name.trim()) {
    errors.push("Profile name is required.");
  }

  if (runner.binaryPath.includes("\0")) {
    errors.push("agentscan binary cannot contain a null byte.");
  }

  if (profile.kind === "ssh") {
    const host = sshHost.trim();
    if (!host) {
      errors.push("SSH host is required.");
    } else if (host.startsWith("-") || /\s/.test(host) || host.includes("\0")) {
      errors.push("SSH host must be a single host alias and cannot start with '-'.");
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

export function nextRemoteProfileName(profiles: DesktopProfileConfig[]): string {
  const remoteCount = profiles.filter((profile) => profile.kind === "ssh").length;
  return remoteCount === 0 ? "Remote" : `Remote ${remoteCount + 1}`;
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
