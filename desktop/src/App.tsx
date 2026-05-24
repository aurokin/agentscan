import { useEffect, useMemo, useState, type SetStateAction } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { register, unregister } from "@tauri-apps/plugin-global-shortcut";

const PICKER_HOTKEY = "CommandOrControl+Shift+A";
const LIVE_PICKER_EVENT = "agentscan-live-picker";
const SETTINGS_STORAGE_KEY = "agentscan.desktop.localRunnerSettings";
const PROFILES_STORAGE_KEY = "agentscan.desktop.profiles";
const DEBUG_LOG_LIMIT = 80;
const LOCAL_PROFILE_ID = "local";

let hotkeyOperationQueue = Promise.resolve();
let liveOperationQueue = Promise.resolve();
let windowOperationQueue = Promise.resolve();

type DesktopProfile = {
  id: string;
  name: string;
  kind: ProfileKind;
};

type ProfileKind = "local" | "ssh";

type AgentscanPreflight = {
  binary: string;
  ok: boolean;
  version: string | null;
  error: string | null;
};

type RunnerSettings = {
  binaryPath: string;
  env: EnvironmentVariable[];
};

type DesktopRunnerSettings =
  | ({ kind: "local" } & RunnerSettings)
  | ({ kind: "ssh"; host: string; clientTty: string | null } & RunnerSettings);

type ProfileState = {
  activeProfileId: string;
  profiles: DesktopProfileConfig[];
};

type DesktopProfileConfig = LocalProfileConfig | SshProfileConfig;

type LocalProfileConfig = {
  id: string;
  name: string;
  kind: "local";
  runner: RunnerSettings;
};

type SshProfileConfig = {
  id: string;
  name: string;
  kind: "ssh";
  host: string;
  clientTty: string;
  runner: RunnerSettings;
  enabled: boolean;
};

type EnvironmentVariable = {
  name: string;
  value: string;
};

type DebugEntry = {
  id: number;
  time: string;
  kind: "command" | "stream" | "settings";
  label: string;
  detail: string;
};

type PickerRow = {
  key: string;
  pane_id: string;
  provider: string | null;
  status: { kind: string };
  display_label: string;
  location_tag: string;
};

type LiveSnapshotSummary = {
  paneCount: number;
  generatedAt: string | null;
  sourceKind: string | null;
};

type LivePickerEvent =
  | { kind: "connecting"; message: string }
  | { kind: "reconnecting"; message: string; diagnostics: unknown | null }
  | { kind: "rows"; rows: PickerRow[]; snapshot: LiveSnapshotSummary }
  | { kind: "offline"; message: string; retrying: boolean; diagnostics: unknown | null }
  | { kind: "shutdown"; message: string }
  | { kind: "fatal"; message: string; diagnostics: unknown | null };

type LiveConnectionState =
  | { status: "connecting"; message: string }
  | { status: "online"; message: string; snapshot: LiveSnapshotSummary }
  | { status: "reconnecting"; message: string; diagnostics: unknown | null }
  | { status: "offline"; message: string; retrying: boolean; diagnostics: unknown | null }
  | { status: "shutdown"; message: string }
  | { status: "fatal"; message: string; diagnostics: unknown | null };

type LoadState =
  | { status: "loading" }
  | {
      status: "ready";
      profiles: DesktopProfile[];
      preflight: AgentscanPreflight;
      picker: PickerState;
    }
  | { status: "failed"; message: string };

type PickerState =
  | { status: "loading" }
  | { status: "ready"; rows: PickerRow[] }
  | { status: "failed"; message: string };

type PickerActivation =
  | { status: "idle" }
  | { status: "running"; paneId: string }
  | { status: "failed"; message: string };

type DraftValidation = {
  errors: string[];
};

