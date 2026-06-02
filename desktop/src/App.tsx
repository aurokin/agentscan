import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emitTo, listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { register, unregister } from "@tauri-apps/plugin-global-shortcut";
import { Result, useAtomSet, useAtomValue } from "@effect-atom/atom-react";
import { providerLogo, type LogoTheme } from "./providerLogos";
import { configureAtom, liveStateAtom, reconnectAtom, startAtom } from "./effect/atoms";
import type { ConnectionStatus, LiveState, PickerRow } from "./effect/types";
import logoUrl from "./assets/agentscan-logo.png";

const PICKER_HOTKEY = "CommandOrControl+Shift+A";
const SETTINGS_STORAGE_KEY = "agentscan.desktop.localRunnerSettings";
const PROFILES_STORAGE_KEY = "agentscan.desktop.profiles";
// Keep in sync with the pre-paint theme script in index.html.
const THEME_STORAGE_KEY = "agentscan.desktop.theme";
// macOS "glass": vibrancy backdrop (Rust) + a translucent surface tint (CSS).
const GLASS_STORAGE_KEY = "agentscan.desktop.glass";
const SURFACE_ALPHA_STORAGE_KEY = "agentscan.desktop.surfaceAlpha";
// Dock layout preference: "auto" follows the window's aspect ratio; "vertical"/
// "horizontal" pin the layout and snap the window to that shape.
const ORIENTATION_STORAGE_KEY = "agentscan.desktop.orientation";
// Tint alpha floor of 0.20 caps transparency at 80% (the slider reads 1 - alpha):
// the surface always keeps a little tint over the native vibrancy frost, so the UI
// never washes out fully even at the most transparent setting.
const SURFACE_ALPHA_MIN = 0.2;
const SURFACE_ALPHA_MAX = 1;
// First-run default: 0.50 alpha == 50% transparency (the slider reads 1 - alpha),
// a balanced frosted look — clearly glassy but still a substantial surface tint.
const SURFACE_ALPHA_DEFAULT = 0.5;
// "How see-through is the surface" as a 0..1 scalar (0 frosted/solid, 1 fully
// clear) that adaptive tokens interpolate against. Mirrors the slider math.
const glassClearFor = (alpha: number) =>
  (SURFACE_ALPHA_MAX - alpha) / (SURFACE_ALPHA_MAX - SURFACE_ALPHA_MIN);
const setGlassClear = (clear: number) => {
  document.documentElement.style.setProperty("--glass-clear", clear.toFixed(3));
};
const DEBUG_LOG_LIMIT = 80;
const LOCAL_PROFILE_ID = "local";
// Window min-size floors, applied at runtime per orientation. The vertical pair
// mirrors the startup floor in tauri.{macos.,}conf.json; horizontal drops the
// height floor so the bar can shrink to dock height instead of a tall slab.
const WINDOW_MIN_WIDTH = 220;
const WINDOW_MIN_HEIGHT_VERTICAL = 520;
const WINDOW_MIN_HEIGHT_HORIZONTAL = 96;
// Max-size caps per pinned orientation: vertical stays a strip (width capped, height
// free), horizontal stays a bar (height capped, width free). "auto" clears the cap.
// The free axis uses a value larger than any display so it reads as unbounded.
const WINDOW_MAX_WIDTH_VERTICAL = 520;
const WINDOW_MAX_HEIGHT_HORIZONTAL = 200;
const WINDOW_MAX_UNBOUNDED = 10000;

// Per-row picker hotkeys are triggered with Control rather than Command. The
// default key set overlaps macOS ⌘ shortcuts — ⌘Q quits, ⌘C/V/X are clipboard,
// ⌘F/Z/R are find/undo/refresh — so ⌘ would be hostile. Control has no such
// collisions (only emacs text-nav in inputs, which we override on a match).
const IS_MAC =
  typeof navigator !== "undefined" && /Mac|iP(hone|ad|od)/.test(navigator.platform);
const HOTKEY_MODIFIER_LABEL = IS_MAC ? "⌃" : "Ctrl ";

let hotkeyOperationQueue = Promise.resolve();
let windowOperationQueue = Promise.resolve();
// Serializes set_window_glass invokes so a fast off→on toggle can't land its
// native calls out of order and leave the blur layer out of sync with the UI.
let glassOperationQueue = Promise.resolve();

// Live-picker subscription state (connection status + rows + epoch fencing + the
// reconnect/latch policy) is owned by the Effect LiveConnection service. This
// component drives it via configure/reconnect/start and observes liveStateAtom.
const DEFAULT_LIVE_STATE: LiveState = {
  connection: { status: "connecting", message: "Starting live client" },
  rows: [],
};

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

// Which window this React tree drives. The dock (window label "main") renders the
// picker; the settings window (label "settings") renders the settings panel. Both
// run the same component, gated on this mode plus cross-window pref sync.
type ShellMode = "dock" | "settings";

// The dock can sit as a tall narrow strip (vertical, the default) or a short wide
// bar (horizontal). Orientation is derived from the window's own aspect ratio, so
// dragging it wide-and-short flips the layout axis with no explicit control.
type Orientation = "vertical" | "horizontal";

// Layout preference. "auto" derives orientation live from the window shape (the
// responsive default); the other two pin the layout and snap the window to match.
type OrientationPreference = "auto" | Orientation;

type ThemePreference = "dark" | "light" | "system";

// Separate webview windows don't share React state or the localStorage "storage"
// event, so prefs the user changes in one window are mirrored to the other over
// Tauri's event bus. Only user actions emit; the listener applies remote changes
// without re-emitting, so dock -> settings -> dock can't loop.
const PREFS_SYNC_EVENT = "agentscan:prefs-sync";
type PrefsSync =
  | { kind: "theme"; theme: ThemePreference }
  | { kind: "orientation"; orientation: OrientationPreference }
  | { kind: "glass"; enabled: boolean; alpha: number }
  | { kind: "profiles" }
  // Dock -> settings: the dock's resolved preflight (with the runnerKey it
  // describes) so the settings card can reuse it instead of probing itself.
  // `status` mirrors the dock's LoadState discriminant so the card can reproduce
  // its tones: "ready" carries `preflight`; "loading" reads as Checking; "failed"
  // (dock IPC error) reads as Unreachable. `preflight` is non-null only on "ready".
  | {
      kind: "preflight";
      status: LoadState["status"];
      runnerKey: string;
      preflight: AgentscanPreflight | null;
    }
  // Settings -> dock: ask the dock to re-emit its current preflight. emitTo has
  // no replay, so a settings window shown after the dock probed would otherwise
  // miss the result; it requests one on show to reconcile.
  | { kind: "preflight-request" };

// The dock's resolved preflight as held by the settings window, mirrored from the
// "preflight" sync above (kind dropped). The settings card reproduces its tones from
// `status` + `preflight`, guarded by `runnerKey` against its own active runner.
type SyncedPreflight = {
  status: LoadState["status"];
  runnerKey: string;
  preflight: AgentscanPreflight | null;
};

// Collapse a preference to the concrete theme in effect, resolving "system" from
// the OS appearance. Used to pick per-theme logo variants.
function resolveThemeMode(pref: ThemePreference): LogoTheme {
  if (pref !== "system") {
    return pref;
  }
  try {
    return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  } catch {
    return "dark";
  }
}

// Wider than tall reads as a horizontal bar; otherwise the default vertical strip.
// Base CSS is vertical, so an unset/indeterminate result harmlessly stays vertical.
function orientationForViewport(): Orientation {
  return window.innerWidth > window.innerHeight ? "horizontal" : "vertical";
}

