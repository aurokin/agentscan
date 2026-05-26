import { useEffect, useMemo, useRef, useState, type SetStateAction } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { register, unregister } from "@tauri-apps/plugin-global-shortcut";
import { providerLogo } from "./providerLogos";

const PICKER_HOTKEY = "CommandOrControl+Shift+A";
const LIVE_PICKER_EVENT = "agentscan-live-picker";
const SETTINGS_STORAGE_KEY = "agentscan.desktop.localRunnerSettings";
const PROFILES_STORAGE_KEY = "agentscan.desktop.profiles";
// Keep in sync with the pre-paint theme script in index.html.
const THEME_STORAGE_KEY = "agentscan.desktop.theme";
const DEBUG_LOG_LIMIT = 80;
const LOCAL_PROFILE_ID = "local";

let hotkeyOperationQueue = Promise.resolve();
let liveOperationQueue = Promise.resolve();
let windowOperationQueue = Promise.resolve();

// Monotonic id assigned to each live-picker subscription. The backend stamps
// every emitted event with the epoch of the worker that produced it so the
// frontend can drop late frames from a superseded subscription after a switch.
//
// Epochs must be *strictly increasing across reloads/HMR*, not merely unique:
// after a window reload (which restarts the JS but leaves the Rust worker
// running) a late start() invoke from the torn-down page could otherwise
// replace the freshly reloaded page's worker, which filters events by its own
// epoch and would then drop every frame. The backend rejects a start whose
// epoch is not greater than the last one it honored, so the counter is
// persisted and seeded from wall-clock time — guaranteeing each page load
// produces higher epochs than any prior load even if the stored value is lost.
const LIVE_EPOCH_STORAGE_KEY = "agentscan.liveEpochSeq";
// Bounded self-heal for a failed live-picker start (transient IPC/spawn errors):
// retry a few times with a short delay, then stay fatal until the profile or
// settings change. Prevents a persistently bad config from retrying forever.
const LIVE_START_MAX_RETRIES = 4;
const LIVE_START_RETRY_DELAY_MS = 1500;
let liveEpochSeq = Date.now();
function nextLiveEpoch() {
  let base = liveEpochSeq;
  try {
    const stored = Number.parseInt(
      window.localStorage.getItem(LIVE_EPOCH_STORAGE_KEY) ?? "",
      10,
    );
    if (Number.isFinite(stored) && stored >= base) {
      base = stored;
    }
  } catch {
    // localStorage unavailable; the in-memory counter still increases per page.
  }
  liveEpochSeq = base + 1;
  try {
    window.localStorage.setItem(LIVE_EPOCH_STORAGE_KEY, String(liveEpochSeq));
  } catch {
    // Persistence is best-effort; monotonicity within this page still holds.
  }
  return liveEpochSeq;
}

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
  // The currently-focused tmux pane (active pane of the active window).
  // Distinct from the selection cursor; rendered with its own treatment.
  is_active: boolean;
};

type ShellView = "picker" | "settings";

type ThemePreference = "dark" | "light" | "system";