function App() {
  const [state, setState] = useState<LoadState>({ status: "loading" });
  const [profileState, setProfileState] = useState<ProfileState>(() => loadStoredProfiles());
  const activeProfile = useMemo(() => getActiveProfile(profileState), [profileState]);
  const runnerSettings = useMemo(() => runnerSettingsForProfile(activeProfile), [activeProfile]);
  const [profileNameDraft, setProfileNameDraft] = useState(() =>
    getActiveProfile(loadStoredProfiles()).name,
  );
  const [settingsDraft, setSettingsDraft] = useState<RunnerSettings>(() =>
    getActiveProfile(loadStoredProfiles()).runner,
  );
  const [sshHostDraft, setSshHostDraft] = useState(() => {
    const profile = getActiveProfile(loadStoredProfiles());
    return profile.kind === "ssh" ? profile.host : "";
  });
  const [sshClientTtyDraft, setSshClientTtyDraft] = useState(() => {
    const profile = getActiveProfile(loadStoredProfiles());
    return profile.kind === "ssh" ? profile.clientTty : "";
  });
  const [debugEntries, setDebugEntries] = useState<DebugEntry[]>([]);
  const [liveState, setLiveState] = useState<LiveConnectionState>({
    status: "connecting",
    message: "Starting live client",
  });
  const [pickerFilter, setPickerFilter] = useState("");
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [isPickerVisible, setIsPickerVisible] = useState(true);
  const [selectedPaneId, setSelectedPaneId] = useState<string | null>(null);
  const [activation, setActivation] = useState<PickerActivation>({ status: "idle" });
  const validation = useMemo(
    () =>
      validateProfileDraft(
        activeProfile,
        profileNameDraft,
        settingsDraft,
        sshHostDraft,
        sshClientTtyDraft,
      ),
    [activeProfile, profileNameDraft, settingsDraft, sshHostDraft, sshClientTtyDraft],
  );
  const isSettingsDirty = useMemo(
    () =>
      profileNameDraft.trim() !== activeProfile.name ||
      sshHostDraft.trim() !== (activeProfile.kind === "ssh" ? activeProfile.host : "") ||
      sshClientTtyDraft.trim() !==
        (activeProfile.kind === "ssh" ? activeProfile.clientTty : "") ||
      !runnerSettingsEqual(settingsDraft, activeProfile.runner),
    [activeProfile, profileNameDraft, settingsDraft, sshHostDraft, sshClientTtyDraft],
  );

  useEffect(() => {
    void placePickerWindow();
  }, []);

  useEffect(() => {
    let cancelled = false;

    async function loadShellState() {
      setState({ status: "loading" });
      setLiveState({
        status: "connecting",
        message: "Starting live client",
      });

      try {
        const profileValidation = validateProfileDraft(
          activeProfile,
          activeProfile.name,
          activeProfile.runner,
          activeProfile.kind === "ssh" ? activeProfile.host : "",
          activeProfile.kind === "ssh" ? activeProfile.clientTty : "",
        );
        if (profileValidation.errors.length > 0) {
          const message = profileValidation.errors.join(" ");
          setState({
            status: "ready",
            profiles: profileState.profiles.map(profileSummary),
            preflight: {
              binary: commandPrefix(activeProfile),
              ok: false,
              version: null,
              error: message,
            },
            picker: { status: "failed", message },
          });
          setLiveState({ status: "fatal", message, diagnostics: null });
          return;
        }

        const [profiles, preflight] = await Promise.all([
          Promise.resolve(profileState.profiles.map(profileSummary)),
          runCommand<AgentscanPreflight>(
            `${commandPrefix(activeProfile)} --version`,
            () =>
              invoke<AgentscanPreflight>("preflight_agentscan", {
                settings: runnerSettings,
              }),
            appendDebugEntry,
          ),
        ]);

        if (!cancelled) {
          setState({
            status: "ready",
            profiles,
            preflight,
            picker: { status: "loading" },
          });
        }

      } catch (error) {
        if (!cancelled) {
          setState({
            status: "failed",
            message: errorMessage(error),
          });
        }
      }
    }

    void loadShellState();

    return () => {
      cancelled = true;
    };
  }, [profileState, runnerSettings]);

  useEffect(() => {
    if (state.status !== "ready" || !state.preflight.ok) {
      return;
    }

    let disposed = false;
    let unlisten: UnlistenFn | null = null;

    function enqueueLiveOperation(operation: () => Promise<void>) {
      liveOperationQueue = liveOperationQueue.then(operation, operation);
      return liveOperationQueue;
    }

    async function startLivePicker() {
      try {
        unlisten = await listen<LivePickerEvent>(LIVE_PICKER_EVENT, (event) => {
          if (!disposed) {
            applyLivePickerEvent(event.payload, setLiveState, setState);
            appendDebugEntry({
              kind: "stream",
              label: liveStateLabelFromEvent(event.payload),
              detail: liveEventDetail(event.payload),
            });
          }
        });
      } catch (error) {
        if (!disposed) {
          const message = errorMessage(error);
          setLiveState({ status: "fatal", message, diagnostics: null });
          markPickerFailedIfEmpty(setState, message);
        }
        return;
      }

      if (disposed) {
        unlisten();
        unlisten = null;
        return;
      }

      try {
        await enqueueLiveOperation(async () => {
          if (!disposed) {
            await runCommand(
              `${commandPrefix(activeProfile)} subscribe --format json`,
              () => invoke("start_live_picker", { settings: runnerSettings }),
              appendDebugEntry,
            );
          }
        });
      } catch (error) {
        if (!disposed) {
          const message = errorMessage(error);
          setLiveState({ status: "fatal", message, diagnostics: null });
          markPickerFailedIfEmpty(setState, message);
        }
      }
    }

    void startLivePicker();

    return () => {
      disposed = true;
      unlisten?.();
      void enqueueLiveOperation(async () => {
        await runCommand("stop live picker", () => invoke("stop_live_picker"), appendDebugEntry);
      });
    };
  }, [activeProfile, runnerSettings, state.status]);

  useEffect(() => {
    let disposed = false;
    let registered = false;

    function enqueueHotkeyOperation(operation: () => Promise<void>) {
      hotkeyOperationQueue = hotkeyOperationQueue.then(operation, operation);
      return hotkeyOperationQueue;
    }

    function registerPickerHotkey() {
      return enqueueHotkeyOperation(async () => {
        if (disposed) {
          return;
        }

        try {
          await register(PICKER_HOTKEY, (event) => {
            if (event.state === "Pressed") {
              void togglePickerWindow(() => setIsPickerVisible(true));
            }
          });
          registered = true;

          if (disposed) {
            await unregister(PICKER_HOTKEY);
            registered = false;
          }
        } catch (error) {
          if (disposed) {
            return;
          }

          setActivation({
            status: "failed",
            message: `Unable to register ${PICKER_HOTKEY}: ${errorMessage(error)}`,
          });
        }
      });
    }

    void registerPickerHotkey();

    return () => {
      disposed = true;
      void enqueueHotkeyOperation(async () => {
        if (registered) {
          await unregister(PICKER_HOTKEY);
          registered = false;
        }
      });
    };
  }, []);

  const statusText = useMemo(() => {
    if (state.status === "loading") {
      return `Checking ${profileKindLabel(activeProfile)} CLI`;
    }

    if (state.status === "failed") {
      return "IPC failed";
    }

    return state.preflight.ok
      ? `${profileKindLabel(activeProfile)} CLI ready`
      : `${profileKindLabel(activeProfile)} CLI unavailable`;
  }, [activeProfile, state]);

  async function refreshPickerRows() {
    if (state.status !== "ready") {
      return;
    }

    setIsRefreshing(true);
    setState({ ...state, picker: { status: "loading" } });

    try {
      const rows = await runCommand<PickerRow[]>(
        `${commandPrefix(activeProfile)} hotkeys --format json`,
        () => invoke<PickerRow[]>("load_picker_rows", { settings: runnerSettings }),
        appendDebugEntry,
      );
      setState((current) =>
        current.status === "ready"
          ? { ...current, picker: { status: "ready", rows } }
          : current,
      );
    } catch (error) {
      setState((current) =>
        current.status === "ready"
          ? { ...current, picker: { status: "failed", message: errorMessage(error) } }
          : current,
      );
    } finally {
      setIsRefreshing(false);
    }
  }

  const allPickerRows =
    state.status === "ready" && state.picker.status === "ready" ? state.picker.rows : [];
  const pickerRows = useMemo(
    () => filterPickerRows(allPickerRows, pickerFilter),
    [allPickerRows, pickerFilter],
  );
  const pickerStatus = state.status === "ready" ? state.picker.status : state.status;
  const selectedIndex = selectedPaneId
    ? Math.max(0, pickerRows.findIndex((row) => row.pane_id === selectedPaneId))
    : 0;
  const selectedRow = pickerRows[selectedIndex] ?? null;

  useEffect(() => {
    if (pickerStatus === "loading") {
      return;
    }

    if (pickerRows.length === 0) {
      if (allPickerRows.length === 0 && selectedPaneId !== null) {
        setSelectedPaneId(null);
      }
      return;
    }

    if (!selectedPaneId || !pickerRows.some((row) => row.pane_id === selectedPaneId)) {
      setSelectedPaneId(pickerRows[0].pane_id);
    }
  }, [allPickerRows.length, pickerRows, pickerStatus, selectedPaneId]);

  useEffect(() => {
    setProfileNameDraft(activeProfile.name);
    setSettingsDraft(activeProfile.runner);
    setSshHostDraft(activeProfile.kind === "ssh" ? activeProfile.host : "");
    setSshClientTtyDraft(activeProfile.kind === "ssh" ? activeProfile.clientTty : "");
  }, [activeProfile]);

  async function activateSelectedRow(row = selectedRow) {
    if (!row || activation.status === "running") {
      return;
    }

    setActivation({ status: "running", paneId: row.pane_id });

    try {
      await runCommand(
        focusCommandLabel(activeProfile, row.pane_id),
        () => invoke("focus_picker_row", { paneId: row.pane_id, settings: runnerSettings }),
        appendDebugEntry,
      );
      setActivation({ status: "idle" });
      await hidePickerWindow();
    } catch (error) {
      setActivation({ status: "failed", message: errorMessage(error) });
      await refreshPickerRows();
    }
  }

  function moveSelection(delta: number) {
    if (pickerRows.length === 0) {
      return;
    }

    const next = selectedIndex + delta;
    const clamped = Math.max(0, Math.min(next, pickerRows.length - 1));
    setSelectedPaneId(pickerRows[clamped].pane_id);
  }

  function handlePickerKeyDown(event: KeyboardEvent) {
    if (!isPickerVisible) {
      return;
    }

    if (isInteractiveShortcutTarget(event.target)) {
      return;
    }

    if (event.key === "ArrowDown" || event.key === "j") {
      event.preventDefault();
      moveSelection(1);
    } else if (event.key === "ArrowUp" || event.key === "k") {
      event.preventDefault();
      moveSelection(-1);
    } else if (event.key === "Home") {
      event.preventDefault();
      setSelectedPaneId(pickerRows[0]?.pane_id ?? null);
    } else if (event.key === "End") {
      event.preventDefault();
      setSelectedPaneId(pickerRows[pickerRows.length - 1]?.pane_id ?? null);
    } else if (event.key === "Enter") {
      event.preventDefault();
      void activateSelectedRow();
    } else if (event.key === "Escape") {
      event.preventDefault();
      setIsPickerVisible(false);
      void hidePickerWindow();
    }
  }

  function applyRunnerSettings() {
    const validation = validateProfileDraft(
      activeProfile,
      profileNameDraft,
      settingsDraft,
      sshHostDraft,
      sshClientTtyDraft,
    );
    if (validation.errors.length > 0) {
      appendDebugEntry({
        kind: "settings",
        label: `${activeProfile.name} settings rejected`,
        detail: validation.errors.join(" · "),
      });
      return;
    }

    const normalized = normalizeRunnerSettings(settingsDraft);
    setSettingsDraft(normalized);
    setProfileState((current) => {
      const next = updateActiveProfileSettings(
        current,
        profileNameDraft.trim(),
        normalized,
        sshHostDraft,
        sshClientTtyDraft,
      );
      storeProfiles(next);
      return next;
    });
    appendDebugEntry({
      kind: "settings",
      label: `${activeProfile.name} settings applied`,
      detail: `${runnerSummary(normalized)} · ${normalized.env.length} env ${normalized.env.length === 1 ? "name" : "names"}`,
    });
  }

  function resetProfileSettings() {
    setProfileNameDraft(activeProfile.name);
    setSettingsDraft(activeProfile.runner);
    setSshHostDraft(activeProfile.kind === "ssh" ? activeProfile.host : "");
    setSshClientTtyDraft(activeProfile.kind === "ssh" ? activeProfile.clientTty : "");
  }

  function selectProfile(id: string) {
    setProfileState((current) => {
      const profile = current.profiles.find((candidate) => candidate.id === id);
      if (!profile || !isRunnableProfile(profile)) {
        return current;
      }

      const next = { ...current, activeProfileId: id };
      storeProfiles(next);
      return next;
    });
  }

  function addSshProfile() {
    setProfileState((current) => {
      const profile: SshProfileConfig = {
        id: newProfileId("ssh"),
        name: nextRemoteProfileName(current.profiles),
        kind: "ssh",
        host: "",
        clientTty: "",
        runner: emptyRunnerSettings(),
        enabled: true,
      };
      const next = {
        activeProfileId: profile.id,
        profiles: [...current.profiles, profile],
      };
      storeProfiles(next);
      return next;
    });
  }

  function deleteActiveProfile() {
    if (activeProfile.kind === "local") {
      return;
    }

    setProfileState((current) => {
      const profiles = current.profiles.filter((profile) => profile.id !== activeProfile.id);
      const fallback = profiles.find((profile) => profile.kind === "local") ?? profiles[0];
      const next = normalizeProfileState({
        activeProfileId: fallback?.id,
        profiles,
      });
      storeProfiles(next);
      return next;
    });
    appendDebugEntry({
      kind: "settings",
      label: `${activeProfile.name} profile deleted`,
      detail: "active profile changed",
    });
  }

  function updateEnvironmentVariable(index: number, patch: Partial<EnvironmentVariable>) {
    setSettingsDraft((current) => ({
      ...current,
      env: current.env.map((variable, variableIndex) =>
        variableIndex === index ? { ...variable, ...patch } : variable,
      ),
    }));
  }

  function addEnvironmentVariable() {
    setSettingsDraft((current) => ({
      ...current,
      env: [...current.env, { name: "", value: "" }],
    }));
  }

  function removeEnvironmentVariable(index: number) {
    setSettingsDraft((current) => ({
      ...current,
      env: current.env.filter((_, variableIndex) => variableIndex !== index),
    }));
  }

  function appendDebugEntry(entry: Omit<DebugEntry, "id" | "time">) {
    setDebugEntries((current) =>
      [
        {
          ...entry,
          id: Date.now() + Math.random(),
          time: new Date().toLocaleTimeString(),
        },
        ...current,
      ].slice(0, DEBUG_LOG_LIMIT),
    );
  }

  useEffect(() => {
    window.addEventListener("keydown", handlePickerKeyDown);
    return () => window.removeEventListener("keydown", handlePickerKeyDown);
  });

  return (
    <main className="app-shell">
      <section className="summary">
        <div>
          <p className="eyebrow">agentscan desktop</p>
          <h1>Local agent workspace</h1>
        </div>
        <span className="status-pill">{statusText}</span>
      </section>

      {state.status === "ready" ? (
        <>
          <section className="content-grid" aria-label="Desktop shell state">
            <div className="panel">
              <div className="panel-heading compact">
                <h2>Profiles</h2>
                <button type="button" onClick={addSshProfile}>
                  Add SSH
                </button>
              </div>
              <ul className="profile-list">
                {state.profiles.map((profile) => (
                  <li key={profile.id}>
                    <button
                      aria-pressed={profile.id === activeProfile.id}
                      className={profile.id === activeProfile.id ? "active" : undefined}
                      type="button"
                      onClick={() => selectProfile(profile.id)}
                    >
                      <span>{profile.name}</span>
                      <small>{profile.kind === "ssh" ? "ssh" : profile.kind}</small>
                    </button>
                  </li>
                ))}
              </ul>
            </div>

            <div className="panel">
              <h2>Preflight</h2>
              <dl className="preflight">
                <div>
                  <dt>Binary</dt>
                  <dd>{state.preflight.binary}</dd>
                </div>
                <div>
                  <dt>Version</dt>
                  <dd>{state.preflight.version ?? "Unavailable"}</dd>
                </div>
                {!state.preflight.ok ? (
                  <div>
                    <dt>Error</dt>
                    <dd>{state.preflight.error ?? "Unknown failure"}</dd>
                  </div>
                ) : null}
              </dl>
            </div>
          </section>

          <section className="settings-panel" aria-label="Local runner settings">
            <div className="panel-heading">
              <h2>{activeProfile.name} Runner</h2>
              <div className="panel-actions">
                {activeProfile.kind === "ssh" ? (
                  <button
                    className="secondary-button danger"
                    type="button"
                    onClick={deleteActiveProfile}
                  >
                    Delete
                  </button>
                ) : null}
                <button
                  className="secondary-button"
                  type="button"
                  onClick={resetProfileSettings}
                  disabled={!isSettingsDirty}
                >
                  Reset
                </button>
                <button
                  type="button"
                  onClick={applyRunnerSettings}
                  disabled={!isSettingsDirty || validation.errors.length > 0}
                >
                  Apply
                </button>
              </div>
            </div>
            {validation.errors.length > 0 ? (
              <div className="error-state settings-error" role="alert">
                <h3>Invalid settings</h3>
                <ul>
                  {validation.errors.map((error) => (
                    <li key={error}>{error}</li>
                  ))}
                </ul>
              </div>
            ) : null}
            <div className="settings-grid">
              <label>
                <span>profile name</span>
                <input
                  value={profileNameDraft}
                  onChange={(event) => setProfileNameDraft(event.target.value)}
                />
              </label>
              {activeProfile.kind === "ssh" ? (
                <label>
                  <span>ssh host</span>
                  <input
                    value={sshHostDraft}
                    onChange={(event) => setSshHostDraft(event.target.value)}
                    placeholder="user@host"
                  />
                </label>
              ) : null}
              {activeProfile.kind === "ssh" ? (
                <label>
                  <span>remote client tty</span>
                  <input
                    value={sshClientTtyDraft}
                    onChange={(event) => setSshClientTtyDraft(event.target.value)}
                    placeholder="Best-effort"
                  />
                </label>
              ) : null}
              <label>
                <span>agentscan binary</span>
                <input
                  value={settingsDraft.binaryPath}
                  onChange={(event) =>
                    setSettingsDraft((current) => ({
                      ...current,
                      binaryPath: event.target.value,
                    }))
                  }
                  placeholder="Auto-detect"
                />
              </label>
              <div className="env-editor">
                <span>environment</span>
                <div className="env-list">
                  {settingsDraft.env.map((variable, index) => (
                    <div className="env-row" key={index}>
                      <input
                        aria-label="Environment variable name"
                        value={variable.name}
                        onChange={(event) =>
                          updateEnvironmentVariable(index, { name: event.target.value })
                        }
                        placeholder="NAME"
                        spellCheck={false}
                      />
                      <input
                        aria-label="Environment variable value"
                        value={variable.value}
                        onChange={(event) =>
                          updateEnvironmentVariable(index, { value: event.target.value })
                        }
                        placeholder="value"
                        spellCheck={false}
                      />
                      <button
                        className="secondary-button"
                        type="button"
                        onClick={() => removeEnvironmentVariable(index)}
                      >
                        Remove
                      </button>
                    </div>
                  ))}
                </div>
                <button
                  className="secondary-button"
                  type="button"
                  onClick={addEnvironmentVariable}
                >
                  Add env
                </button>
              </div>
            </div>
          </section>

          <section className="picker-panel" aria-label="Local picker rows" tabIndex={-1}>
            <div className="panel-heading">
              <h2>Picker</h2>
              <div className="panel-actions">
                {!isPickerVisible ? (
                  <button type="button" onClick={() => setIsPickerVisible(true)}>
                    Show
                  </button>
                ) : null}
                <button
                  type="button"
                  onClick={refreshPickerRows}
                  disabled={isRefreshing || isSettingsDirty || validation.errors.length > 0}
                >
                  {isRefreshing ? "Refreshing" : "Refresh"}
                </button>
              </div>
            </div>

            <LiveConnectionBanner state={liveState} />

            <div className="picker-toolbar">
              <label className="picker-search">
                <span>Search picker rows</span>
                <input
                  value={pickerFilter}
                  onChange={(event) => setPickerFilter(event.target.value)}
                  placeholder="Filter by agent, status, key, or location"
                />
              </label>
              <div className="picker-filter-actions">
                <span>
                  {pickerRows.length} / {allPickerRows.length}
                </span>
                {pickerFilter.trim() ? (
                  <button
                    className="secondary-button"
                    type="button"
                    onClick={() => setPickerFilter("")}
                  >
                    Clear
                  </button>
                ) : null}
              </div>
            </div>

            {activation.status === "failed" ? (
              <div className="error-state activation-error" role="alert">
                <h3>Unable to focus pane</h3>
                <p>{activation.message}</p>
              </div>
            ) : null}

            {isPickerVisible ? (
              <PickerRows
                activation={activation}
                filterQuery={pickerFilter}
                rows={pickerRows}
                selectedPaneId={selectedPaneId}
                state={state.picker}
                totalRows={allPickerRows.length}
                onActivate={activateSelectedRow}
                onClearFilter={() => setPickerFilter("")}
                onSelect={(row) => setSelectedPaneId(row.pane_id)}
              />
            ) : (
              <p className="muted">Picker hidden.</p>
            )}
          </section>

          <section className="debug-panel" aria-label="Command debug log">
            <div className="panel-heading">
              <h2>Debug</h2>
              <button type="button" onClick={() => setDebugEntries([])}>
                Clear
              </button>
            </div>
            <DebugLog entries={debugEntries} />
          </section>
        </>
      ) : (
        <section className="panel" aria-live="polite">
          <h2>{state.status === "loading" ? "Loading" : "Unable to load"}</h2>
          <p>{state.status === "failed" ? state.message : "Waiting for backend response."}</p>
        </section>
      )}
    </main>
  );
}