type PickerGroup = {
  project: string;
  rows: PickerRow[];
};

// Preflight-only resolved state. Picker rows + live connection status now live in
// the LiveConnection service (liveStateAtom); this `state` only tracks the CLI
// probe + profile list for the dock and the settings card.
type LoadState =
  | { status: "loading" }
  | {
      status: "ready";
      // The runner config this resolved state describes (see runnerKeyForProfile).
      // `state` lags the active runner by one async cycle on a switch or settings
      // apply, so consumers compare this to the active runnerKey before trusting
      // preflight for live decisions.
      runnerKey: string;
      profiles: DesktopProfile[];
      preflight: AgentscanPreflight;
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

function App({ mode }: { mode: ShellMode }) {
  const [state, setState] = useState<LoadState>({ status: "loading" });
  // The settings window never runs its own preflight; instead it reuses the dock's
  // resolved result, mirrored over the event bus (see the preflight sync below). This
  // avoids a second `ssh … --version` for remote profiles (an extra round-trip and a
  // possible duplicate passphrase prompt) every time Settings is opened, and keeps the
  // card current even while the window is visible-but-unfocused. Always null in the dock.
  const [syncedPreflight, setSyncedPreflight] = useState<SyncedPreflight | null>(null);
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
  // Live connection (status + rows) is owned by the LiveConnection service. The dock
  // observes liveStateAtom and drives the service via configure/reconnect/start. The
  // settings window mounts these too (separate webview/runtime) but never enables a
  // target, so its supervisor stays idle.
  const liveResult = useAtomValue(liveStateAtom);
  const live: LiveState = Result.getOrElse(liveResult, () => DEFAULT_LIVE_STATE);
  const configureLive = useAtomSet(configureAtom);
  const startLive = useAtomSet(startAtom);
  const reconnectLive = useAtomSet(reconnectAtom);
  const [pickerFilter, setPickerFilter] = useState("");
  // Layout axis, seeded from the current window shape and kept in sync on resize.
  const [orientation, setOrientation] = useState<Orientation>(orientationForViewport);
  // Layout preference: "auto" follows `orientation`; a pinned value overrides it.
  const [orientationPref, setOrientationPref] =
    useState<OrientationPreference>(loadStoredOrientation);
  const effectiveOrientation: Orientation =
    orientationPref === "auto" ? orientation : orientationPref;
  // The summon hotkey is registered once but must place by the LIVE orientation, so a
  // pinned/auto horizontal bar is re-summoned as a bar, not snapped to the vertical
  // strip. A render-synced ref keeps the registered handler current.
  const summonPlacementRef = useRef<() => Promise<void>>(placePickerWindow);
  summonPlacementRef.current =
    effectiveOrientation === "horizontal" ? placeBarWindow : placePickerWindow;
  // Set once the orientation-sizing effect has scheduled the initial dock placement, so
  // it places on first mount (and every pinned reshape) but never re-snaps an "auto"
  // window on a later drag.
  const didInitialPlaceRef = useRef(false);
  // Footer source switcher: which agentscan we're listening to (local vs a
  // remote over SSH). Open state for the inline dropdown.
  const [isSourceMenuOpen, setIsSourceMenuOpen] = useState(false);
  const sourceMenuRef = useRef<HTMLDivElement | null>(null);
  // Debug log is a diagnostic panel — collapsed by default to keep Settings calm.
  const [isDebugOpen, setIsDebugOpen] = useState(false);
  // Appearance: dark / light / system (system follows the OS).
  const [themePref, setThemePref] = useState<ThemePreference>(loadStoredTheme);
  // Concrete theme in effect, kept in sync by the theme effect; drives per-theme
  // logo variant selection. Seeded so first paint picks the right logos.
  const [resolvedTheme, setResolvedTheme] = useState<LogoTheme>(() =>
    resolveThemeMode(loadStoredTheme()),
  );
  // macOS glass: the vibrancy backdrop toggle and the tint opacity over it. The
  // toggle is only surfaced on macOS; elsewhere these stay inert.
  const [glassEnabled, setGlassEnabled] = useState<boolean>(loadStoredGlass);
  const [surfaceAlpha, setSurfaceAlpha] = useState<number>(loadStoredSurfaceAlpha);
  // The glass toggle's async resolution sets `--glass-clear` from the latest
  // alpha; reading it through a render-synced ref keeps the toggle effect off
  // surfaceAlpha's dep list (so a slider tick can't re-fire the native call).
  const surfaceAlphaRef = useRef(surfaceAlpha);
  surfaceAlphaRef.current = surfaceAlpha;
  // Tracks the active runner config so in-flight refreshes/focus can detect a
  // profile switch OR a settings change and discard results from the previous
  // target. Updated synchronously during render (below) so async completions
  // never observe a stale key through a late-running effect.
  const activeRunnerKeyRef = useRef(runnerKey);
  activeRunnerKeyRef.current = runnerKey;
  const [selectedPaneId, setSelectedPaneId] = useState<string | null>(null);
  // The focused pane id we last *observed as visible*. We follow focus only when
  // this value changes (a genuine focus move), not when it merely reappears, so a
  // manual j/k/click pick survives a search filter being applied and cleared, and
  // a focus move to a hidden pane is still followed once that pane becomes
  // visible again.
  const prevFocusedPaneIdRef = useRef<string | null>(null);
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
  // Render-synced mirror so the settings focus-reconcile listener can read the latest
  // dirty state without re-subscribing on every keystroke.
  const isSettingsDirtyRef = useRef(isSettingsDirty);
  isSettingsDirtyRef.current = isSettingsDirty;

  useEffect(() => {
    // Only the dock probes. It gates the live picker, so it must run preflight; the
    // settings window reuses the dock's resolved result over the event bus instead of
    // firing its own (which for SSH would be a second `ssh … --version` — an extra
    // round-trip and a possible duplicate passphrase prompt). See the preflight sync.
    if (mode !== "dock") {
      return;
    }
    let cancelled = false;

    async function loadShellState() {
      // Keep any existing ready state so a profile/runner reload stays in the
      // picker (gated to a "Switching profile…" state via activeReadyState)
      // instead of dropping to the boot screen. Only the very first load, with
      // no ready state yet, shows the boot "Connecting" screen.
      setState((current) => (current.status === "ready" ? current : { status: "loading" }));

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
          });
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
          setState({ status: "ready", runnerKey, profiles, preflight });
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
    // Keyed on runnerKey (the active runner's identity), NOT profileState/runnerSettings:
    // editing or deleting an INACTIVE profile in Settings syncs a fresh profileState but
    // leaves the active runner unchanged, so re-running here would needlessly re-probe
    // (an extra ssh --version / passphrase prompt) for a target that didn't change.
    // runnerKey is a stable string, so React skips this effect when its value is unchanged;
    // activeProfile/runnerSettings are fully determined by it where the effect reads them.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [runnerKey, mode]);

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

  // Drive the LiveConnection service to the active target. The service owns the
  // subscription, epoch fencing, reconnect/latch backoff, and recovery; this just
  // tells it WHICH runner to track and WHETHER it's ready (preflight ok + valid
  // profile + dock). configure dedupes on runnerKey + enabled, so an inactive-profile
  // edit that leaves the active runner unchanged doesn't re-arm the worker.
  useEffect(() => {
    if (mode !== "dock") {
      return;
    }
    configureLive({ settings: runnerSettings, runnerKey, enabled: liveReady });
  }, [mode, runnerKey, liveReady, runnerSettings, configureLive]);

  useEffect(() => {
    // The global summon hotkey belongs to the dock; registering it in both windows
    // would double-bind the shortcut.
    if (mode !== "dock") {
      return;
    }

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
              void raisePickerWindow(summonPlacementRef.current);
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
  }, [mode]);

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
  // The picker rows + connection status now come from the LiveConnection service
  // (live), not from `state`. Both lag the active profile the same way preflight
  // does, so while the resolved state's runnerKey doesn't match the active runner
  // (a switch in flight) we show a neutral "loading"/"Switching…" view rather than
  // the previous profile's stale rows/offline banner.
  const displayConnection: ConnectionStatus = activeReadyState
    ? live.connection
    : { status: "connecting", message: "Switching profile…" };
  // Spin the reconnect affordance while the live client is (re)connecting.
  const isReconnecting =
    displayConnection.status === "connecting" || displayConnection.status === "reconnecting";
  // A failed focus re-arms the live client (activateSelectedRow's catch) to drop the
  // now-dead pane. Until the fresh snapshot lands, `live.rows` still carries that stale
  // row — reconnecting preserves rows to avoid a flicker on a healthy manual reconnect —
  // so gate the list to "loading" during THIS recovery, matching the old refresh which
  // set picker→loading, instead of leaving the known-dead row clickable and instantly
  // re-triggerable. Keyed on activation.status==="failed" so a manual reconnect (idle)
  // keeps its rows; it self-clears once rows refresh and isReconnecting goes false.
  const recoveringFromFailedActivation = activation.status === "failed" && isReconnecting;
  const pickerDataState: PickerState =
    activeReadyState && !recoveringFromFailedActivation
      ? pickerStateFromLive(live)
      : { status: "loading" };
  const allPickerRows =
    pickerDataState.status === "ready" ? pickerDataState.rows : [];
  // Server-level count echoed on every row; >1 means focus-following is
  // best-effort, so we warn that the live-pane highlight may not be reliable.
  const attachedClientCount = allPickerRows[0]?.attached_client_count ?? 0;
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
  // Derive from the unfiltered rows: the search filter must not change the focus
  // signal, or hiding the focused row would null it and spuriously reset
  // follow-state, yanking a manual selection when the filter is cleared.
  //
  // Prefer the collapsed `is_focused` signal. If no row carries it — an older or
  // remote `agentscan` (schema < 5) that doesn't emit the field — fall back to
  // the first `is_active` pane so the picker still defaults to/highlights a live
  // pane instead of going dark.
  const focusedPaneId =
    allPickerRows.find((row) => row.is_focused)?.pane_id ??
    (allPickerRows.some((row) => row.is_focused !== undefined)
      ? null
      : (allPickerRows.find((row) => row.is_active)?.pane_id ?? null));

  useEffect(() => {
    if (pickerStatus === "loading") {
      return;
    }

    // No data at all → clear selection and focus-follow state.
    if (allPickerRows.length === 0) {
      if (selectedPaneId !== null) {
        setSelectedPaneId(null);
      }
      prevFocusedPaneIdRef.current = null;
      return;
    }

    // Filter matched nothing: leave selection and follow-state untouched so
    // clearing the filter restores them. There's no visible row to target now.
    if (pickerRows.length === 0) {
      return;
    }

    const focusedVisible =
      focusedPaneId !== null && pickerRows.some((row) => row.pane_id === focusedPaneId);

    // Follow a genuine focus *move*: we have a prior observed focus value and it
    // changed to a different, now-visible pane. Comparing focus to its own
    // previous value — not to the current selection — is the key to surviving the
    // search filter: applying then clearing a filter doesn't change the focus
    // value, so it never re-snaps over a manual pick. A `null` previous value is
    // first observation / re-init, *not* a move: it must fall through to the
    // selection-validity branch so an already-made manual pick isn't clobbered.
    if (
      focusedVisible &&
      prevFocusedPaneIdRef.current !== null &&
      focusedPaneId !== prevFocusedPaneIdRef.current
    ) {
      prevFocusedPaneIdRef.current = focusedPaneId;
      setSelectedPaneId(focusedPaneId);
      return;
    }

    // Record the focus value once the focused pane is visible: initializing the
    // marker on first observation, or confirming an unchanged value. While it's
    // hidden (filtered) or unknown (null), leave the marker so a pending move is
    // still followed when the pane reappears.
    if (focusedVisible) {
      prevFocusedPaneIdRef.current = focusedPaneId;
    }

    // Keep a valid, visible selection (initial mount with no pick yet, or the
    // selected row was filtered out / vanished). Prefer the focused pane when
    // visible, else the first row. A still-valid selection — including a manual
    // pick made before this effect first ran — is left untouched.
    if (!selectedPaneId || !pickerRows.some((row) => row.pane_id === selectedPaneId)) {
      setSelectedPaneId(focusedVisible ? focusedPaneId! : pickerRows[0].pane_id);
    }
  }, [allPickerRows.length, pickerRows, pickerStatus, selectedPaneId, focusedPaneId]);

  // The pane selection is TARGET-scoped (tmux pane ids collide across hosts/sessions), so
  // it's reset by runnerKey; the search filter / source menu are IDENTITY-scoped UI, reset
  // by the active profile id. Both seeded to the initial values so mount doesn't clobber
  // the selection the selection-keeper effect just established or wipe an opening search.
  const lastSelectionResetRunnerKeyRef = useRef(runnerKey);
  const lastPickerResetProfileIdRef = useRef(profileState.activeProfileId);

  useEffect(() => {
    // Drafts and the transient activation guard reset on any active-profile change
    // (switch OR in-place edit): activeRunnerKeyRef is updated synchronously during
    // render, so resolved data from the previous target is already discarded. Freeing
    // activationInFlightRef lets the new target activate immediately rather than waiting
    // on a stale focus_picker_row's finally. The live client's reconnect/retry policy is
    // owned by the LiveConnection service, which re-targets on the runnerKey change.
    setProfileNameDraft(activeProfile.name);
    setSettingsDraft(activeProfile.runner);
    setSshHostDraft(activeProfile.kind === "ssh" ? activeProfile.host : "");
    setSshClientTtyDraft(activeProfile.kind === "ssh" ? activeProfile.clientTty : "");
    setActivation({ status: "idle" });
    activationInFlightRef.current = false;

    // The pane selection clears whenever the underlying TARGET changes — a profile switch
    // OR an in-place runner/host/binary edit (both move runnerKey). tmux pane ids like %1
    // collide across hosts/sessions, so a carried-over selectedPaneId would otherwise
    // silently highlight/activate a different agent on the new target.
    if (lastSelectionResetRunnerKeyRef.current !== runnerKey) {
      lastSelectionResetRunnerKeyRef.current = runnerKey;
      setSelectedPaneId(null);
      // Re-arm focus-follow so the new target snaps to its own focused pane.
      prevFocusedPaneIdRef.current = null;
    }

    // The search filter and open source menu are identity-scoped UI: clear them only on an
    // actual source switch, not on an in-place edit — otherwise a settings apply (which
    // reloads profileState in this dock window) would wipe a concurrent dock search.
    if (lastPickerResetProfileIdRef.current !== profileState.activeProfileId) {
      lastPickerResetProfileIdRef.current = profileState.activeProfileId;
      setPickerFilter("");
      setIsSourceMenuOpen(false);
    }
  }, [activeProfile]);

  // A wide drag or pinning to horizontal can strand an already-open source menu in
  // the thin bar (where it clips). Close it whenever the layout goes horizontal.
  useEffect(() => {
    if (effectiveOrientation === "horizontal") {
      setIsSourceMenuOpen(false);
    }
  }, [effectiveOrientation]);

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
      setResolvedTheme(resolved);
    };
    apply();

    if (themePref !== "system") {
      return;
    }
    media.addEventListener("change", apply);
    return () => media.removeEventListener("change", apply);
  }, [themePref]);

  // Track the window's aspect ratio; the sidebar renders data-orientation from this
  // state and the horizontal axis overrides in styles.css key off it. Re-deriving on
  // every resize is cheap, and setOrientation no-ops when the axis is unchanged, so a
  // drag that stays vertical never re-renders.
  useEffect(() => {
    const apply = () => setOrientation(orientationForViewport());
    apply();
    window.addEventListener("resize", apply);
    return () => window.removeEventListener("resize", apply);
  }, []);

  // Persist the layout preference. Window shaping is handled by the effect below.
  useEffect(() => {
    try {
      window.localStorage.setItem(ORIENTATION_STORAGE_KEY, orientationPref);
    } catch {
      // Persistence is best-effort; the in-memory preference still applies.
    }
  }, [orientationPref]);

  // Shape and constrain the dock window for the current orientation preference in one
  // race-free sequence ("auto" = free: no cap, no snap). Caps are lifted before min is
  // raised so a larger min can never transiently exceed a stale max; then the real cap
  // is applied and we snap to the canonical strip/bar. A pinned change reshapes; "auto"
  // just follows the user's drag. The settings window is separate, so opening it no
  // longer reshapes anything here.
  useEffect(() => {
    if (mode !== "dock") {
      return;
    }
    const sizingOrientation: Orientation =
      orientationPref === "auto" ? effectiveOrientation : orientationPref;
    const minHeight =
      sizingOrientation === "horizontal"
        ? WINDOW_MIN_HEIGHT_HORIZONTAL
        : WINDOW_MIN_HEIGHT_VERTICAL;
    const maxSize =
      orientationPref === "vertical"
        ? new LogicalSize(WINDOW_MAX_WIDTH_VERTICAL, WINDOW_MAX_UNBOUNDED)
        : orientationPref === "horizontal"
          ? new LogicalSize(WINDOW_MAX_UNBOUNDED, WINDOW_MAX_HEIGHT_HORIZONTAL)
          : null;
    // Place on the first dock mount (so a saved layout opens correctly) and on every
    // pinned reshape, but not on a later "auto" drag — which must not be fought. The
    // placement runs in THIS operation, after the matching min-size is applied, so a bar
    // can actually shrink to its short height instead of fighting the tall startup min
    // (a separate, earlier-queued placement would race and leave a horizontal layout in a
    // tall window). placePickerWindow/placeBarWindow follow the live orientation.
    const shouldPlace = orientationPref !== "auto" || !didInitialPlaceRef.current;
    didInitialPlaceRef.current = true;
    void enqueueWindowOperation(async () => {
      try {
        const win = getCurrentWindow();
        // Fully unbind first (null is Tauri's unset) so a larger min can't clash
        // with a stale max, then re-apply the real cap below.
        await win.setMaxSize(null);
        await win.setMinSize(new LogicalSize(WINDOW_MIN_WIDTH, minHeight));
        await win.setMaxSize(maxSize);
        if (shouldPlace) {
          await summonPlacementRef.current();
        }
      } catch {
        // Best-effort: a failed update leaves the prior constraints/shape in place.
      }
    });
  }, [mode, orientationPref, effectiveOrientation]);

  // The dock and settings windows mirror shared prefs to each other (separate
  // webviews don't share React state). The label of the *other* window is the emit
  // target.
  const otherWindowLabel = mode === "settings" ? "main" : "settings";
  const broadcastPrefs = (payload: PrefsSync) => {
    void emitTo(otherWindowLabel, PREFS_SYNC_EVENT, payload).catch(() => {
      // Best-effort: the other window still converges on its next reload.
    });
  };

  // The preflight the dock currently has resolved, in the sync wire shape. Carries
  // the dock's LoadState status plus the runnerKey it describes (which lags the active
  // runner by one async cycle on a switch — exactly as the dock's own card guards);
  // `preflight` is non-null only when ready. The dock pushes this to settings on every
  // change and on request; the ref lets the []-dep listener answer a replay request
  // without capturing stale state.
  const dockPreflightSync: PrefsSync =
    state.status === "ready"
      ? {
          kind: "preflight",
          status: "ready",
          runnerKey: state.runnerKey,
          preflight: state.preflight,
        }
      : { kind: "preflight", status: state.status, runnerKey, preflight: null };
  const dockPreflightSyncRef = useRef(dockPreflightSync);
  dockPreflightSyncRef.current = dockPreflightSync;

  // Mirror the dock's resolved preflight to the settings window whenever it changes,
  // so the settings card reuses it instead of probing itself. Serializing on the JSON
  // of the payload keeps this from re-emitting on unrelated re-renders (live row
  // updates re-render the dock frequently). Settings ignores its own (mode-gated).
  const dockPreflightSyncKey = JSON.stringify(dockPreflightSync);
  useEffect(() => {
    if (mode !== "dock") {
      return;
    }
    broadcastPrefs(dockPreflightSync);
    // dockPreflightSync is recomputed each render; gate on its stable serialization.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode, dockPreflightSyncKey]);

  // Open the settings window (created hidden at launch, kept warm). The dock no
  // longer renders settings itself.
  const openSettings = () => {
    setIsSourceMenuOpen(false);
    void (async () => {
      try {
        const settingsWindow = await WebviewWindow.getByLabel("settings");
        if (!settingsWindow) {
          return;
        }
        await settingsWindow.unminimize();
        try {
          // Center the settings window on the dock's current monitor before revealing it.
          // The kept-warm window otherwise reuses its last position, which on a
          // multi-monitor setup can be off-screen or on a different display than the dock
          // (so "Open settings" looks like it did nothing).
          await invoke("place_settings_window");
        } catch {
          // Positioning is best-effort; still show at the OS-restored position.
        }
        await settingsWindow.show();
        await settingsWindow.setFocus();
      } catch {
        // Best-effort; nothing to fall back to if the handle is missing.
      }
    })();
  };
  // The settings window closes by hiding (kept warm), so a back/Done press just
  // hides this window. It never probes (it reuses the dock's synced preflight), so
  // there's no per-window probe to stop on hide.
  const closeSettings = () => {
    void getCurrentWindow().hide();
  };

  // Apply prefs changed in the other window. The listener only sets state — the
  // existing theme/glass/orientation/profile effects then apply it — and never
  // re-broadcasts, so window A -> B -> A can't loop.
  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | null = null;
    void listen<PrefsSync>(PREFS_SYNC_EVENT, (event) => {
      const payload = event.payload;
      if (payload.kind === "theme") {
        setThemePref(payload.theme);
      } else if (payload.kind === "orientation") {
        setOrientationPref(payload.orientation);
      } else if (payload.kind === "glass") {
        setGlassEnabled(payload.enabled);
        setSurfaceAlpha(payload.alpha);
      } else if (payload.kind === "profiles") {
        // Keep a warm settings window's unsaved drafts: a dock-side source switch must
        // not silently overwrite an in-progress edit (the window is hidden, not closed,
        // precisely to preserve it). Active id and drafts both stay on the edited
        // profile, so they never mismatch; the change is adopted later via the
        // focus-reconcile path once the edit is applied or reset. The dock always adopts
        // so its live picker tracks the current profile config.
        if (mode === "settings" && isSettingsDirtyRef.current) {
          return;
        }
        setProfileState(loadStoredProfiles());
      } else if (payload.kind === "preflight") {
        // Settings adopts the dock's resolved preflight (keyed by the runnerKey it
        // describes). The card guards on runnerKey === its own active runnerKey, so a
        // result that arrives mid-switch (for the previous source) reads as "Checking"
        // until the matching one lands. The dock ignores this (it's the producer).
        if (mode === "settings") {
          setSyncedPreflight({
            status: payload.status,
            runnerKey: payload.runnerKey,
            preflight: payload.preflight,
          });
        }
      } else if (payload.kind === "preflight-request") {
        // The dock answers a settings-side replay request with its current preflight.
        // Read through a ref so this []-dep listener never captures a stale state.
        if (mode === "dock") {
          broadcastPrefs(dockPreflightSyncRef.current);
        }
      }
    }).then((fn) => {
      if (disposed) {
        fn();
      } else {
        unlisten = fn;
      }
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // The live listener above can miss a change broadcast before it registered, and
  // emitTo has no replay — so a warm (hidden) settings window could linger on stale
  // state with no way to recover short of a restart. Re-read localStorage whenever this
  // window gains focus (i.e. is shown/reopened) to reconcile. Appearance prefs are
  // always safe to refresh (primitive setters no-op when unchanged); the profile is
  // re-seeded only when there are no unsaved edits AND the snapshot actually changed, so
  // a kept-warm in-progress edit is never clobbered and idle focuses don't churn state.
  useEffect(() => {
    if (mode !== "settings") {
      return;
    }
    const win = getCurrentWindow();
    let disposed = false;
    let unlisten: UnlistenFn | null = null;
    void win
      .onFocusChanged(({ payload: focused }) => {
        if (!focused) {
          return;
        }
        setThemePref(loadStoredTheme());
        setOrientationPref(loadStoredOrientation());
        setGlassEnabled(loadStoredGlass());
        setSurfaceAlpha(loadStoredSurfaceAlpha());
        if (!isSettingsDirtyRef.current) {
          setProfileState((current) => {
            const reloaded = loadStoredProfiles();
            return JSON.stringify(current) === JSON.stringify(reloaded) ? current : reloaded;
          });
        }
        // Preflight has no localStorage to re-read and emitTo has no replay, so ask the
        // dock to re-emit its current result. Covers a preflight broadcast missed while
        // this window was hidden/unfocused (it isn't subscribed to the live picker, so
        // it can't recompute the result itself) — fixing a card stuck on "Checking"
        // after a dock-side source switch this window didn't see.
        broadcastPrefs({ kind: "preflight-request" });
      })
      .then((fn) => {
        if (disposed) {
          fn();
        } else {
          unlisten = fn;
        }
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [mode]);

  // The profiles listener drops dock-side syncs while this window is dirty, and the
  // focus-reconcile only runs on a later focus — so a change skipped mid-edit would
  // linger after Reset/Apply clears the drafts (still focused, no new focus event).
  // Adopt the latest stored profiles whenever the window is clean. Value-guarded so an
  // unchanged reload doesn't churn state or reset the picker selection.
  useEffect(() => {
    if (mode !== "settings" || isSettingsDirty) {
      return;
    }
    setProfileState((current) => {
      const reloaded = loadStoredProfiles();
      return JSON.stringify(current) === JSON.stringify(reloaded) ? current : reloaded;
    });
  }, [mode, isSettingsDirty]);

  // Keep the settings window warm: intercept its close so the red button / Cmd-W
  // hides it (instant reopen, drafts preserved) rather than destroying it.
  useEffect(() => {
    if (mode !== "settings") {
      return;
    }
    const win = getCurrentWindow();
    const unlistenPromise = win.onCloseRequested((event) => {
      event.preventDefault();
      void win.hide();
    });
    return () => {
      void unlistenPromise.then((unlisten) => unlisten());
    };
  }, [mode]);

  // Closing the dock means quitting. The settings window is kept warm (hidden, never
  // self-destroys), so it must be torn down before the dock goes — otherwise that
  // hidden window keeps the process alive with no visible UI. preventDefault() holds
  // the dock open until the (awaited) settings teardown finishes, then we force the
  // dock closed; without it the dock webview can be destroyed mid-IPC and strand the
  // hidden window. destroy() forces teardown without firing either hide-handler, and
  // the dock is destroyed even if the settings lookup throws, so the app always exits.
  useEffect(() => {
    if (mode !== "dock") {
      return;
    }
    const win = getCurrentWindow();
    const unlistenPromise = win.onCloseRequested(async (event) => {
      event.preventDefault();
      try {
        const settings = await WebviewWindow.getByLabel("settings");
        await settings?.destroy();
      } catch {
        // Best-effort; fall through to tear the dock down regardless.
      }
      await win.destroy();
    });
    return () => {
      void unlistenPromise.then((unlisten) => unlisten());
    };
  }, [mode]);

  // Toggle the macOS glass backdrop. Order matters so we never flash the bare
  // desktop through the transparent window: when enabling, raise the blur layer
  // first, then mark the surface translucent; when disabling, go opaque first,
  // then drop the blur. macOS-only — the toggle isn't offered anywhere else.
  useEffect(() => {
    if (!IS_MAC) {
      return;
    }
    try {
      window.localStorage.setItem(GLASS_STORAGE_KEY, glassEnabled ? "on" : "off");
    } catch {
      // Best-effort; the in-memory preference still applies this session.
    }

    // Vibrancy lives on the dock. The settings window is a solid, normally-chromed
    // window, so it persists + mirrors the pref but never frosts itself.
    if (mode !== "dock") {
      return;
    }

    let cancelled = false;
    glassOperationQueue = glassOperationQueue.then(async () => {
      // A newer toggle superseded this one before it ran; skip the native call
      // entirely so the queue settles on the latest desired state.
      if (cancelled) {
        return;
      }
      try {
        if (glassEnabled) {
          await invoke("set_window_glass", { enabled: true });
          if (!cancelled) {
            // Flip the surface translucent and arm the adaptive tokens together,
            // so `--glass-clear` is only nonzero once the blur is actually live.
            document.documentElement.setAttribute("data-glass", "on");
            setGlassClear(glassClearFor(surfaceAlphaRef.current));
          }
        } else {
          document.documentElement.setAttribute("data-glass", "off");
          setGlassClear(0);
          await invoke("set_window_glass", { enabled: false });
        }
      } catch (error) {
        // Native call failed: keep the surface opaque AND the tokens un-adapted.
        document.documentElement.setAttribute("data-glass", "off");
        setGlassClear(0);
        appendDebugEntry({
          kind: "command",
          label: "Glass effect",
          detail: errorMessage(error),
        });
      }
    });

    return () => {
      cancelled = true;
    };
  }, [glassEnabled, mode]);

  // Drive the tint opacity via a CSS variable; the data-glass rules in styles.css
  // only consume it while glass is on, so this is harmless when glass is off.
  // `--glass-clear` (a 0..1 see-through scalar the adaptive tokens interpolate
  // against) is owned by the glass-toggle effect so it stays in lockstep with the
  // actual data-glass state, not the pending React intent. Here we only refresh it
  // for slider moves while glass is already live; on/off transitions are that
  // effect's job.
  useEffect(() => {
    const root = document.documentElement;
    root.style.setProperty("--surface-alpha", String(surfaceAlpha));
    if (root.getAttribute("data-glass") === "on") {
      setGlassClear(glassClearFor(surfaceAlpha));
    }
    try {
      window.localStorage.setItem(SURFACE_ALPHA_STORAGE_KEY, surfaceAlpha.toFixed(2));
    } catch {
      // Best-effort persistence.
    }
  }, [surfaceAlpha]);

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
      // A failed focus is strong evidence the row is stale (the pane is gone). The
      // daemon is event-driven with periodic reconcile OFF by default, so a missed
      // tmux close notification won't self-correct — agentscan's own design names the
      // connect/reconnect bootstrap as the ground-truth recovery (config.rs). Re-arm
      // the live client: re-subscribing makes the daemon publish a fresh initial
      // snapshot, which the worker re-derives via load_picker_rows, dropping the dead
      // row. This is the push-model equivalent of the old one-shot refetch.
      reconnectLive();
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
    // Control + a row's displayed hotkey jumps straight to that pane. Require
    // Control alone so we never shadow ⌘ shortcuts or Ctrl+⌘ combos. On macOS,
    // editing uses ⌘, so Ctrl is free even inside the search box — bypass the
    // interactive-target gate so you can filter then jump in one motion. On
    // Windows/Linux, Ctrl *is* the editing modifier (Ctrl+C/V/X/Z/F), so only
    // honor the hotkey when no input/button is focused; otherwise native
    // clipboard/find/undo wins. (Key match is character-based to mirror the kbd
    // label and the CLI's configured char hotkeys; non-US layouts that shift
    // digit keys may no-op on the default number row, which is a silent miss
    // rather than a wrong action.)
    const ctrlActivate = event.ctrlKey && !event.metaKey && !event.altKey && !event.shiftKey;
    if (ctrlActivate && (IS_MAC || !isInteractiveShortcutTarget(event.target))) {
      const target = pickerRowForKeyboardKey(pickerRows, event.key);
      if (target) {
        event.preventDefault();
        setSelectedPaneId(target.pane_id);
        void activateSelectedRow(target);
        return;
      }
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
      // Merge onto the LATEST persisted state, not this window's in-memory snapshot: a
      // warm settings window can hold a stale snapshot (it skips profile syncs while
      // dirty), so writing that whole snapshot back would clobber dock-side add/delete/
      // source-switch changes already in localStorage. Apply the edit to the profile this
      // form is editing (by id) and keep the dock's latest profile list + active source.
      const editedId = current.activeProfileId;
      const latest = loadStoredProfiles();
      const next = updateProfileSettingsById(
        latest,
        editedId,
        profileNameDraft.trim(),
        normalized,
        sshHostDraft,
        sshClientTtyDraft,
      );
      storeProfiles(next);
      void broadcastPrefs({ kind: "profiles" });
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
    // A dirty settings window may show a stale active highlight: it skips dock-side profile
    // syncs mid-edit, and the focus/clean-adopt reconcilers also skip while dirty, so the
    // card the rail highlights (activeProfile.id) can lag the persisted active source. Clicking
    // that already-highlighted card must stay a no-op here — otherwise the id !== latest path
    // below would rewrite localStorage to our stale id and broadcast, flipping the dock off the
    // source it was deliberately switched to elsewhere (its drafts-preserving switch must win
    // until Apply/Reset). This divergence only exists while dirty in settings, so clean windows
    // and the dock are untouched: clicking a different source still writes + broadcasts, and
    // re-selecting the live active still hits the same-reference no-op below. Keyed on
    // activeProfile.id (the highlighted card) rather than current.activeProfileId, since
    // getActiveProfile falls back to the first runnable profile when the active id is unset.
    if (mode === "settings" && isSettingsDirty && id === activeProfile.id) {
      return;
    }

    setProfileState((current) => {
      // Switch active on the LATEST persisted state so a concurrent dock-side
      // add/delete isn't clobbered by this window's possibly-stale snapshot.
      const latest = loadStoredProfiles();
      // Clicking the already-persisted (live) active source must not bump loadShellState
      // (a needless re-probe momentarily unmatches state.runnerKey, which gates liveReady
      // off and flickers the connection through "Switching profile…") — so when our
      // snapshot already matches, keep the SAME reference and do nothing. But a dirty
      // settings window may hold a stale snapshot
      // (it skips syncs mid-edit); there, adopt the latest so clicking the dock's current
      // source navigates to it instead of staying stuck on the stale selection.
      if (id === latest.activeProfileId) {
        return JSON.stringify(current) === JSON.stringify(latest) ? current : latest;
      }

      const profile = latest.profiles.find((candidate) => candidate.id === id);
      if (!profile || !isRunnableProfile(profile)) {
        return current;
      }

      const next = { ...latest, activeProfileId: id };
      storeProfiles(next);
      void broadcastPrefs({ kind: "profiles" });
      return next;
    });
  }

  function addSshProfile() {
    // Generate the id ONCE, outside the updater: React StrictMode double-invokes state
    // updaters in dev, and a fresh id per invocation combined with the in-updater
    // append+persist would create two profiles from a single click. A stable id plus the
    // existence guard below make the append idempotent across the doubled invocation.
    const id = newProfileId("ssh");
    setProfileState(() => {
      // Append onto the LATEST persisted state so a concurrent dock-side change isn't
      // clobbered by this window's possibly-stale snapshot.
      const latest = loadStoredProfiles();
      if (latest.profiles.some((profile) => profile.id === id)) {
        // The first (StrictMode-doubled) invocation already appended and persisted this
        // profile; don't add a duplicate on the second pass.
        return latest;
      }
      const profile: SshProfileConfig = {
        id,
        name: nextRemoteProfileName(latest.profiles),
        kind: "ssh",
        host: "",
        clientTty: "",
        runner: emptyRunnerSettings(),
        enabled: true,
      };
      const next = {
        activeProfileId: profile.id,
        profiles: [...latest.profiles, profile],
      };
      storeProfiles(next);
      void broadcastPrefs({ kind: "profiles" });
      return next;
    });
  }

  function deleteActiveProfile() {
    if (activeProfile.kind === "local") {
      return;
    }

    const targetId = activeProfile.id;
    setProfileState(() => {
      // Remove from the LATEST persisted state so a concurrent dock-side change isn't
      // clobbered. Keep the latest active source unless it's the profile being deleted.
      const latest = loadStoredProfiles();
      const profiles = latest.profiles.filter((profile) => profile.id !== targetId);
      const fallback = profiles.find((profile) => profile.kind === "local") ?? profiles[0];
      const next = normalizeProfileState({
        activeProfileId: profiles.some((profile) => profile.id === latest.activeProfileId)
          ? latest.activeProfileId
          : fallback?.id,
        profiles,
      });
      storeProfiles(next);
      void broadcastPrefs({ kind: "profiles" });
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
    if (mode !== "dock") {
      return;
    }
    const handler = (event: KeyboardEvent) => pickerKeyDownRef.current(event);
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [mode]);

  // The dock shows its boot/error screen until the active runner has a usable
  // preflight. That covers three not-ready cases: still probing; the probe failed
  // (IPC error); or the probe resolved but reports the CLI unavailable (bad binary
  // path / SSH target) for the CURRENT runner. The last case used to fall through to
  // a perpetual "Waiting for a source" live banner that never explained what broke —
  // routing it here surfaces the real preflight error and the Open settings recovery
  // path. A stale ready state from a profile still switching (runnerKey mismatch) is
  // left to the picker's "Switching…" view, not treated as an error.
  const dockPreflightUnusable =
    state.status === "ready" && state.runnerKey === runnerKey && !state.preflight.ok;
  if (mode === "dock" && (state.status !== "ready" || dockPreflightUnusable)) {
    const probing = state.status === "loading";
    const detail =
      state.status === "failed"
        ? state.message
        : state.status === "ready"
          ? (state.preflight.error ?? `${profileKindLabel(activeProfile)} CLI unavailable`)
          : "Waiting for the daemon…";
    return (
      // Recovery UI renders in the live orientation: a centered column in the vertical
      // strip, and a compact row in the horizontal bar (styles.css) so the heading and
      // the only "Open settings" path stay visible without clipping in the ~120px bar.
      <main className="sidebar" data-orientation={effectiveOrientation}>
        <div className="boot-state" aria-live="polite">
          <span className="boot-spinner" aria-hidden="true" />
          <div className="boot-copy">
            <h1>{probing ? "Connecting" : "Can’t reach agentscan"}</h1>
            <p>{detail}</p>
          </div>
          {/* Always offer a path into settings: a hung "loading" (e.g. a stalled
              profile/SSH preflight) or a CLI-unavailable runner otherwise traps the
              user with no way to fix the binary path or host. */}
          <button type="button" onClick={openSettings}>
            Open settings
          </button>
        </div>
      </main>
    );
  }

  if (mode === "settings") {
    // The profile list comes from profileState (the live source of truth) so
    // add/delete/switch are reflected immediately. The settings window never runs
    // its own preflight (which for SSH would be a duplicate `ssh … --version`); it
    // reuses the dock's, mirrored over the event bus into `syncedPreflight`. That
    // result is only trusted when its runnerKey matches this window's active source;
    // otherwise it describes the previous one mid-switch and reads as "Checking" until
    // the dock re-probes and pushes the matching one (or a focus-time replay request
    // refreshes it). A failed dock status (IPC error) reads as "Unreachable".
    const syncMatches =
      syncedPreflight !== null && syncedPreflight.runnerKey === runnerKey;
    const preflight = syncMatches ? syncedPreflight.preflight : null;
    const syncFailed = syncMatches && syncedPreflight.status === "failed";
    const preflightTone = !preflight
      ? syncFailed
        ? "error"
        : "unknown"
      : preflight.ok
        ? "idle"
        : "error";
    const preflightLabel = !preflight
      ? syncFailed
        ? "Unreachable"
        : "Checking"
      : preflight.ok
        ? "Ready"
        : "Unavailable";
    const preflightDetail = !preflight
      ? syncFailed
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
            onClick={closeSettings}
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
                  onClick={() => {
                    setThemePref(option);
                    broadcastPrefs({ kind: "theme", theme: option });
                  }}
                >
                  {option === "system" ? "System" : option === "light" ? "Light" : "Dark"}
                </button>
              ))}
            </div>

            <div className="setting-label dock-layout-label">
              <span>Dock layout</span>
              <span className="setting-hint">
                Auto follows the window shape; pin a strip or bar
              </span>
            </div>
            <div
              className="theme-toggle layout-toggle"
              role="group"
              aria-label="Dock layout"
            >
              {(["auto", "vertical", "horizontal"] as OrientationPreference[]).map(
                (option) => (
                  <button
                    className={`theme-option${orientationPref === option ? " active" : ""}`}
                    key={option}
                    type="button"
                    aria-pressed={orientationPref === option}
                    onClick={() => {
                      setOrientationPref(option);
                      broadcastPrefs({ kind: "orientation", orientation: option });
                    }}
                  >
                    {option === "auto"
                      ? "Auto"
                      : option === "vertical"
                        ? "Vertical"
                        : "Horizontal"}
                  </button>
                ),
              )}
            </div>

            {IS_MAC ? (
              <div className="glass-controls">
                <div className="setting-row">
                  <div className="setting-label">
                    <span>Glass</span>
                    <span className="setting-hint">Frost the window over your desktop</span>
                  </div>
                  <button
                    className={`switch${glassEnabled ? " on" : ""}`}
                    type="button"
                    role="switch"
                    aria-checked={glassEnabled}
                    aria-label="Glass effect"
                    onClick={() => {
                      const next = !glassEnabled;
                      setGlassEnabled(next);
                      broadcastPrefs({ kind: "glass", enabled: next, alpha: surfaceAlpha });
                    }}
                  >
                    <span className="switch-thumb" />
                  </button>
                </div>

                {glassEnabled ? (
                  <label className="setting-row">
                    <div className="setting-label">
                      <span>Transparency</span>
                      <span className="setting-hint">
                        {Math.round((1 - surfaceAlpha) * 100)}%
                      </span>
                    </div>
                    {/* Slider reads as transparency (right = clearer); state stores the
                        inverse as the surface alpha the CSS tint consumes. */}
                    <input
                      className="glass-slider"
                      type="range"
                      min={0}
                      max={SURFACE_ALPHA_MAX - SURFACE_ALPHA_MIN}
                      step={0.02}
                      value={SURFACE_ALPHA_MAX - surfaceAlpha}
                      onChange={(event) => {
                        const next = SURFACE_ALPHA_MAX - Number(event.target.value);
                        setSurfaceAlpha(next);
                        broadcastPrefs({ kind: "glass", enabled: glassEnabled, alpha: next });
                      }}
                      aria-label="Glass transparency"
                    />
                  </label>
                ) : null}
              </div>
            ) : null}
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
  // narrows `state` to "ready" so the picker can read state.preflight/profiles.
  if (state.status !== "ready") {
    return null;
  }

  return (
    <main className="sidebar" data-orientation={effectiveOrientation}>
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
        {/* Never disabled: this is the only in-dock manual recovery path, and a
            subscribe can hang in "connecting" without ever emitting a frame (e.g. an
            SSH target stuck in auth before stdout). Disabling it there would wedge the
            user out of forcing a fresh reconnect. The spin is feedback only.
            reconnect is NOT a cached-snapshot replay: re-subscribing makes the daemon
            publish a fresh initial snapshot, which the worker re-derives through
            load_picker_rows — the same fresh pane-output status the old hotkeys-fetch
            button produced, plus it re-arms the connection. */}
        <button
          className="icon-button"
          type="button"
          aria-label="Reconnect"
          title="Reconnect"
          onClick={() => reconnectLive()}
        >
          <span className={isReconnecting ? "spin" : undefined}>{"↻"}</span>
        </button>
      </header>

      {displayConnection.status !== "online" ? (
        <LiveStrip
          status={displayConnection}
          onStart={() => startLive()}
          onReconnect={() => reconnectLive()}
        />
      ) : null}

      {activation.status === "failed" ? (
        <div className="inline-error" role="alert">
          {activation.message}
        </div>
      ) : null}

      <div className="picker-scroll" aria-label="Agents" tabIndex={-1}>
        <GroupedPicker
          activation={activation}
          filterQuery={pickerFilter}
          focusedPaneId={focusedPaneId}
          groups={pickerGroups}
          logoTheme={resolvedTheme}
          selectedPaneId={selectedPaneId}
          state={pickerDataState}
          totalRows={allPickerRows.length}
          onActivate={activateSelectedRow}
          onClearFilter={() => setPickerFilter("")}
          onSelect={(row) => setSelectedPaneId(row.pane_id)}
        />
      </div>

      {attachedClientCount > 1 ? (
        <div className="client-warning" role="status">
          <span className="client-warning-icon" aria-hidden="true">
            ⚠
          </span>
          <span>
            Multiple clients attached to the tmux server — the live-pane highlight
            follows your most recent one.
          </span>
        </div>
      ) : null}

      <footer className="bottombar">
        <div className="source-switcher" ref={sourceMenuRef}>
          <button
            className="source-trigger"
            type="button"
            aria-haspopup="menu"
            aria-expanded={isSourceMenuOpen}
            onClick={() => {
              // The inline menu opens upward and would clip inside the thin
              // horizontal bar, so there the trigger re-docks into settings (which
              // owns the full Sources list) instead of popping a cramped menu.
              if (effectiveOrientation === "horizontal") {
                openSettings();
              } else {
                setIsSourceMenuOpen((open) => !open);
              }
            }}
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
                  openSettings();
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
          onClick={openSettings}
        >
          {"⚙"}
        </button>
      </footer>
    </main>
  );
}

// Project the LiveConnection service's state onto the PickerState the GroupedPicker
// renders. The service is the single owner of rows + connection status; this just
// picks the view: keep showing the last rows while (re)connecting so the list
// doesn't flash a skeleton on a brief blip, show the failure only when a fatal
// state has actually cleared the rows, and otherwise a loading skeleton.
function pickerStateFromLive(live: LiveState): PickerState {
  const { connection, rows } = live;
  if (rows.length > 0) {
    return { status: "ready", rows };
  }
  if (connection.status === "fatal") {
    return { status: "failed", message: connection.message };
  }
  if (connection.status === "connecting" || connection.status === "reconnecting") {
    return { status: "loading" };
  }
  // online or noDaemon with no rows → an empty (but resolved) list.
  return { status: "ready", rows };
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

// The banner shown for any non-online connection. `noDaemon` (the dock latched but
// found no daemon to attach to) offers Start agentscan; `fatal` offers both Start
// agentscan and a latch-only Reconnect (a Start refusal lands here, and Start is the
// action that actually retries it) — so the dock never wedges with a dead stream and
// no way out. connecting/reconnecting are transient and self-heal, so they show
// progress only.
function LiveStrip({
  status,
  onStart,
  onReconnect,
}: {
  status: ConnectionStatus;
  onStart: () => void;
  onReconnect: () => void;
}) {
  const tone = status.status === "fatal" ? "error" : "warn";

  return (
    <div className={`live-strip ${tone}`} aria-live="polite">
      <span className="status-dot" data-tone={tone === "error" ? "error" : "busy"} />
      <span className="live-label">{liveStateLabel(status)}</span>
      <span className="live-message">{status.message}</span>
      {status.status === "noDaemon" ? (
        <button className="live-action" type="button" onClick={onStart}>
          Start agentscan
        </button>
      ) : status.status === "fatal" ? (
        // A fatal includes an explicit-Start refusal (e.g. macOS codesign/trust), whose
        // actual fix is to retry the start once resolved. Reconnect is latch-only and
        // can't spawn, so it would force a no-daemon round-trip before Start reappears.
        // Offer Start agentscan (start-or-latch — strictly more capable, recovers every
        // fatal cause the user fixes) alongside the latch-only Reconnect.
        <div className="live-actions">
          <button className="live-action" type="button" onClick={onStart}>
            Start agentscan
          </button>
          <button className="live-action" type="button" onClick={onReconnect}>
            Reconnect
          </button>
        </div>
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

function loadStoredOrientation(): OrientationPreference {
  try {
    const value = window.localStorage.getItem(ORIENTATION_STORAGE_KEY);
    if (value === "auto" || value === "vertical" || value === "horizontal") {
      return value;
    }
  } catch {
    // localStorage unavailable; fall back to the responsive default.
  }
  return "auto";
}

function loadStoredGlass(): boolean {
  // Glass is macOS-only (native vibrancy); other platforms never enable it.
  if (!IS_MAC) {
    return false;
  }
  try {
    const raw = window.localStorage.getItem(GLASS_STORAGE_KEY);
    // Default glass on for macOS on first run (no stored choice); once the user
    // toggles it, "on"/"off" is persisted and respected.
    return raw === null ? true : raw === "on";
  } catch {
    return true;
  }
}

function loadStoredSurfaceAlpha(): number {
  try {
    // Guard the missing/empty case explicitly: Number(null) and Number("") are
    // both 0 (finite), which would otherwise clamp first-time users to the most
    // transparent setting instead of the frosted default.
    const raw = window.localStorage.getItem(SURFACE_ALPHA_STORAGE_KEY);
    if (raw !== null && raw.trim() !== "") {
      const parsed = Number(raw);
      if (Number.isFinite(parsed)) {
        return Math.min(SURFACE_ALPHA_MAX, Math.max(SURFACE_ALPHA_MIN, parsed));
      }
    }
  } catch {
    // localStorage unavailable; use the default tint.
  }
  return SURFACE_ALPHA_DEFAULT;
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

function updateProfileSettingsById(
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

function liveStateLabel(status: ConnectionStatus) {
  switch (status.status) {
    case "online":
      return "Live";
    case "reconnecting":
      return "Reconnecting";
    case "noDaemon":
      return "No daemon";
    case "fatal":
      return "Live client failed";
    case "connecting":
      return "Connecting";
  }
}

function pickerRowForKeyboardKey(rows: PickerRow[], key: string) {
  const normalizedKey = normalizePickerKeyboardKey(key);
  if (normalizedKey === null) {
    return undefined;
  }

  // Match the key returned by `agentscan hotkeys --format json`; this keeps
  // desktop activation tied to the user's configured picker_keys, not the
  // built-in default order.
  return rows.find((row) => normalizePickerKeyboardKey(row.key) === normalizedKey);
}

function normalizePickerKeyboardKey(key: string) {
  if (key.length !== 1) {
    return null;
  }

  const normalizedKey = key.toUpperCase();
  return /^[A-Z0-9]$/.test(normalizedKey) ? normalizedKey : null;
}

// Persistent-window model: the global hotkey raises/focuses the window; it
// never toggles it away. The caller passes the placement for the live orientation
// so summoning a pinned/auto horizontal bar re-docks it as a bar, not a strip.
async function raisePickerWindow(place: () => Promise<void> = placePickerWindow) {
  await enqueueWindowOperation(async () => {
    const appWindow = getCurrentWindow();
    await place();
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

async function placeBarWindow() {
  try {
    await invoke("place_bar_window");
  } catch {
    // Placement is best-effort; the layout still follows the pinned orientation.
  }
}

function GroupedPicker({
  activation,
  filterQuery,
  focusedPaneId,
  groups,
  logoTheme,
  selectedPaneId,
  state,
  totalRows,
  onActivate,
  onClearFilter,
  onSelect,
}: {
  activation: PickerActivation;
  filterQuery: string;
  focusedPaneId: string | null;
  groups: PickerGroup[];
  logoTheme: LogoTheme;
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
    return (
      <div className="empty-detected">
        <img className="empty-logo" src={logoUrl} alt="agentscan" />
        <p className="empty-note">No agents detected.</p>
      </div>
    );
  }

  return (
    <div className="picker-groups">
      {groups.map((group) => (
        <section className="picker-group" key={group.project}>
          <h2 className="group-header">{group.project}</h2>
          <ul className="agent-list">
            {group.rows.map((row) => {
              const isSelected = row.pane_id === selectedPaneId;
              // The single live pane the user is in. The selection cursor follows
              // it, so in the common case the two coincide and the selection ring
              // sits on the live pane. When they diverge (manual j/k/click away),
              // a faint "live" ring keeps the live pane discoverable. Derived from
              // the same resolved id as the cursor so the legacy `is_active`
              // fallback stays single-row and consistent.
              const isFocused = row.pane_id === focusedPaneId;
              const isFocusing =
                activation.status === "running" && activation.paneId === row.pane_id;
              const logo = providerLogo(row.provider, logoTheme);
              return (
                <li
                  aria-selected={isSelected}
                  aria-current={isFocused ? "true" : undefined}
                  className={`agent-row${isSelected ? " selected" : ""}${
                    isFocused && !isSelected ? " live" : ""
                  }`}
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
                  <kbd>
                    <span className="kbd-mod">{HOTKEY_MODIFIER_LABEL}</span>
                    {row.key}
                  </kbd>
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