type PickerGroup = {
  project: string;
  rows: PickerRow[];
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

// Wire payload: a LivePickerEvent plus the epoch of the subscription that
// produced it (see backend LivePickerEnvelope).
type LivePickerEnvelope = LivePickerEvent & { epoch: number };

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
      // The runner config this resolved state describes (see runnerKeyForProfile).
      // `state` lags the active runner by one async cycle on a switch or settings
      // apply, so consumers compare this to the active runnerKey before trusting
      // preflight/picker for live decisions.
      runnerKey: string;
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
  // Identity of the exact runner configuration a resolved state describes. It
  // changes on a profile switch AND on any settings edit (binary/env/host/tty)
  // to the active profile, so resolved preflight/picker data is invalidated
  // whenever the underlying target changes, not just when the profile id does.
  const runnerKey = useMemo(() => runnerKeyForProfile(activeProfile), [activeProfile]);
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
  const [view, setView] = useState<ShellView>("picker");
  // Footer source switcher: which agentscan we're listening to (local vs a
  // remote over SSH). Open state for the inline dropdown.
  const [isSourceMenuOpen, setIsSourceMenuOpen] = useState(false);
  const sourceMenuRef = useRef<HTMLDivElement | null>(null);
  // Debug log is a diagnostic panel — collapsed by default to keep Settings calm.
  const [isDebugOpen, setIsDebugOpen] = useState(false);
  // Appearance: dark / light / system (system follows the OS).
  const [themePref, setThemePref] = useState<ThemePreference>(loadStoredTheme);
  // Tracks the active runner config so in-flight refreshes/focus can detect a
  // profile switch OR a settings change and discard results from the previous
  // target. Updated synchronously during render (below) so async completions
  // never observe a stale key through a late-running effect.
  const activeRunnerKeyRef = useRef(runnerKey);
  activeRunnerKeyRef.current = runnerKey;
  // Identifies the latest refresh request so a superseded refresh neither
  // applies its rows nor clears the spinner out from under a newer one.
  const refreshTokenRef = useRef(0);
  // Bumped each time a live snapshot applies rows, so a slower manual refresh
  // can detect that fresher live rows landed mid-flight and skip overwriting.
  const liveRowsSeqRef = useRef(0);
  // Bumped to re-run the live effect and re-attempt a failed start; the attempt
  // counter bounds retries so a persistently failing config settles into fatal.
  const [liveRetryToken, setLiveRetryToken] = useState(0);
  const liveRetryAttemptRef = useRef(0);
  const [selectedPaneId, setSelectedPaneId] = useState<string | null>(null);
  const [activation, setActivation] = useState<PickerActivation>({ status: "idle" });
  // Synchronous in-flight guard for activation. The `activation` state alone
  // can't gate concurrent activations: a double-click (or two rapid clicks)
  // dispatches both click events before React re-renders, so each handler reads
  // the same stale "idle" activation and a state-based guard lets both through —
  // firing focus_picker_row twice. A ref updates synchronously, so the second
  // click sees the in-flight activation and bails.
  const activationInFlightRef = useRef(false);
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
      // Keep any existing ready state so a profile/runner reload stays in the
      // picker (gated to a "Switching profile…" state via activeReadyState)
      // instead of dropping to the boot screen. Only the very first load, with
      // no ready state yet, shows the boot "Connecting" screen.
      setState((current) => (current.status === "ready" ? current : { status: "loading" }));
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
            runnerKey,
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
          const failureMessage = preflight.error ?? "agentscan is unavailable";
          setState((current) => {
            // If the restarted live subscription already delivered rows for this
            // profile, keep them rather than clobbering back to "loading" (which
            // would wedge until the next daemon snapshot).
            const keepLiveRows =
              preflight.ok &&
              current.status === "ready" &&
              current.runnerKey === runnerKey &&
              current.picker.status === "ready";
            return {
              status: "ready",
              runnerKey,
              profiles,
              preflight,
              picker: !preflight.ok
                ? { status: "failed", message: failureMessage }
                : keepLiveRows
                  ? current.picker
                  : { status: "loading" },
            };
          });
          if (!preflight.ok) {
            // The live effect won't run (liveReady is false), so settle the live
            // state instead of leaving it stuck on "Starting live client".
            setLiveState({ status: "fatal", message: failureMessage, diagnostics: null });
          }
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

  // Derived directly from the active profile (not from async preflight state) so
  // a switch to a synchronously-invalid profile immediately gates the live
  // picker off in the same render, rather than briefly subscribing the bad
  // target before loadShellState resolves.
  const activeProfileValid = useMemo(
    () =>
      validateProfileDraft(
        activeProfile,
        activeProfile.name,
        activeProfile.runner,
        activeProfile.kind === "ssh" ? activeProfile.host : "",
        activeProfile.kind === "ssh" ? activeProfile.clientTty : "",
      ).errors.length === 0,
    [activeProfile],
  );
  const liveReady =
    state.status === "ready" &&
    state.runnerKey === runnerKey &&
    state.preflight.ok &&
    activeProfileValid;

  useEffect(() => {
    if (!liveReady) {
      return;
    }

    let disposed = false;
    // The live event channel is global and shared across profiles. On a profile
    // switch a late frame from the previous worker can still arrive on the
    // channel, so the backend stamps every event with the epoch of the worker
    // that produced it and we accept only events matching this subscription's
    // epoch. This rejects superseded frames while still applying our own first
    // snapshot immediately.
    const epoch = nextLiveEpoch();
    const subscriptionRunnerKey = runnerKey;
    let unlisten: UnlistenFn | null = null;
    let retryTimer: ReturnType<typeof setTimeout> | undefined;

    function enqueueLiveOperation(operation: () => Promise<void>) {
      liveOperationQueue = liveOperationQueue.then(operation, operation);
      return liveOperationQueue;
    }

    // Re-arm the live effect after a failed start so transient IPC/spawn errors
    // self-heal without a manual profile switch. Bounded by the attempt counter.
    function scheduleLiveRetry() {
      if (disposed || liveRetryAttemptRef.current >= LIVE_START_MAX_RETRIES) {
        return;
      }
      liveRetryAttemptRef.current += 1;
      retryTimer = setTimeout(() => {
        if (!disposed) {
          setLiveRetryToken((token) => token + 1);
        }
      }, LIVE_START_RETRY_DELAY_MS);
    }

    async function startLivePicker() {
      try {
        unlisten = await listen<LivePickerEnvelope>(LIVE_PICKER_EVENT, (event) => {
          if (!disposed && event.payload.epoch === epoch) {
            if (liveEventChangesPickerRows(event.payload)) {
              // Mark that the picker rows changed (new live snapshot, or a
              // terminal event that cleared them) so an in-flight manual refresh
              // detects the supersession and won't overwrite/resurrect rows.
              liveRowsSeqRef.current += 1;
            }
            applyLivePickerEvent(event.payload, subscriptionRunnerKey, setLiveState, setState);
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
          markPickerFailedIfEmpty(setState, subscriptionRunnerKey, message);
          scheduleLiveRetry();
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
              () => invoke("start_live_picker", { settings: runnerSettings, epoch }),
              appendDebugEntry,
            );
          }
        });
        // The worker is installed; clear the retry budget for this runner.
        if (!disposed) {
          liveRetryAttemptRef.current = 0;
        }
      } catch (error) {
        if (!disposed) {
          const message = errorMessage(error);
          setLiveState({ status: "fatal", message, diagnostics: null });
          markPickerFailedIfEmpty(setState, subscriptionRunnerKey, message);
          scheduleLiveRetry();
        }
      }
    }

    void startLivePicker();

    return () => {
      disposed = true;
      if (retryTimer !== undefined) {
        clearTimeout(retryTimer);
      }
      unlisten?.();
      void enqueueLiveOperation(async () => {
        // Pass this subscription's epoch so a stale cleanup (after reload/HMR)
        // can't stop a newer worker; the backend only stops if the epoch matches.
        await runCommand(
          "stop live picker",
          () => invoke("stop_live_picker", { epoch }),
          appendDebugEntry,
        );
      });
    };
  }, [activeProfile, runnerSettings, liveReady, liveRetryToken]);

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
              void raisePickerWindow();
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
    // Preflight from a not-yet-refreshed previous profile is untrustworthy, so
    // report "Checking" until the resolved state matches the active profile.
    if (state.status === "loading" || (state.status === "ready" && state.runnerKey !== runnerKey)) {
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

    const requestRunnerKey = runnerKey;
    const token = (refreshTokenRef.current += 1);
    const liveSeqAtStart = liveRowsSeqRef.current;
    const isCurrent = () =>
      refreshTokenRef.current === token && activeRunnerKeyRef.current === requestRunnerKey;
    setIsRefreshing(true);
    // Functional + runner-guarded so a stale closure can never resurrect the
    // previous target's resolved state into the active one.
    setState((current) =>
      current.status === "ready" && current.runnerKey === requestRunnerKey
        ? { ...current, picker: { status: "loading" } }
        : current,
    );

    try {
      const rows = await runCommand<PickerRow[]>(
        `${commandPrefix(activeProfile)} hotkeys --format json`,
        () => invoke<PickerRow[]>("load_picker_rows", { settings: runnerSettings }),
        appendDebugEntry,
      );
      if (!isCurrent()) {
        return; // Superseded by a profile switch, settings apply, or newer refresh.
      }
      if (liveRowsSeqRef.current !== liveSeqAtStart) {
        return; // A fresher live snapshot landed mid-flight; don't overwrite it.
      }
      setState((current) =>
        current.status === "ready" && current.runnerKey === requestRunnerKey
          ? { ...current, picker: { status: "ready", rows } }
          : current,
      );
    } catch (error) {
      if (!isCurrent()) {
        return;
      }
      if (liveRowsSeqRef.current !== liveSeqAtStart) {
        return; // Fresher live rows already displayed; don't replace them with an error.
      }
      setState((current) =>
        current.status === "ready" && current.runnerKey === requestRunnerKey
          ? { ...current, picker: { status: "failed", message: errorMessage(error) } }
          : current,
      );
    } finally {
      // Only the latest refresh clears the spinner, so a superseded request
      // can't re-enable the button while a newer one is still running.
      if (refreshTokenRef.current === token) {
        setIsRefreshing(false);
      }
    }
  }

  // `state` lags the active runner by one async cycle after a switch or settings
  // apply (loadShellState resets it in an effect, after paint). Until the resolved
  // state's runnerKey matches the active runner, its preflight/picker rows belong
  // to the previous target, so treat that window as "loading" everywhere ready
  // data is consumed.
  const activeReadyState =
    state.status === "ready" && state.runnerKey === runnerKey ? state : null;
  // Sources offered in the footer quick-switch: the built-in local runner plus
  // enabled SSH profiles. A remote with no host yet can only resolve to a failed
  // source, so exclude it from quick-switch (it's still listed in Settings, where
  // it gets configured) — except keep the active one so the trigger's source is
  // always represented in its own menu.
  const sourceProfiles = useMemo(
    () =>
      profileState.profiles.filter(
        (profile) =>
          isRunnableProfile(profile) &&
          (profile.kind !== "ssh" ||
            profile.host.trim().length > 0 ||
            profile.id === activeProfile.id),
      ),
    [profileState, activeProfile.id],
  );
  // Tone for the footer status dot, derived from the resolved preflight of the
  // active source (not a stale previous one). Detail lives in the title tooltip.
  const sourceStatusTone = !activeReadyState
    ? "unknown"
    : activeReadyState.preflight.ok
      ? "idle"
      : "error";
  const pickerDataState: PickerState = activeReadyState
    ? activeReadyState.picker
    : { status: "loading" };
  // liveState lags the active profile the same way; show a neutral switching
  // banner rather than the previous profile's stale offline/fatal state.
  const displayLiveState: LiveConnectionState = activeReadyState
    ? liveState
    : { status: "connecting", message: "Switching profile…" };
  const allPickerRows =
    pickerDataState.status === "ready" ? pickerDataState.rows : [];
  const pickerRows = useMemo(
    () => groupRowsByProject(filterPickerRows(allPickerRows, pickerFilter)).flatMap((g) => g.rows),
    [allPickerRows, pickerFilter],
  );
  const pickerGroups = useMemo(() => groupRowsByProject(pickerRows), [pickerRows]);
  const pickerStatus = pickerDataState.status;
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
    // activeRunnerKeyRef is updated synchronously during render; here we reset
    // per-profile UI so nothing leaks across a switch: editable drafts, the
    // search filter, stale activation error, and the pane selection (tmux pane
    // ids like %1 collide across hosts/sessions, so a carried-over selectedPaneId
    // would silently highlight/activate a different agent on the new profile).
    setProfileNameDraft(activeProfile.name);
    setSettingsDraft(activeProfile.runner);
    setSshHostDraft(activeProfile.kind === "ssh" ? activeProfile.host : "");
    setSshClientTtyDraft(activeProfile.kind === "ssh" ? activeProfile.clientTty : "");
    setPickerFilter("");
    setActivation({ status: "idle" });
    // Free the in-flight activation guard too: an old focus_picker_row from the
    // previous target may still be running, but activeRunnerKeyRef already
    // discards its completion, so the new profile must be able to activate
    // immediately rather than waiting on the stale call's finally.
    activationInFlightRef.current = false;
    setSelectedPaneId(null);
    setIsSourceMenuOpen(false);
    liveRetryAttemptRef.current = 0;
  }, [activeProfile]);

  // Dismiss the source dropdown on an outside click or Escape. The keydown is
  // captured so it closes the menu before the picker's global Escape handler
  // hides the whole window.
  useEffect(() => {
    if (!isSourceMenuOpen) {
      return;
    }

    function onPointerDown(event: MouseEvent) {
      if (sourceMenuRef.current && !sourceMenuRef.current.contains(event.target as Node)) {
        setIsSourceMenuOpen(false);
      }
    }

    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.stopPropagation();
        event.preventDefault();
        setIsSourceMenuOpen(false);
      }
    }

    window.addEventListener("mousedown", onPointerDown);
    window.addEventListener("keydown", onKeyDown, true);
    return () => {
      window.removeEventListener("mousedown", onPointerDown);
      window.removeEventListener("keydown", onKeyDown, true);
    };
  }, [isSourceMenuOpen]);

  // Close the source menu whenever the picker isn't the active view. The
  // outside-click/Escape handlers above miss keyboard-driven navigation (e.g.
  // activating the gear or the boot screen's "Open settings" with Enter), which
  // would otherwise leave the dropdown rendered already-open on return.
  useEffect(() => {
    if (view !== "picker") {
      setIsSourceMenuOpen(false);
    }
  }, [view]);

  // Apply the theme to <html data-theme> and persist it. "system" resolves from
  // prefers-color-scheme and re-resolves live when the OS appearance changes.
  useEffect(() => {
    try {
      window.localStorage.setItem(THEME_STORAGE_KEY, themePref);
    } catch {
      // Persistence is best-effort; the in-memory preference still applies.
    }

    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      const resolved =
        themePref === "system" ? (media.matches ? "dark" : "light") : themePref;
      document.documentElement.setAttribute("data-theme", resolved);
    };
    apply();

    if (themePref !== "system") {
      return;
    }
    media.addEventListener("change", apply);
    return () => media.removeEventListener("change", apply);
  }, [themePref]);

  async function activateSelectedRow(row = selectedRow) {
    if (!row || activationInFlightRef.current) {
      return;
    }
    activationInFlightRef.current = true;

    const requestRunnerKey = runnerKey;
    setActivation({ status: "running", paneId: row.pane_id });

    try {
      await runCommand(
        focusCommandLabel(activeProfile, row.pane_id),
        () => invoke("focus_picker_row", { paneId: row.pane_id, settings: runnerSettings }),
        appendDebugEntry,
      );
      if (activeRunnerKeyRef.current !== requestRunnerKey) {
        // Target switched mid-flight (profile or settings). The profile-switch
        // effect already reset activation; don't touch it here or we could clear
        // a newer activation the user started. Also skip the post-focus UI.
        return;
      }
      // Persistent-window model: focusing a pane must not hide the desktop.
      // Reset activation to idle and leave the window visible.
      setActivation({ status: "idle" });
    } catch (error) {
      if (activeRunnerKeyRef.current !== requestRunnerKey) {
        return;
      }
      setActivation({ status: "failed", message: errorMessage(error) });
      await refreshPickerRows();
    } finally {
      // Only release the guard if this is still the active target. After a
      // profile/settings switch the effect already cleared the ref (and a newer
      // activation may have re-set it), so a stale completion must not clear a
      // guard that now belongs to a different in-flight activation.
      if (activeRunnerKeyRef.current === requestRunnerKey) {
        activationInFlightRef.current = false;
      }
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
    if (view !== "picker") {
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
      // Persistent-window model: Escape never hides the window. Clear the search
      // filter if one is active; otherwise it's a no-op.
      if (pickerFilter) {
        event.preventDefault();
        setPickerFilter("");
      }
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
      // No-op if already active: re-selecting would still bump loadShellState
      // (resetting liveState to "connecting") without re-running the live effect
      // (activeProfile identity is unchanged), wedging the live strip.
      if (id === current.activeProfileId) {
        return current;
      }

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

  // Hold the latest handler in a ref so the global listener binds once instead
  // of churning on every render (live row updates re-render frequently).
  const pickerKeyDownRef = useRef(handlePickerKeyDown);
  pickerKeyDownRef.current = handlePickerKeyDown;
  useEffect(() => {
    const handler = (event: KeyboardEvent) => pickerKeyDownRef.current(event);
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  // Settings must be reachable even when the daemon/IPC is unreachable: that is
  // exactly when the user needs to fix the binary path or SSH host. So the
  // settings view wins before the unready boot screen returns.
  if (view !== "settings" && state.status !== "ready") {
    return (
      <main className="sidebar">
        <div className="boot-state" aria-live="polite">
          <span className="boot-spinner" aria-hidden="true" />
          <h1>{state.status === "loading" ? "Connecting" : "Can’t reach agentscan"}</h1>
          <p>{state.status === "failed" ? state.message : "Waiting for the daemon…"}</p>
          {/* Always offer a path into settings: a hung "loading" (e.g. a stalled
              profile/SSH preflight) otherwise traps the user with no way to fix
              the binary path or host. */}
          <button type="button" onClick={() => setView("settings")}>
            Open settings
          </button>
        </div>
      </main>
    );
  }

  if (view === "settings") {
    // The profile list comes from profileState (the live source of truth) so
    // add/delete/switch are reflected immediately, even while a kept-ready
    // `state` still describes the previous profile during a reload. Preflight is
    // only trusted when the resolved state matches the active profile.
    // Preflight is only trusted when the resolved state matches the active
    // source; otherwise it describes the previous one mid-switch.
    const preflight =
      state.status === "ready" && state.runnerKey === runnerKey ? state.preflight : null;
    const preflightTone = !preflight
      ? state.status === "failed"
        ? "error"
        : "unknown"
      : preflight.ok
        ? "idle"
        : "error";
    const preflightLabel = !preflight
      ? state.status === "failed"
        ? "Unreachable"
        : "Checking"
      : preflight.ok
        ? "Ready"
        : "Unavailable";
    const preflightDetail = !preflight
      ? state.status === "failed"
        ? "Can’t reach agentscan"
        : "Probing agentscan…"
      : preflight.ok
        ? `${preflight.binary} · ${preflight.version ?? "ready"}`
        : (preflight.error ?? "agentscan unavailable");
    // The source rail only earns its space once there's more than the built-in
    // local source; with a single source it just duplicates the detail card. So
    // hide it then and offer a quiet "add remote" affordance instead.
    const hasMultipleSources = profileState.profiles.length > 1;
    // Shared by both header layouts (see adaptive header below).
    const detailActions = (
      <div className="detail-actions">
        {activeProfile.kind === "ssh" ? (
          <button className="ghost-button danger" type="button" onClick={deleteActiveProfile}>
            Delete
          </button>
        ) : null}
        <button
          className="ghost-button"
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
    );

    return (
      <main className="sidebar settings-view">
        <header className="topbar settings-topbar">
          <button
            className="icon-button back"
            type="button"
            aria-label="Back to picker"
            onClick={() => setView("picker")}
          >
            {"←"}
          </button>
          <h1>Settings</h1>
        </header>

        <div className="settings-scroll">
          {hasMultipleSources ? (
          <section className="settings-section" aria-label="Sources">
            <div className="section-title">
              <span>Sources</span>
              <button className="ghost-button add-source" type="button" onClick={addSshProfile}>
                {"+ Remote"}
              </button>
            </div>
            <div className="source-rail">
              {profileState.profiles.map((profile) => {
                const isActive = profile.id === activeProfile.id;
                return (
                  <button
                    aria-pressed={isActive}
                    className={`source-card${isActive ? " active" : ""}`}
                    key={profile.id}
                    type="button"
                    onClick={() => selectProfile(profile.id)}
                  >
                    <span
                      className="source-card-mark"
                      data-kind={profile.kind}
                      aria-hidden="true"
                    >
                      {profile.kind === "ssh" ? "⇆" : "⌂"}
                    </span>
                    <span className="source-card-text">
                      <span className="source-card-name">{profile.name}</span>
                      <span className="source-card-sub">{sourceLabel(profile)}</span>
                    </span>
                    <span className={`kind-chip ${profile.kind}`}>
                      {profile.kind === "ssh" ? "remote" : "local"}
                    </span>
                  </button>
                );
              })}
            </div>
          </section>
          ) : null}

          <section className="settings-section" aria-label="Configuration">
            {/* Adaptive header: with the rail visible the identity already shows
                there, so use a CONFIGURATION section title (actions on the right,
                mirroring SOURCES). With the rail hidden, keep the chip + name in
                the card — it's the only identity on screen. */}
            {hasMultipleSources ? (
              <div className="section-title">
                <span>Configuration</span>
                {detailActions}
              </div>
            ) : null}
            <div className="source-detail">
              {!hasMultipleSources ? (
                <div className="detail-head">
                  <div className="detail-head-title">
                    <span className={`kind-chip ${activeProfile.kind}`}>
                      {activeProfile.kind === "ssh" ? "remote" : "local"}
                    </span>
                    <h2>{activeProfile.name}</h2>
                  </div>
                  {detailActions}
                </div>
              ) : null}

              <div className="detail-status" data-tone={preflightTone}>
                <span
                  className={`status-dot${preflightTone === "unknown" ? " pulsing" : ""}`}
                  data-tone={preflightTone}
                  aria-hidden="true"
                />
                <span className="detail-status-text">
                  <strong>{preflightLabel}</strong>
                  <span className="mono detail-status-detail">{preflightDetail}</span>
                </span>
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

              <div className="field-grid">
                <label className="field">
                  <span className="field-label">Name</span>
                  <input
                    value={profileNameDraft}
                    onChange={(event) => setProfileNameDraft(event.target.value)}
                  />
                </label>
                {activeProfile.kind === "ssh" ? (
                  <label className="field">
                    <span className="field-label">SSH host</span>
                    <input
                      value={sshHostDraft}
                      onChange={(event) => setSshHostDraft(event.target.value)}
                      placeholder="user@host"
                      spellCheck={false}
                    />
                  </label>
                ) : null}
                {activeProfile.kind === "ssh" ? (
                  <label className="field">
                    <span className="field-label">Remote client tty</span>
                    <input
                      value={sshClientTtyDraft}
                      onChange={(event) => setSshClientTtyDraft(event.target.value)}
                      placeholder="Best-effort"
                      spellCheck={false}
                    />
                  </label>
                ) : null}
                <label className="field">
                  <span className="field-label">agentscan binary</span>
                  <input
                    value={settingsDraft.binaryPath}
                    onChange={(event) =>
                      setSettingsDraft((current) => ({
                        ...current,
                        binaryPath: event.target.value,
                      }))
                    }
                    placeholder="Auto-detect"
                    spellCheck={false}
                  />
                </label>
              </div>

              <div className="env-block">
                <div className="env-head">
                  <span className="field-label">Environment</span>
                  <span className="env-count">{settingsDraft.env.length}</span>
                </div>
                <div className="env-list">
                  {settingsDraft.env.length === 0 ? (
                    <p className="env-empty">No variables — agentscan runs with the inherited env.</p>
                  ) : (
                    settingsDraft.env.map((variable, index) => (
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
                        <span className="env-eq" aria-hidden="true">
                          =
                        </span>
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
                          className="env-remove"
                          type="button"
                          aria-label="Remove variable"
                          onClick={() => removeEnvironmentVariable(index)}
                        >
                          {"×"}
                        </button>
                      </div>
                    ))
                  )}
                </div>
                <button className="ghost-button env-add" type="button" onClick={addEnvironmentVariable}>
                  {"+ Add variable"}
                </button>
              </div>
            </div>
            {!hasMultipleSources ? (
              <button className="add-remote-cta" type="button" onClick={addSshProfile}>
                {"+ Add a remote source"}
              </button>
            ) : null}
          </section>

          <section className="settings-section" aria-label="Appearance">
            <div className="section-title">
              <span>Appearance</span>
            </div>
            <div className="theme-toggle" role="group" aria-label="Theme">
              {(["dark", "light", "system"] as ThemePreference[]).map((option) => (
                <button
                  className={`theme-option${themePref === option ? " active" : ""}`}
                  key={option}
                  type="button"
                  aria-pressed={themePref === option}
                  onClick={() => setThemePref(option)}
                >
                  {option === "system" ? "System" : option === "light" ? "Light" : "Dark"}
                </button>
              ))}
            </div>
          </section>

          <section className="settings-section" aria-label="Debug log">
            <div className="section-title">
              <button
                className="collapse-toggle"
                type="button"
                aria-expanded={isDebugOpen}
                onClick={() => setIsDebugOpen((open) => !open)}
              >
                <span className={`collapse-caret${isDebugOpen ? " open" : ""}`} aria-hidden="true">
                  {"›"}
                </span>
                <span className="collapse-label">Debug log</span>
                <span className="env-count">{debugEntries.length}</span>
              </button>
              {isDebugOpen ? (
                <button className="ghost-button" type="button" onClick={() => setDebugEntries([])}>
                  Clear
                </button>
              ) : null}
            </div>
            {isDebugOpen ? <DebugLog entries={debugEntries} /> : null}
          </section>
        </div>
      </main>
    );
  }

  // Unreachable: the guards above return for every not-ready picker case. This
  // narrows `state` to "ready" so the picker can read state.picker/preflight.
  if (state.status !== "ready") {
    return null;
  }

  return (
    <main className="sidebar">
      <header className="topbar">
        <div className="search-field">
          <span className="search-icon" aria-hidden="true">
            {"⌕"}
          </span>
          <input
            aria-label="Search agents"
            value={pickerFilter}
            onChange={(event) => setPickerFilter(event.target.value)}
            placeholder="Search agents"
          />
          {pickerFilter.trim() ? (
            <button
              className="search-clear"
              type="button"
              aria-label="Clear search"
              onClick={() => setPickerFilter("")}
            >
              {"×"}
            </button>
          ) : null}
        </div>
        <button
          className="icon-button"
          type="button"
          aria-label="Refresh"
          title="Refresh"
          onClick={refreshPickerRows}
          disabled={isRefreshing}
        >
          <span className={isRefreshing ? "spin" : undefined}>{"↻"}</span>
        </button>
      </header>

      {displayLiveState.status !== "online" ? <LiveStrip state={displayLiveState} /> : null}

      {activation.status === "failed" ? (
        <div className="inline-error" role="alert">
          {activation.message}
        </div>
      ) : null}

      <div className="picker-scroll" aria-label="Agents" tabIndex={-1}>
        <GroupedPicker
          activation={activation}
          filterQuery={pickerFilter}
          groups={pickerGroups}
          selectedPaneId={selectedPaneId}
          state={pickerDataState}
          totalRows={allPickerRows.length}
          onActivate={activateSelectedRow}
          onClearFilter={() => setPickerFilter("")}
          onSelect={(row) => setSelectedPaneId(row.pane_id)}
        />
      </div>

      <footer className="bottombar">
        <div className="source-switcher" ref={sourceMenuRef}>
          <button
            className="source-trigger"
            type="button"
            aria-haspopup="menu"
            aria-expanded={isSourceMenuOpen}
            onClick={() => setIsSourceMenuOpen((open) => !open)}
            title={statusText}
          >
            <span
              className="status-dot"
              data-tone={sourceStatusTone}
              aria-hidden="true"
            />
            <span className="source-label">{sourceLabel(activeProfile)}</span>
            <span
              className={`source-caret${isSourceMenuOpen ? " open" : ""}`}
              aria-hidden="true"
            >
              {"›"}
            </span>
          </button>
          {isSourceMenuOpen ? (
            <div className="source-menu" role="menu">
              {sourceProfiles.map((profile) => {
                const isActive = profile.id === activeProfile.id;
                return (
                  <button
                    className={`source-option${isActive ? " active" : ""}`}
                    key={profile.id}
                    role="menuitemradio"
                    aria-checked={isActive}
                    type="button"
                    onClick={() => {
                      selectProfile(profile.id);
                      setIsSourceMenuOpen(false);
                    }}
                  >
                    <span className="source-check" aria-hidden="true">
                      {isActive ? "✓" : ""}
                    </span>
                    <span className="source-option-text">
                      <span className="source-option-name">{profile.name}</span>
                      <span className="source-option-sub">{sourceLabel(profile)}</span>
                    </span>
                  </button>
                );
              })}
              <div className="source-menu-divider" role="separator" />
              <button
                className="source-option manage"
                role="menuitem"
                type="button"
                onClick={() => {
                  setIsSourceMenuOpen(false);
                  setView("settings");
                }}
              >
                <span className="source-check" aria-hidden="true">
                  {"⚙"}
                </span>
                <span className="source-option-label">Manage sources…</span>
              </button>
            </div>
          ) : null}
        </div>
        <button
          className="icon-button"
          type="button"
          aria-label="Settings"
          title="Settings"
          onClick={() => setView("settings")}
        >
          {"⚙"}
        </button>
      </footer>
    </main>
  );
}

function applyLivePickerEvent(
  event: LivePickerEvent,
  runnerKey: string,
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
      // Only persist rows into the state blob that still belongs to this
      // subscription's runner, matching the activeReadyState/refresh guards.
      current.status === "ready" && current.runnerKey === runnerKey
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
    // A retrying offline is transient (daemon will reconnect), so keep any rows
    // we already have; a terminal offline means the current rows belong to a
    // dead subscription and must not stay selectable.
    if (event.retrying) {
      markPickerFailedIfEmpty(setState, runnerKey, event.message);
    } else {
      markPickerFailed(setState, runnerKey, event.message);
    }
    return;
  }

  if (event.kind === "shutdown") {
    setLiveState({ status: "shutdown", message: event.message });
    markPickerFailed(setState, runnerKey, event.message);
    return;
  }

  setLiveState({
    status: "fatal",
    message: event.message,
    diagnostics: event.diagnostics,
  });
  markPickerFailed(setState, runnerKey, event.message);
}

function markPickerFailedIfEmpty(
  setState: (value: SetStateAction<LoadState>) => void,
  runnerKey: string,
  message: string,
) {
  setState((current) => {
    if (
      current.status !== "ready" ||
      current.runnerKey !== runnerKey ||
      current.picker.status === "ready"
    ) {
      return current;
    }

    return { ...current, picker: { status: "failed", message } };
  });
}

// Whether a live event mutates the picker rows: a fresh snapshot ("rows") or a
// terminal event that clears them (fatal/shutdown/terminal-offline). Used to
// bump the live sequence so an in-flight manual refresh detects the change and
// neither overwrites fresh rows nor resurrects rows a terminal event cleared.
function liveEventChangesPickerRows(event: LivePickerEvent): boolean {
  switch (event.kind) {
    case "rows":
    case "shutdown":
    case "fatal":
      return true;
    case "offline":
      return !event.retrying;
    default:
      return false;
  }
}

// Terminal live events (fatal/shutdown/terminal-offline) mean any rows we are
// showing belong to a dead subscription. Replace them with the failure even if
// the picker is currently "ready", so stale rows can't be selected and focused
// against panes that may no longer exist.
function markPickerFailed(
  setState: (value: SetStateAction<LoadState>) => void,
  runnerKey: string,
  message: string,
) {
  setState((current) => {
    if (current.status !== "ready" || current.runnerKey !== runnerKey) {
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

function LiveStrip({ state }: { state: LiveConnectionState }) {
  const tone = state.status === "fatal" || state.status === "offline" ? "error" : "warn";

  return (
    <div className={`live-strip ${tone}`} aria-live="polite">
      <span className="status-dot" data-tone={tone === "error" ? "error" : "busy"} />
      <span className="live-label">{liveStateLabel(state)}</span>
      <span className="live-message">{state.message}</span>
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

function loadStoredTheme(): ThemePreference {
  try {
    const value = window.localStorage.getItem(THEME_STORAGE_KEY);
    if (value === "dark" || value === "light" || value === "system") {
      return value;
    }
  } catch {
    // localStorage unavailable; fall back to following the OS.
  }
  return "system";
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
        name: "Default",
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
      clientTty:
        typeof profile.clientTty === "string" ? profile.clientTty.trim() : "",
      runner,
      // Default to enabled so profiles persisted before the `enabled` field (or
      // partial profiles missing it) remain selectable; only an explicit false
      // disables a profile.
      enabled: profile.enabled !== false,
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

// Stable string identity of a profile's full runner configuration. Used to
// invalidate resolved preflight/picker state when the target changes, including
// same-profile settings edits (which keep the same id).
function runnerKeyForProfile(profile: DesktopProfileConfig): string {
  return JSON.stringify(runnerSettingsForProfile(profile));
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

    // Must be a POSIX shell identifier: names are interpolated unquoted into
    // the remote SSH command, so spaces/hyphens/metacharacters are rejected.
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name)) {
      errors.push(`Environment row ${index + 1} name must be a valid shell identifier.`);
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

function projectOf(row: PickerRow): string {
  const tag = row.location_tag.trim();
  const session = tag.split(":", 1)[0]?.trim();
  return session || "ungrouped";
}

function paneSuffix(row: PickerRow): string {
  const tag = row.location_tag.trim();
  const colon = tag.indexOf(":");
  return colon >= 0 ? tag.slice(colon + 1) : "";
}

// Group rows by tmux session (the project), preserving first-seen order both
// across groups and within each group so keyboard nav matches what's rendered.
function groupRowsByProject(rows: PickerRow[]): PickerGroup[] {
  const groups: PickerGroup[] = [];
  const byProject = new Map<string, PickerGroup>();

  for (const row of rows) {
    const project = projectOf(row);
    let group = byProject.get(project);
    if (!group) {
      group = { project, rows: [] };
      byProject.set(project, group);
      groups.push(group);
    }
    group.rows.push(row);
  }

  return groups;
}

function statusTone(kind: string): string {
  switch (kind) {
    case "busy":
      return "busy";
    case "idle":
      return "idle";
    case "error":
      return "error";
    default:
      return "unknown";
  }
}

// Footer label for an agentscan source: the local runner, or a remote keyed by
// its SSH host (falling back to the profile name when the host isn't set yet).
function sourceLabel(profile: DesktopProfileConfig): string {
  if (profile.kind === "ssh") {
    const host = profile.host.trim();
    return host ? `agentscan @ ${host}` : profile.name;
  }
  return "agentscan";
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

// Persistent-window model: the global hotkey raises/focuses the window; it
// never toggles it away.
async function raisePickerWindow() {
  await enqueueWindowOperation(async () => {
    const appWindow = getCurrentWindow();
    await placePickerWindow();
    await appWindow.show();
    await appWindow.setFocus();
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

function GroupedPicker({
  activation,
  filterQuery,
  groups,
  selectedPaneId,
  state,
  totalRows,
  onActivate,
  onClearFilter,
  onSelect,
}: {
  activation: PickerActivation;
  filterQuery: string;
  groups: PickerGroup[];
  selectedPaneId: string | null;
  state: PickerState;
  totalRows: number;
  onActivate: (row: PickerRow) => void;
  onClearFilter: () => void;
  onSelect: (row: PickerRow) => void;
}) {
  const rowCount = groups.reduce((total, group) => total + group.rows.length, 0);

  if (state.status === "loading" && rowCount === 0) {
    return <p className="empty-note">Loading agents…</p>;
  }

  if (state.status === "failed") {
    return (
      <div className="error-state" role="alert">
        <h3>Unable to load agents</h3>
        <p>{state.message}</p>
      </div>
    );
  }

  if (totalRows > 0 && rowCount === 0 && filterQuery.trim()) {
    return (
      <div className="empty-filter-state">
        <p>No agents match “{filterQuery.trim()}”.</p>
        <button className="ghost-button" type="button" onClick={onClearFilter}>
          Clear search
        </button>
      </div>
    );
  }

  if (rowCount === 0) {
    return <p className="empty-note">No agents detected.</p>;
  }

  return (
    <div className="picker-groups">
      {groups.map((group) => (
        <section className="picker-group" key={group.project}>
          <h2 className="group-header">{group.project}</h2>
          <ul className="agent-list">
            {group.rows.map((row) => {
              const isSelected = row.pane_id === selectedPaneId;
              // The pane tmux is currently focused on — distinct from the
              // selection cursor, so it gets its own accent treatment.
              const isActive = row.is_active;
              const isFocusing =
                activation.status === "running" && activation.paneId === row.pane_id;
              const logo = providerLogo(row.provider);
              return (
                <li
                  aria-selected={isSelected}
                  aria-current={isActive ? "true" : undefined}
                  className={`agent-row${isSelected ? " selected" : ""}${isActive ? " active" : ""}`}
                  key={`${row.key}-${row.pane_id}`}
                  onClick={() => {
                    // Single-click selects and switches the active tmux pane.
                    // Enter still activates the keyboard selection; double-click
                    // is gone (redundant under single-click activation).
                    onSelect(row);
                    onActivate(row);
                  }}
                  title={`${row.display_label} · ${row.provider ?? "unknown"} · ${row.location_tag}`}
                >
                  <span
                    className={`status-dot${isFocusing ? " pulsing" : ""}`}
                    data-tone={isFocusing ? "busy" : statusTone(row.status.kind)}
                    aria-hidden="true"
                  />
                  {logo ? (
                    <img className="provider-logo" src={logo} alt="" aria-hidden="true" />
                  ) : (
                    <span className="provider-logo provider-logo-empty" aria-hidden="true" />
                  )}
                  <span className="agent-label">{row.display_label}</span>
                  <span className="agent-suffix">{paneSuffix(row)}</span>
                  <kbd>{row.key}</kbd>
                </li>
              );
            })}
          </ul>
        </section>
      ))}
    </div>
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