function applyLivePickerEvent(
  event: LivePickerEvent,
  setLiveState: (value: SetStateAction<LiveConnectionState>) => void,
  setState: (value: SetStateAction<LoadState>) => void,
) {
  if (event.kind === "rows") {
    setLiveState({
      status: "online",
      message: `${event.rows.length} picker ${event.rows.length === 1 ? "row" : "rows"}`,
      snapshot: event.snapshot,
    });
    setState((current) =>
      current.status === "ready"
        ? { ...current, picker: { status: "ready", rows: event.rows } }
        : current,
    );
    return;
  }

  if (event.kind === "connecting") {
    setLiveState({ status: "connecting", message: event.message });
    return;
  }

  if (event.kind === "reconnecting") {
    setLiveState({
      status: "reconnecting",
      message: event.message,
      diagnostics: event.diagnostics,
    });
    return;
  }

  if (event.kind === "offline") {
    setLiveState({
      status: "offline",
      message: event.message,
      retrying: event.retrying,
      diagnostics: event.diagnostics,
    });
    markPickerFailedIfEmpty(setState, event.message);
    return;
  }

  if (event.kind === "shutdown") {
    setLiveState({ status: "shutdown", message: event.message });
    markPickerFailedIfEmpty(setState, event.message);
    return;
  }

  setLiveState({
    status: "fatal",
    message: event.message,
    diagnostics: event.diagnostics,
  });
  markPickerFailedIfEmpty(setState, event.message);
}

