import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { register, unregister } from "@tauri-apps/plugin-global-shortcut";
import { Result, useAtomSet, useAtomValue } from "@effect-atom/atom-react";
import { providerLogo, type LogoTheme } from "./providerLogos";
import {
  addSshProfileAtom,
  appearanceAtom,
  applyRunnerSettingsAtom,
  configureAtom,
  configurePreflightAtom,
  deleteActiveProfileAtom,
  liveStateAtom,
  preflightStateAtom,
  profilesAtom,
  reconnectAtom,
  reloadAppearanceAtom,
  reloadProfilesAtom,
  requestPreflightSyncAtom,
  selectProfileAtom,
  setFramelessAtom,
  setGlassEnabledAtom,
  setOrientationAtom,
  setSurfaceAlphaAtom,
  setThemeAtom,
  startAtom,
  syncedPreflightAtom,
} from "./effect/atoms";
import type { ConnectionStatus, LiveState, PickerRow } from "./effect/types";
import type { PreflightState } from "./effect/Preflight";
import {
  glassClearFor,
  loadAppearance,
  SURFACE_ALPHA_MAX,
  SURFACE_ALPHA_MIN,
} from "./effect/appearanceModel";
import {
  commandPrefix,
  focusCommandLabel,
  getActiveProfile,
  isRunnableProfile,
  loadProfileState,
  normalizeRunnerSettings,
  profileKindLabel,
  runnerKeyForProfile,
  runnerSettingsEqual,
  runnerSettingsForProfile,
  runnerSummary,
  validateProfileDraft,
  type DesktopProfileConfig,
  type EnvironmentVariable,
  type RunnerSettings,
} from "./effect/profileModel";
import {
  PREFS_SYNC_EVENT,
  type Orientation,
  type OrientationPreference,
  type PrefsSync,
  type ShellMode,
  type ThemePreference,
} from "./effect/prefs";
import logoUrl from "./assets/agentscan-logo.png";

const PICKER_HOTKEY = "CommandOrControl+Shift+A";
// Appearance prefs (storage keys, alpha bounds, glassClearFor, the parsers) live in
// effect/appearanceModel and are owned by the Appearance Effect service; the DOM apply
// (this setter, the theme/glass/sizing effects) stays here.
const setGlassClear = (clear: number) => {
  document.documentElement.style.setProperty("--glass-clear", clear.toFixed(3));
};
const DEBUG_LOG_LIMIT = 80;
// Window min-size floors, applied at runtime per orientation. The vertical pair
// mirrors the startup floor in tauri.{macos.,}conf.json; horizontal drops the
// height floor so the bar can shrink to dock height instead of a tall slab.
const WINDOW_MIN_WIDTH = 220;
const WINDOW_MIN_HEIGHT_VERTICAL = 520;
// Auto-mode floor when the window is wider than tall: lets a freely-dragged window get
// short without collapsing the chip strip. A PINNED horizontal bar ignores this and locks
// to BAR_WINDOW_HEIGHT (below) instead, so its height isn't resizable at all.
const WINDOW_MIN_HEIGHT_HORIZONTAL = 44;
// Locked height for the pinned horizontal bar: min == max == this, so the bar resizes only
// horizontally (the layout is tuned for this exact height). Mirrors BAR_WINDOW_HEIGHT in
// src-tauri/src/lib.rs (the snap height place_bar_window applies) — keep the two in sync.
const BAR_WINDOW_HEIGHT = 56;
// Max-size caps per pinned orientation: vertical stays a strip (width capped, height free);
// the pinned horizontal bar locks height at BAR_WINDOW_HEIGHT (above) with free width.
// "auto" clears the cap. The free axis uses a value larger than any display.
const WINDOW_MAX_WIDTH_VERTICAL = 520;
const WINDOW_MAX_UNBOUNDED = 10000;
// Corner radius (logical px) for frameless mode, matching the macOS window rounding the
// native frame would otherwise draw. Mirrors --frameless-radius in styles.css and is passed
// to the native glass backdrop so the vibrancy view rounds to the same curve as the webview.
const FRAMELESS_CORNER_RADIUS = 12;

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
// Same discipline for the frameless decorations toggle: serialize the native
// set_window_decorations calls so a fast toggle settles on the latest desired state.
let framelessOperationQueue = Promise.resolve();

// Live-picker subscription state (connection status + rows + epoch fencing + the
// reconnect/latch policy) is owned by the Effect LiveConnection service. This
// component drives it via configure/reconnect/start and observes liveStateAtom.
const DEFAULT_LIVE_STATE: LiveState = {
  connection: { status: "connecting", message: "Starting live client" },
  rows: [],
  rowsRunnerKey: null,
};