function markPickerFailedIfEmpty(
  setState: (value: SetStateAction<LoadState>) => void,
  message: string,
) {
  setState((current) => {
    if (current.status !== "ready" || current.picker.status === "ready") {
      return current;
    }

    return { ...current, picker: { status: "failed", message } };
  });
}

async function runCommand<T>(
  label: string,
  operation: () => Promise<T>,
  appendDebugEntry: (entry: Omit<DebugEntry, "id" | "time">) => void,
) {
  appendDebugEntry({ kind: "command", label, detail: "started" });

  try {
    const result = await operation();
    appendDebugEntry({ kind: "command", label, detail: "ok" });
    return result;
  } catch (error) {
    appendDebugEntry({
      kind: "command",
      label,
      detail: errorMessage(error),
    });
    throw error;
  }
}

function LiveConnectionBanner({ state }: { state: LiveConnectionState }) {
  const tone = state.status === "online" ? "ready" : state.status === "fatal" ? "error" : "warn";

  return (
    <div className={`live-banner ${tone}`} aria-live="polite">
      <div>
        <span>{liveStateLabel(state)}</span>
        <p>{state.message}</p>
      </div>
      {state.status === "online" ? (
        <small>
          {state.snapshot.paneCount} panes
          {state.snapshot.sourceKind ? ` · ${state.snapshot.sourceKind}` : ""}
        </small>
      ) : isDiagnosticState(state) && state.diagnostics ? (
        <small>{formatDiagnostics(state.diagnostics)}</small>
      ) : null}
    </div>
  );
}

function DebugLog({ entries }: { entries: DebugEntry[] }) {
  if (entries.length === 0) {
    return <p className="muted">No debug events yet.</p>;
  }

  return (
    <ol className="debug-list">
      {entries.map((entry) => (
        <li key={entry.id}>
          <time>{entry.time}</time>
          <span>{entry.kind}</span>
          <strong>{entry.label}</strong>
          <small>{entry.detail}</small>
        </li>
      ))}
    </ol>
  );
}

function loadStoredRunnerSettings(): RunnerSettings {
  try {
    const value = window.localStorage.getItem(SETTINGS_STORAGE_KEY);
    if (!value) {
      return emptyRunnerSettings();
    }

    return normalizeRunnerSettings(JSON.parse(value) as Partial<RunnerSettings>);
  } catch {
    return emptyRunnerSettings();
  }
}

function loadStoredProfiles(): ProfileState {
  try {
    const value = window.localStorage.getItem(PROFILES_STORAGE_KEY);
    if (!value) {
      return defaultProfileState(loadStoredRunnerSettings());
    }

    return normalizeProfileState(JSON.parse(value) as Partial<ProfileState>);
  } catch {
    return defaultProfileState(loadStoredRunnerSettings());
  }
}

function storeProfiles(state: ProfileState) {
  window.localStorage.setItem(PROFILES_STORAGE_KEY, JSON.stringify(state));
  const localProfile = state.profiles.find((profile) => profile.kind === "local");
  if (localProfile) {
    window.localStorage.setItem(SETTINGS_STORAGE_KEY, JSON.stringify(localProfile.runner));
  }
}