// Synchronous, best-effort localStorage read used only to seed the first paint
// (active profile / runnerKey / drafts) before the Profiles service atom resolves.
// All profile WRITES and ongoing reads go through the service; this just matches its
// initial seed so the first render isn't a flash of default state.
const readLocalStorage = (key: string): string | null => {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
};

type DebugEntry = {
  id: number;
  time: string;
  kind: "command" | "stream" | "settings";
  label: string;
  detail: string;
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
  key: string;
  project: string;
  rows: PickerRow[];
};

// The dock's resolved preflight is owned by the Preflight Effect service (observed via
// preflightStateAtom as PreflightState); the picker rows + live connection status live
// in the LiveConnection service (liveStateAtom).
const INITIAL_PREFLIGHT: PreflightState = { status: "loading" };

type PickerState =
  | { status: "loading" }
  | { status: "ready"; rows: PickerRow[] }
  | { status: "failed"; message: string };

type PickerActivation =
  | { status: "idle" }
  | { status: "running"; paneId: string }
  | { status: "failed"; message: string };

function App({ mode }: { mode: ShellMode }) {
  // The dock's resolved CLI preflight is owned by the Preflight Effect service. The dock
  // observes preflightStateAtom and drives the probe via configurePreflight; the service
  // also mirrors each result to the settings window over the shared prefs channel.
  const preflightState = Result.getOrElse(
    useAtomValue(preflightStateAtom),
    () => INITIAL_PREFLIGHT,
  );
  const configurePreflight = useAtomSet(configurePreflightAtom);
  // The settings window never runs its own preflight; it reuses the dock's resolved
  // result, mirrored over the prefs channel into the service's `synced` ref (observed
  // here). This avoids a second `ssh … --version` for remote profiles (an extra
  // round-trip and a possible duplicate passphrase prompt) every time Settings is
  // opened, and keeps the card current even while the window is visible-but-unfocused.
  // Always null in the dock (which is the producer, not a consumer). requestPreflightSync
  // asks the dock to re-emit on focus (emitTo has no replay).
  const syncedPreflight = Result.getOrElse(useAtomValue(syncedPreflightAtom), () => null);
  const requestPreflightSync = useAtomSet(requestPreflightSyncAtom);
  // Profile/settings persistence + cross-window adoption are owned by the Profiles
  // Effect service; this window observes its state via an atom and drives changes
  // through the action atoms below. The first synchronous render (before the runtime
  // resolves the atom) falls back to a direct storage read so the active profile /
  // runnerKey / drafts are correct on the very first paint, matching the service seed.
  const initialProfileState = useMemo(() => loadProfileState(readLocalStorage), []);
  const profileStateResult = useAtomValue(profilesAtom);
  const profileState = Result.getOrElse(profileStateResult, () => initialProfileState);
  const selectProfileSet = useAtomSet(selectProfileAtom);
  const addSshProfileSet = useAtomSet(addSshProfileAtom);
  const deleteActiveProfileSet = useAtomSet(deleteActiveProfileAtom);
  const applyRunnerSettingsSet = useAtomSet(applyRunnerSettingsAtom);
  const reloadProfiles = useAtomSet(reloadProfilesAtom);
  const activeProfile = useMemo(() => getActiveProfile(profileState), [profileState]);
  const runnerSettings = useMemo(() => runnerSettingsForProfile(activeProfile), [activeProfile]);
  // Identity of the exact runner configuration a resolved state describes. It
  // changes on a profile switch AND on any settings edit (binary/env/host/tty)
  // to the active profile, so resolved preflight/picker data is invalidated
  // whenever the underlying target changes, not just when the profile id does.
  const runnerKey = useMemo(() => runnerKeyForProfile(activeProfile), [activeProfile]);
  const [profileNameDraft, setProfileNameDraft] = useState(
    () => getActiveProfile(initialProfileState).name,
  );
  const [settingsDraft, setSettingsDraft] = useState<RunnerSettings>(
    () => getActiveProfile(initialProfileState).runner,
  );
  const [sshHostDraft, setSshHostDraft] = useState(() => {
    const profile = getActiveProfile(initialProfileState);
    return profile.kind === "ssh" ? profile.host : "";
  });
  const [sshClientTtyDraft, setSshClientTtyDraft] = useState(() => {
    const profile = getActiveProfile(initialProfileState);
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
  // Horizontal bar only: search collapses to an icon to save width, expanding to the
  // field on click. The field also stays open whenever a query is active (so a filter
  // is always visible/editable). The vertical strip always shows the full field, so this
  // is inert there. searchInputRef lets the expand action move focus into the field.
  const [isSearchExpanded, setIsSearchExpanded] = useState(false);
  const searchInputRef = useRef<HTMLInputElement | null>(null);
  // Whether the native frame has ACTUALLY been removed (the set_window_decorations effect
  // resolved successfully), as opposed to the desired `framelessEnabled` preference. All
  // custom window chrome (drag regions + minimize/close) is gated on this, never the bare
  // preference, so it can't show as duplicate controls over a still-decorated window while a
  // toggle is mid-flight or after the native call rejected.
  const [framelessApplied, setFramelessApplied] = useState(false);
  // Layout axis, seeded from the current window shape and kept in sync on resize.
  const [orientation, setOrientation] = useState<Orientation>(orientationForViewport);
  // Appearance prefs (theme + dock-layout orientation + glass) are owned by the
  // Appearance Effect service; both windows observe its state via an atom and drive
  // changes through these setters (which persist + cross-window broadcast). The DOM/Tauri
  // apply (data-theme, set_window_glass, window shaping, CSS vars) stays in the effects
  // below. The first synchronous render (before the runtime resolves the atom) falls back
  // to a direct storage read so layout/theme/glass are right on the first paint.
  const initialAppearance = useMemo(() => loadAppearance(readLocalStorage), []);
  const appearance = Result.getOrElse(useAtomValue(appearanceAtom), () => initialAppearance);
  const { themePref, orientationPref, glassEnabled, surfaceAlpha, framelessEnabled } =
    appearance;
  const setThemePref = useAtomSet(setThemeAtom);
  const setOrientationPref = useAtomSet(setOrientationAtom);
  const setGlassEnabled = useAtomSet(setGlassEnabledAtom);
  const setSurfaceAlpha = useAtomSet(setSurfaceAlphaAtom);
  const setFrameless = useAtomSet(setFramelessAtom);
  const reloadAppearance = useAtomSet(reloadAppearanceAtom);
  // Layout preference: "auto" follows the live `orientation`; a pinned value overrides it.
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
  // Concrete theme in effect, kept in sync by the theme effect; drives per-theme
  // logo variant selection. Seeded from the service's initial theme so first paint
  // picks the right logos.
  const [resolvedTheme, setResolvedTheme] = useState<LogoTheme>(() =>
    resolveThemeMode(initialAppearance.themePref),
  );
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

  // The active profile's own validation (its committed values, not the form drafts).
  // Both the live-picker gate and the preflight target read it: a synchronously-invalid
  // profile is gated off the picker in the same render (no flash of the bad target) and
  // resolves to a synthetic failed preflight without an IPC probe.
  const activeProfileValidation = useMemo(
    () =>
      validateProfileDraft(
        activeProfile,
        activeProfile.name,
        activeProfile.runner,
        activeProfile.kind === "ssh" ? activeProfile.host : "",
        activeProfile.kind === "ssh" ? activeProfile.clientTty : "",
      ),
    [activeProfile],
  );
  const activeProfileValid = activeProfileValidation.errors.length === 0;

  // Drive the Preflight service to the active runner. Only the dock probes (it gates the
  // live picker); the settings window reuses the dock's result over the prefs channel
  // instead of firing its own (which for SSH would be a second `ssh … --version` — an
  // extra round-trip and a possible duplicate passphrase prompt). The service supersedes
  // an in-flight probe on the next target the way the old `cancelled` flag did, keeps the
  // previous ready result during a switch, and mirrors each result to settings.
  useEffect(() => {
    if (mode !== "dock") {
      return;
    }
    // Precompute the synchronous validation: an invalid profile short-circuits the probe
    // to a synthetic failed preflight (binary label + joined messages), matching the old
    // loadShellState invalid branch; null means "probe the CLI".
    const invalid = activeProfileValid
      ? null
      : {
          binary: commandPrefix(activeProfile),
          error: activeProfileValidation.errors.join(" "),
        };
    configurePreflight({ settings: runnerSettings, runnerKey, invalid });
    // Keyed on runnerKey (the active runner's identity), NOT profileState/runnerSettings:
    // editing or deleting an INACTIVE profile syncs a fresh profileState but leaves the
    // active runner unchanged, so re-running here would needlessly re-probe (an extra ssh
    // --version / passphrase prompt) for a target that didn't change. activeProfile/
    // runnerSettings/validation are fully determined by runnerKey where this reads them.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [runnerKey, mode, configurePreflight]);

  const liveReady =
    preflightState.status === "ready" &&
    preflightState.runnerKey === runnerKey &&
    preflightState.preflight.ok &&
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
    if (
      preflightState.status === "loading" ||
      (preflightState.status === "ready" && preflightState.runnerKey !== runnerKey)
    ) {
      return `Checking ${profileKindLabel(activeProfile)} CLI`;
    }

    if (preflightState.status === "failed") {
      return "IPC failed";
    }

    return preflightState.preflight.ok
      ? `${profileKindLabel(activeProfile)} CLI ready`
      : `${profileKindLabel(activeProfile)} CLI unavailable`;
  }, [activeProfile, preflightState, runnerKey]);

  // `preflightState` lags the active runner by one async cycle after a switch or settings
  // apply (the service resolves the new probe asynchronously). Until the resolved state's
  // runnerKey matches the active runner, its preflight/picker rows belong to the previous
  // target, so treat that window as "loading" everywhere ready data is consumed.
  const activeReadyState =
    preflightState.status === "ready" && preflightState.runnerKey === runnerKey
      ? preflightState
      : null;
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
      ? pickerStateFromLive(live, runnerKey)
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

  // Apply the theme to <html data-theme>. "system" resolves from prefers-color-scheme
  // and re-resolves live when the OS appearance changes. Persistence + the cross-window
  // broadcast are owned by the Appearance service (driven by the setter); this effect
  // only applies the resolved theme to the DOM and the logo variant.
  useEffect(() => {
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

  // The layout preference is persisted by the Appearance service (driven by the setter);
  // window shaping for the current orientation is handled by the effect below.
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
    // Pinned horizontal locks the bar to BAR_WINDOW_HEIGHT (min == max height) so it can
    // only be resized horizontally — the layout is tuned for that exact height. Pinned
    // vertical caps width into a strip. "auto" stays free on both axes (just a min floor
    // matched to the live shape).
    const minSize =
      orientationPref === "horizontal"
        ? new LogicalSize(WINDOW_MIN_WIDTH, BAR_WINDOW_HEIGHT)
        : new LogicalSize(
            WINDOW_MIN_WIDTH,
            sizingOrientation === "horizontal"
              ? WINDOW_MIN_HEIGHT_HORIZONTAL
              : WINDOW_MIN_HEIGHT_VERTICAL,
          );
    const maxSize =
      orientationPref === "vertical"
        ? new LogicalSize(WINDOW_MAX_WIDTH_VERTICAL, WINDOW_MAX_UNBOUNDED)
        : orientationPref === "horizontal"
          ? new LogicalSize(WINDOW_MAX_UNBOUNDED, BAR_WINDOW_HEIGHT)
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
        await win.setMinSize(minSize);
        await win.setMaxSize(maxSize);
        if (shouldPlace) {
          await summonPlacementRef.current();
        }
      } catch {
        // Best-effort: a failed update leaves the prior constraints/shape in place.
      }
    });
  }, [mode, orientationPref, effectiveOrientation]);

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

  // Apply the cross-window `profiles` sync. Appearance (theme/orientation/glass) and
  // preflight syncs are consumed by their own Effect services over the same channel
  // (PrefsBridge installs its own listener), so only the React-synchronously-gated
  // `profiles` adoption remains here; the handler never re-broadcasts, so A -> B -> A
  // can't loop.
  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | null = null;
    void listen<PrefsSync>(PREFS_SYNC_EVENT, (event) => {
      const payload = event.payload;
      if (payload.kind === "profiles") {
        // The Profiles service owns persistence + the reload primitive, but the
        // adopt/skip DECISION stays here because it gates on the settings form's
        // unsaved-edit flag — React-synchronous state. Reading isSettingsDirtyRef in
        // the event handler (not a lagged service Ref) preserves the original
        // guarantee: a dock-side source switch never clobbers an in-progress edit (the
        // window is hidden, not closed, precisely to preserve it). The dock always
        // adopts so its live picker tracks the current profile config; the skipped
        // change is reconciled later via the focus/clean reload paths.
        //
        // The reload now applies via an async hop (atom dispatch -> Effect fiber ->
        // service ref -> re-render) rather than a setProfileState called inline here. Note
        // that old setProfileState was itself a batched React setState, NOT a synchronous
        // reconcile, so the same draft-reset-on-active-change window already existed; the
        // hop only widens it by a few microtasks (sub-millisecond). The trigger stays
        // synchronously dirty-gated, and the reload is value-guarded (an equal snapshot
        // leaves the ref untouched, so the [activeProfile] reset effect never fires) —
        // strictly safer than the old unconditional setProfileState, which reinstalled
        // state even on a no-op sync. The only residual is that sub-ms window where an edit
        // begun between this dirty check and a genuine-change reload landing could be
        // reset; closing it fully would mean keeping ProfileState in React (precisely what
        // this migration removes). The focus/clean reconcilers recover state on next focus.
        if (mode === "settings" && isSettingsDirtyRef.current) {
          return;
        }
        reloadProfiles();
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

  // The service listeners above can miss a change broadcast before they registered, and
  // emitTo has no replay — so a warm (hidden) settings window could linger on stale
  // state with no way to recover short of a restart. Reconcile from storage whenever this
  // window gains focus (i.e. is shown/reopened). Both reconciles are value-guarded, so an
  // unchanged snapshot is a no-op; the profile is additionally skipped while there are
  // unsaved edits, so a kept-warm in-progress edit is never clobbered.
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
        reloadAppearance();
        if (!isSettingsDirtyRef.current) {
          reloadProfiles();
        }
        // Preflight has no localStorage to re-read and emitTo has no replay, so ask the
        // dock (via the Preflight service) to re-emit its current result. Covers a
        // preflight broadcast missed while this window was hidden/unfocused (it isn't
        // subscribed to the live picker, so it can't recompute the result itself) —
        // fixing a card stuck on "Checking" after a dock-side source switch it didn't see.
        requestPreflightSync();
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
  }, [mode, reloadAppearance, reloadProfiles, requestPreflightSync]);

  // The React listener drops dock-side syncs while this window is dirty, and the focus-
  // reconcile only runs on a later focus — so a change skipped mid-edit would linger
  // after Reset/Apply clears the drafts (still focused, no new focus event). Reconcile
  // from storage whenever the window is clean. The service's reload is value-guarded,
  // so an unchanged snapshot doesn't churn state or reset the picker selection.
  useEffect(() => {
    if (mode !== "settings" || isSettingsDirty) {
      return;
    }
    reloadProfiles();
  }, [mode, isSettingsDirty, reloadProfiles]);

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
  // Persistence + the cross-window mirror are owned by the Appearance service; this
  // effect only applies the native vibrancy, which lives on the dock (the settings
  // window is a solid, normally-chromed window and never frosts itself).
  useEffect(() => {
    if (!IS_MAC || mode !== "dock") {
      return;
    }

    let cancelled = false;
    glassOperationQueue = glassOperationQueue.then(async () => {
      // A newer toggle superseded this one before it ran; skip the native call
      // entirely so the queue settles on the latest desired state.
      if (cancelled) {
        return;
      }
      // Round the vibrancy backdrop to match the frameless CSS corners; null lets a framed
      // window's native rounding apply. Keyed on the APPLIED frameless state (like the CSS
      // rounding via data-frameless), not the bare preference, so the frost only rounds once
      // the frame is actually gone. Re-applied whenever that state changes (dep below).
      const radius = framelessApplied ? FRAMELESS_CORNER_RADIUS : null;
      try {
        if (glassEnabled) {
          await invoke("set_window_glass", { enabled: true, radius });
          if (!cancelled) {
            // Flip the surface translucent and arm the adaptive tokens together,
            // so `--glass-clear` is only nonzero once the blur is actually live.
            document.documentElement.setAttribute("data-glass", "on");
            setGlassClear(glassClearFor(surfaceAlphaRef.current));
          }
        } else {
          document.documentElement.setAttribute("data-glass", "off");
          setGlassClear(0);
          await invoke("set_window_glass", { enabled: false, radius });
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
  }, [glassEnabled, framelessApplied, mode]);

  // Drive the tint opacity via a CSS variable; the data-glass rules in styles.css
  // only consume it while glass is on, so this is harmless when glass is off.
  // `--glass-clear` (a 0..1 see-through scalar the adaptive tokens interpolate
  // against) is owned by the glass-toggle effect so it stays in lockstep with the
  // actual data-glass state, not the pending React intent. Here we only refresh it
  // for slider moves while glass is already live; on/off transitions are that
  // effect's job. Persistence is owned by the Appearance service (driven by the setter).
  useEffect(() => {
    const root = document.documentElement;
    root.style.setProperty("--surface-alpha", String(surfaceAlpha));
    if (root.getAttribute("data-glass") === "on") {
      setGlassClear(glassClearFor(surfaceAlpha));
    }
  }, [surfaceAlpha]);

  // Apply the frameless-chrome preference to the dock window. Like glass, this is a
  // dock-only native apply (the settings window keeps its normal frame) owned by React,
  // while persistence + the cross-window mirror live in the Appearance service. The
  // data-frameless attribute is what surfaces the custom drag region + window controls in
  // styles.css, so it's flipped only once set_window_decorations resolves — the controls
  // never render over a still-framed window, and a failed native call leaves the attribute
  // "off" so we don't strip the only chrome without a working replacement. Serialized
  // through a queue so a fast toggle settles on the latest desired state.
  useEffect(() => {
    if (mode !== "dock") {
      return;
    }

    let cancelled = false;
    framelessOperationQueue = framelessOperationQueue.then(async () => {
      if (cancelled) {
        return;
      }
      try {
        await invoke("set_window_decorations", { decorations: !framelessEnabled });
        if (!cancelled) {
          document.documentElement.setAttribute(
            "data-frameless",
            framelessEnabled ? "on" : "off",
          );
          // Reveal/hide the custom chrome only now that the native frame change landed, so
          // it tracks the real window state rather than the pending preference.
          setFramelessApplied(framelessEnabled);
        }
      } catch (error) {
        // The native call failed: assume the frame is still present and hide the custom
        // chrome, so we never stack our controls on top of a native titlebar.
        document.documentElement.setAttribute("data-frameless", "off");
        setFramelessApplied(false);
        appendDebugEntry({
          kind: "command",
          label: "Frameless window",
          detail: errorMessage(error),
        });
      }
    });

    return () => {
      cancelled = true;
    };
  }, [framelessEnabled, mode]);

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
      // filter if one is active; otherwise it's a no-op. In the horizontal bar it also
      // collapses search back to its icon (inert in the always-expanded vertical strip).
      if (pickerFilter) {
        event.preventDefault();
        setPickerFilter("");
      }
      setIsSearchExpanded(false);
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

    // Normalize the draft (and reflect it in the form), then hand the edit to the
    // service, which merges it onto the latest persisted state, persists, and
    // broadcasts. Validation + the debug log stay here; persistence is the service's.
    const normalized = normalizeRunnerSettings(settingsDraft);
    setSettingsDraft(normalized);
    applyRunnerSettingsSet({
      name: profileNameDraft,
      runner: normalized,
      sshHost: sshHostDraft,
      sshClientTty: sshClientTtyDraft,
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

  // The persisted-state mutators live in the Profiles service, which owns the
  // merge-onto-latest discipline, the same-active no-op, and the cross-window
  // broadcast. These thin wrappers just dispatch the intent — and the mutator runs
  // once in an Effect fiber, so the old StrictMode generate-id-once dance in
  // addSshProfile is no longer needed.
  function selectProfile(id: string) {
    // A dirty settings window may highlight a stale active card (it skips dock-side
    // profile syncs mid-edit, and the focus/clean reconcilers also skip while dirty),
    // so the highlighted card (activeProfile.id) can lag the persisted active source.
    // Re-clicking that already-highlighted card must stay a no-op — otherwise the
    // service's id !== latest path would rewrite storage to our stale id and flip the
    // dock off the source it was switched to elsewhere. The dirty flag is React-
    // synchronous, so this guard belongs here, ahead of the dispatch. Clean windows
    // and the dock are untouched (their highlight matches the persisted active).
    if (mode === "settings" && isSettingsDirty && id === activeProfile.id) {
      return;
    }
    selectProfileSet(id);
  }

  function addSshProfile() {
    addSshProfileSet();
  }

  function deleteActiveProfile() {
    // Guard here too so the debug entry (and its reference to the about-to-be-deleted
    // profile's name) only fires for a real deletion; the service also no-ops on local.
    if (activeProfile.kind === "local") {
      return;
    }
    deleteActiveProfileSet();
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

  // Move focus into the search field the moment it expands (horizontal bar), so a click
  // on the search icon lands the caret without a second click. Only fires on the
  // false->true transition; the field is unmounted while collapsed.
  useEffect(() => {
    if (isSearchExpanded) {
      searchInputRef.current?.focus();
    }
  }, [isSearchExpanded]);

  // The dock shows its boot/error screen until the active runner has a usable
  // preflight. That covers three not-ready cases: still probing; the probe failed
  // (IPC error); or the probe resolved but reports the CLI unavailable (bad binary
  // path / SSH target) for the CURRENT runner. The last case used to fall through to
  // a perpetual "Waiting for a source" live banner that never explained what broke —
  // routing it here surfaces the real preflight error and the Open settings recovery
  // path. A stale ready state from a profile still switching (runnerKey mismatch) is
  // left to the picker's "Switching…" view, not treated as an error.
  const dockPreflightUnusable =
    preflightState.status === "ready" &&
    preflightState.runnerKey === runnerKey &&
    !preflightState.preflight.ok;
  // Custom window chrome for frameless mode, shared by the boot/recovery screen and the
  // picker below. Callers gate these on framelessApplied so they only appear once the native
  // frame is actually gone. data-tauri-drag-region="" adds the drag handle; undefined omits
  // it (the chrome bands only become draggable when frameless).
  const dragRegion = framelessApplied ? "" : undefined;
  const windowControls = (
    <>
      <button
        className="icon-button"
        type="button"
        aria-label="Minimize window"
        title="Minimize"
        onClick={() => void getCurrentWindow().minimize()}
      >
        {"–"}
      </button>
      <button
        className="icon-button window-close"
        type="button"
        aria-label="Close window"
        title="Close"
        onClick={() => void getCurrentWindow().hide()}
      >
        {"×"}
      </button>
    </>
  );

  if (mode === "dock" && (preflightState.status !== "ready" || dockPreflightUnusable)) {
    const probing = preflightState.status === "loading";
    const detail =
      preflightState.status === "failed"
        ? preflightState.message
        : preflightState.status === "ready"
          ? (preflightState.preflight.error ?? `${profileKindLabel(activeProfile)} CLI unavailable`)
          : "Waiting for the daemon…";
    return (
      // Recovery UI renders in the live orientation: a centered column in the vertical
      // strip, and a compact row in the horizontal bar (styles.css) so the heading and
      // the only "Open settings" path stay visible without clipping in the short bar.
      <main
        className="sidebar"
        data-orientation={effectiveOrientation}
        data-tauri-drag-region={dragRegion}
      >
        {/* The drag region must sit on boot-state too, not just <main>: boot-state fills the
            window (height:100% / flex:1), so Tauri — which starts a drag only when the click
            target itself carries the attribute — would otherwise see every click land on this
            covering child and never drag. Clicks on the spinner/copy/button target those
            elements (no attribute), so they stay non-draggable. */}
        <div className="boot-state" aria-live="polite" data-tauri-drag-region={dragRegion}>
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
        {/* Frameless mode strips the native frame, so the recovery screen would otherwise
            be a borderless window the user can't move/minimize/dismiss while connecting or
            after a failure. The boot screen has no footer, so float the controls instead. */}
        {framelessApplied ? (
          <div className="boot-window-controls">{windowControls}</div>
        ) : null}
      </main>
    );
  }

  if (mode === "settings") {
    // The profile list comes from profileState (the live source of truth) so
    // add/delete/switch are reflected immediately. The settings window never runs
    // its own preflight (which for SSH would be a duplicate `ssh … --version`); it
    // reuses the dock's, mirrored by the Preflight service into `syncedPreflight`. That
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
                  onClick={() => setThemePref(option)}
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
                    onClick={() => setOrientationPref(option)}
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

            <div className="setting-row">
              <div className="setting-label">
                <span>Frameless</span>
                <span className="setting-hint">
                  Hide the title bar; drag, minimize, and close from the dock itself
                </span>
              </div>
              <button
                className={`switch${framelessEnabled ? " on" : ""}`}
                type="button"
                role="switch"
                aria-checked={framelessEnabled}
                aria-label="Frameless window"
                onClick={() => setFrameless(!framelessEnabled)}
              >
                <span className="switch-thumb" />
              </button>
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
                    onClick={() => setGlassEnabled(!glassEnabled)}
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
                      onChange={(event) =>
                        setSurfaceAlpha(SURFACE_ALPHA_MAX - Number(event.target.value))
                      }
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

  // Horizontal bar: collapse search to an icon unless the user expanded it or a query is
  // active. The vertical strip always shows the full field (searchCollapsed is never true).
  const searchCollapsed =
    effectiveOrientation === "horizontal" && !isSearchExpanded && !pickerFilter.trim();

  return (
    <main className="sidebar" data-orientation={effectiveOrientation}>
      <header className="topbar" data-tauri-drag-region={dragRegion}>
        {searchCollapsed ? (
          <button
            className="icon-button search-expand"
            type="button"
            aria-label="Search agents"
            title="Search agents"
            onClick={() => setIsSearchExpanded(true)}
          >
            {"⌕"}
          </button>
        ) : (
          <div className="search-field">
            <span className="search-icon" aria-hidden="true">
              {"⌕"}
            </span>
            <input
              ref={searchInputRef}
              aria-label="Search agents"
              value={pickerFilter}
              onChange={(event) => setPickerFilter(event.target.value)}
              placeholder="Search agents"
              onBlur={() => {
                // Leaving an empty field collapses it back to the icon (bar only). Defer the
                // collapse past the current event: blur is a discrete event, so collapsing
                // here flushes the reflow synchronously and the adjacent reconnect button
                // shifts left between mousedown and click — eating the click. A macrotask runs
                // after the click resolves, so the neighbor's onClick lands first.
                if (effectiveOrientation === "horizontal" && !pickerFilter.trim()) {
                  setTimeout(() => setIsSearchExpanded(false), 0);
                }
              }}
              onKeyDown={(event) => {
                // Horizontal bar only: Escape clears the query, collapses to the icon, and
                // blurs — which unmounts the field so the global key handler (it ignores
                // input targets) resumes j/k nav. Left as-is in the vertical strip, where
                // the field is permanent and a field-level Escape was already a no-op.
                if (event.key === "Escape" && effectiveOrientation === "horizontal") {
                  event.preventDefault();
                  if (pickerFilter) {
                    setPickerFilter("");
                  }
                  setIsSearchExpanded(false);
                  event.currentTarget.blur();
                }
              }}
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
        )}
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

      <footer className="bottombar" data-tauri-drag-region={dragRegion}>
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
        {/* Settings and (when frameless) the window controls are one trailing group with a
            single even gap, so the spacing reads as ⚙ − × rather than settings floating off
            from a tight min/close pair. The controls are gated on framelessApplied (the
            applied native state), so they only appear once the titlebar is actually gone —
            close hides the window (dismiss), matching Escape and the summonable-dock model. */}
        <div className="bottombar-actions">
          <button
            className="icon-button"
            type="button"
            aria-label="Settings"
            title="Settings"
            onClick={openSettings}
          >
            {"⚙"}
          </button>
          {framelessApplied ? windowControls : null}
        </div>
      </footer>
    </main>
  );
}

// Project the LiveConnection service's state onto the PickerState the GroupedPicker
// renders. The service is the single owner of rows + connection status; this just
// picks the view: keep showing the last rows while (re)connecting so the list
// doesn't flash a skeleton on a brief blip, show the failure only when a fatal
// state has actually cleared the rows, and otherwise a loading skeleton.
//
// Rows are trusted only when their producing runner (rowsRunnerKey) matches the
// active one. After a source switch the service preserves the previous runner's rows
// through the new subscription's connecting window (so a same-runner reconnect won't
// flicker), and `state`-derived readiness (preflight) can resolve for the new runner
// before that subscription's first snapshot — without this gate the dock would briefly
// render the prior source's panes and activate one against the new runner's settings.
function pickerStateFromLive(live: LiveState, activeRunnerKey: string): PickerState {
  const { connection } = live;
  const rows = live.rowsRunnerKey === activeRunnerKey ? live.rows : [];
  if (rows.length > 0) {
    return { status: "ready", rows };
  }
  if (connection.status === "fatal") {
    return { status: "failed", message: connection.message };
  }
  if (connection.status === "connecting" || connection.status === "reconnecting") {
    return { status: "loading" };
  }
  // online or noDaemon with no (matching) rows → an empty (but resolved) list.
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

function projectOf(row: PickerRow): string {
  const workspaceLabel = row.workspace?.label?.trim();
  if (workspaceLabel) {
    return workspaceLabel;
  }

  const tag = row.location_tag.trim();
  const session = tag.split(":", 1)[0]?.trim();
  return session || "ungrouped";
}

function projectKeyOf(row: PickerRow): string {
  const workspaceId = row.workspace?.id?.trim();
  if (workspaceId) {
    return workspaceId;
  }

  return projectOf(row);
}

function paneSuffix(row: PickerRow): string {
  const tag = row.location_tag.trim();
  if (row.workspace?.source && row.workspace.source !== "session") {
    return tag;
  }

  const colon = tag.indexOf(":");
  return colon >= 0 ? tag.slice(colon + 1) : "";
}

// Group rows by backend workspace context, preserving first-seen order both
// across groups and within each group so keyboard nav matches what's rendered.
function groupRowsByProject(rows: PickerRow[]): PickerGroup[] {
  const groups: PickerGroup[] = [];
  const byProject = new Map<string, PickerGroup>();

  for (const row of rows) {
    const projectKey = projectKeyOf(row);
    const project = projectOf(row);
    let group = byProject.get(projectKey);
    if (!group) {
      group = { key: projectKey, project, rows: [] };
      byProject.set(projectKey, group);
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
      row.workspace?.label ?? "",
      row.workspace?.source ?? "",
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
    // Restore first: macOS show() does NOT un-minimize a minimized window, so summoning a
    // dock the user minimized (now reachable via the frameless minimize button) would
    // silently no-op without this. Mirrors openSettings's unminimize-before-show order.
    await appWindow.unminimize();
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
        <section className="picker-group" key={group.key}>
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