function storeRunnerSettings(settings: RunnerSettings) {
  window.localStorage.setItem(SETTINGS_STORAGE_KEY, JSON.stringify(settings));
}

function emptyRunnerSettings(): RunnerSettings {
  return { binaryPath: "", env: [] };
}

function defaultProfileState(runner = emptyRunnerSettings()): ProfileState {
  return {
    activeProfileId: LOCAL_PROFILE_ID,
    profiles: [
      {
        id: LOCAL_PROFILE_ID,
        name: "Local",
        kind: "local",
        runner: normalizeRunnerSettings(runner),
      },
    ],
  };
}

function normalizeProfileState(value: Partial<ProfileState>): ProfileState {
  const profiles = Array.isArray(value.profiles)
    ? value.profiles.map(normalizeProfile).filter((profile) => profile !== null)
    : [];

  if (!profiles.some((profile) => profile.kind === "local")) {
    profiles.unshift(defaultProfileState(loadStoredRunnerSettings()).profiles[0]);
  }

  const fallbackProfile = profiles.find(isRunnableProfile) ?? profiles[0];
  const activeProfileId =
    typeof value.activeProfileId === "string" &&
    profiles.some((profile) => profile.id === value.activeProfileId && isRunnableProfile(profile))
      ? value.activeProfileId
      : fallbackProfile.id;

  return { activeProfileId, profiles };
}

function normalizeProfile(value: unknown): DesktopProfileConfig | null {
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
      name: name || "Local",
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
      clientTty:
        typeof profile.clientTty === "string" ? profile.clientTty.trim() : "",
      runner,
      enabled: profile.enabled === true,
    };
  }

  return null;
}

function getActiveProfile(state: ProfileState): DesktopProfileConfig {
  return (
    state.profiles.find(
      (profile) => profile.id === state.activeProfileId && isRunnableProfile(profile),
    ) ??
    state.profiles.find(isRunnableProfile) ??
    state.profiles[0]
  );
}

function isRunnableProfile(profile: DesktopProfileConfig): boolean {
  return profile.kind === "local" || profile.enabled;
}

function updateActiveProfileSettings(
  state: ProfileState,
  name: string,
  runner: RunnerSettings,
  sshHost: string,
  sshClientTty: string,
): ProfileState {
  return {
    ...state,
    profiles: state.profiles.map((profile) =>
      profile.id === state.activeProfileId
        ? updateProfileSettings(profile, name, runner, sshHost, sshClientTty)
        : profile,
    ),
  };
}

function updateProfileSettings(
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

function profileSummary(profile: DesktopProfileConfig): DesktopProfile {
  return {
    id: profile.id,
    name: profile.name,
    kind: profile.kind,
  };
}

function runnerSummary(settings: RunnerSettings) {
  return settings.binaryPath.trim() || "auto-detected agentscan";
}

function runnerSettingsForProfile(profile: DesktopProfileConfig): DesktopRunnerSettings {
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

function commandPrefix(profile: DesktopProfileConfig) {
  const binary = profile.runner.binaryPath.trim() || "agentscan";

  if (profile.kind === "ssh") {
    return `ssh ${profile.host || "<host>"} -- ${binary}`;
  }

  return binary;
}

function focusCommandLabel(profile: DesktopProfileConfig, paneId: string) {
  const base = `${commandPrefix(profile)} focus`;
  if (profile.kind === "ssh" && profile.clientTty.trim()) {
    return `${base} --client-tty ${profile.clientTty.trim()} ${paneId}`;
  }

  return `${base} ${paneId}`;
}

function profileKindLabel(profile: DesktopProfileConfig) {
  return profile.kind === "ssh" ? "SSH" : "Local";
}

function validateProfileDraft(
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
    const name = variable.name.trim();
    if (!name) {
      errors.push(`Environment row ${index + 1} needs a name.`);
      return;
    }

    if (name.includes("=") || name.includes("\0")) {
      errors.push(`Environment row ${index + 1} has an invalid name.`);
      return;
    }

    if (seenNames.has(name)) {
      errors.push(`Environment variable ${name} is duplicated.`);
    }
    seenNames.add(name);
  });

  return { errors };
}

function runnerSettingsEqual(left: RunnerSettings, right: RunnerSettings) {
  if (left.binaryPath !== right.binaryPath || left.env.length !== right.env.length) {
    return false;
  }

  return left.env.every(
    (variable, index) =>
      variable.name === right.env[index]?.name && variable.value === right.env[index]?.value,
  );
}

function newProfileId(prefix: string) {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return `${prefix}-${crypto.randomUUID()}`;
  }

  return `${prefix}-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

function nextRemoteProfileName(profiles: DesktopProfileConfig[]) {
  const remoteCount = profiles.filter((profile) => profile.kind === "ssh").length;
  return remoteCount === 0 ? "Remote" : `Remote ${remoteCount + 1}`;
}

function normalizeRunnerSettings(settings: Partial<RunnerSettings>): RunnerSettings {
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

function filterPickerRows(rows: PickerRow[], query: string) {
  const terms = query
    .trim()
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean);

  if (terms.length === 0) {
    return rows;
  }

  return rows.filter((row) => {
    const searchable = [
      row.key,
      row.pane_id,
      row.provider ?? "unknown",
      row.status.kind,
      row.display_label,
      row.location_tag,
    ]
      .join(" ")
      .toLowerCase();

    return terms.every((term) => searchable.includes(term));
  });
}

function liveStateLabelFromEvent(event: LivePickerEvent) {
  if (event.kind === "rows") {
    return "snapshot";
  }

  return event.kind;
}

function liveEventDetail(event: LivePickerEvent) {
  if (event.kind === "rows") {
    return `${event.rows.length} rows · ${event.snapshot.paneCount} panes`;
  }

  if (event.kind === "offline") {
    return `${event.message}${event.retrying ? " · retrying" : ""}`;
  }

  return event.message;
}

function liveStateLabel(state: LiveConnectionState) {
  if (state.status === "online") {
    return "Live";
  }

  if (state.status === "reconnecting") {
    return "Reconnecting";
  }

  if (state.status === "offline") {
    return state.retrying ? "Offline, retrying" : "Offline";
  }

  if (state.status === "fatal") {
    return "Live client failed";
  }

  if (state.status === "shutdown") {
    return "Daemon shutdown";
  }

  return "Connecting";
}

function isDiagnosticState(
  state: LiveConnectionState,
): state is Extract<LiveConnectionState, { diagnostics: unknown | null }> {
  return "diagnostics" in state;
}

function formatDiagnostics(diagnostics: unknown) {
  if (!diagnostics || typeof diagnostics !== "object") {
    return String(diagnostics);
  }

  const status = diagnostics as { state?: unknown; message?: unknown; subscriber_count?: unknown };
  const parts = [
    typeof status.state === "string" ? status.state : null,
    typeof status.message === "string" ? status.message : null,
    typeof status.subscriber_count === "number"
      ? `${status.subscriber_count} subscribers`
      : null,
  ].filter(Boolean);

  return parts.length > 0 ? parts.join(" · ") : "Daemon diagnostics available";
}

async function togglePickerWindow(beforeShow: () => void) {
  await enqueueWindowOperation(async () => {
    const appWindow = getCurrentWindow();

    if (await appWindow.isVisible()) {
      await appWindow.hide();
      return;
    }

    beforeShow();
    await placePickerWindow();
    await appWindow.show();
    await appWindow.setFocus();
  });
}

async function hidePickerWindow() {
  await enqueueWindowOperation(async () => {
    await getCurrentWindow().hide();
  });
}

function enqueueWindowOperation(operation: () => Promise<void>) {
  windowOperationQueue = windowOperationQueue.then(operation, operation);
  return windowOperationQueue;
}

async function placePickerWindow() {
  try {
    await invoke("place_picker_window");
  } catch {
    // Placement is best-effort; showing and focusing the picker is more important.
  }
}

function PickerRows({
  activation,
  filterQuery,
  rows,
  selectedPaneId,
  state,
  totalRows,
  onActivate,
  onClearFilter,
  onSelect,
}: {
  activation: PickerActivation;
  filterQuery: string;
  rows: PickerRow[];
  selectedPaneId: string | null;
  state: PickerState;
  totalRows: number;
  onActivate: (row: PickerRow) => void;
  onClearFilter: () => void;
  onSelect: (row: PickerRow) => void;
}) {
  if (state.status === "loading") {
    return <p className="muted">Loading picker rows.</p>;
  }

  if (state.status === "failed") {
    return (
      <div className="error-state" role="alert">
        <h3>Unable to load picker rows</h3>
        <p>{state.message}</p>
      </div>
    );
  }

  if (totalRows > 0 && rows.length === 0 && filterQuery.trim()) {
    return (
      <div className="empty-filter-state">
        <p>No picker rows match this filter.</p>
        <button className="secondary-button" type="button" onClick={onClearFilter}>
          Clear
        </button>
      </div>
    );
  }

  if (rows.length === 0) {
    return <p className="muted">No picker rows are available.</p>;
  }

  return (
    <ul className="picker-list">
      {rows.map((row) => (
        <li
          aria-selected={row.pane_id === selectedPaneId}
          className={row.pane_id === selectedPaneId ? "selected" : undefined}
          key={`${row.key}-${row.pane_id}`}
          onClick={() => onSelect(row)}
          onDoubleClick={() => onActivate(row)}
        >
          <kbd>{row.key}</kbd>
          <div className="picker-row-main">
            <span>{row.display_label}</span>
            <small>{row.location_tag}</small>
          </div>
          <div className="picker-row-meta">
            <span>{row.provider ?? "unknown"}</span>
            <small>
              {activation.status === "running" && activation.paneId === row.pane_id
                ? "focusing"
                : row.status.kind}
            </small>
          </div>
        </li>
      ))}
    </ul>
  );
}

function isInteractiveShortcutTarget(target: EventTarget | null) {
  if (!(target instanceof HTMLElement)) {
    return false;
  }

  return (
    target.isContentEditable ||
    Boolean(target.closest("button,input,select,textarea,a,[contenteditable]"))
  );
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

export default App;
