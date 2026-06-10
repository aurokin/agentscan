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
  liveStatesAtom,
  preflightStateAtom,
  profilesAtom,
  reconnectAtom,
  reloadAppearanceAtom,
  reloadProfilesAtom,
  reorderProfileAtom,
  requestPreflightSyncAtom,
  selectProfileAtom,
  setFramelessAtom,
  setGlassEnabledAtom,
  setOrientationAtom,
  setSurfaceAlphaAtom,
  setThemeAtom,
  startAtom,
  syncedPreflightAtom,
  toggleProfileOpenAtom,
} from "./effect/atoms";
import type { ConnectionStatus, LiveState, PickerRow } from "./effect/types";
import { liveStateFor, type LiveStates } from "./effect/LiveConnection";
import { pickerRowForKeyboardKey } from "./effect/keybinds";
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
  folderProfiles,
  getActiveProfile,
  keybindOwnerId,
  loadProfileState,
  normalizeRunnerSettings,
  profileKindLabel,
  runnerKeyForProfile,
  runnerSettingsEqual,
  runnerSettingsForProfile,
  runnerSummary,
  sourceLabel,
  validateProfileDraft,
  type DesktopProfileConfig,
  type EnvironmentVariable,
  type PreflightLabelSource,
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

// How long a failed activation's error strip stays up. Long enough to read,
// short enough that one-shot action feedback doesn't linger as a standing
// condition (the full error remains in the debug log).
const ACTIVATION_FAILURE_TTL_MS = 10_000;
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
// reconnect/latch policy) is owned by the Effect LiveConnection service, as a
// per-source map keyed by runnerKey. This component drives it via
// configure/reconnect/start and observes liveStatesAtom, reading the active
// runner's entry through liveStateFor (which supplies the initial fallback).
const EMPTY_LIVE_STATES: LiveStates = new Map<string, LiveState>();

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
// in the LiveConnection service (liveStatesAtom).
const INITIAL_PREFLIGHT: PreflightState = { status: "loading" };

type PickerState =
  | { status: "loading" }
  | { status: "ready"; rows: PickerRow[] }
  | { status: "failed"; message: string };

// Activations are tagged with the runnerKey of the row's OWN source so the
// running pulse / failure recovery scope to that source's folder (pane ids like
// %1 collide across hosts). A null sourceKey marks a source-less failure (the
// summon-hotkey registration error reuses this banner).
type PickerActivation =
  | { status: "idle" }
  | { status: "running"; paneId: string; sourceKey: string }
  | { status: "failed"; message: string; sourceKey: string | null };

// Stable empty fallbacks for the no-owner case so effect dep arrays don't churn.
const EMPTY_PICKER_ROWS: PickerRow[] = [];
const EMPTY_PICKER_GROUPS: PickerGroup[] = [];

// Source-kind mark: Lucide "house" / "server" outlines (ISC), inlined so the
// mark renders crisply at small sizes instead of leaning on font glyph
// coverage. Each context sizes it via font-size (the icon is 1em square).
function SourceKindIcon({ kind }: { kind: "local" | "ssh" }) {
  return (
    <svg
      className="source-kind-icon"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {kind === "ssh" ? (
        <>
          <rect width="20" height="8" x="2" y="2" rx="2" ry="2" />
          <rect width="20" height="8" x="2" y="14" rx="2" ry="2" />
          <path d="M6 6h.01" />
          <path d="M6 18h.01" />
        </>
      ) : (
        <>
          <path d="M15 21v-8a1 1 0 0 0-1-1h-4a1 1 0 0 0-1 1v8" />
          <path d="M3 10a2 2 0 0 1 .709-1.528l7-5.999a2 2 0 0 1 2.582 0l7 5.999A2 2 0 0 1 21 10v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
        </>
      )}
    </svg>
  );
}

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
  // Promise mode so the horizontal footer's settings deep-link can await the
  // selection commit before opening the window; other callers ignore the promise.
  const selectProfileSet = useAtomSet(selectProfileAtom, { mode: "promise" });
  const addSshProfileSet = useAtomSet(addSshProfileAtom);
  const deleteActiveProfileSet = useAtomSet(deleteActiveProfileAtom);
  // Promise mode: the apply outcome ("applied" | "duplicate-host") drives the
  // debug-log entry, so a commit-time refusal is never reported as applied.
  const applyRunnerSettingsSet = useAtomSet(applyRunnerSettingsAtom, { mode: "promise" });
  const toggleProfileOpenSet = useAtomSet(toggleProfileOpenAtom);
  const reorderProfileSet = useAtomSet(reorderProfileAtom);
  const reloadProfiles = useAtomSet(reloadProfilesAtom);
  const activeProfile = useMemo(() => getActiveProfile(profileState), [profileState]);
  const runnerSettings = useMemo(() => runnerSettingsForProfile(activeProfile), [activeProfile]);
  // Identity of the exact runner configuration a resolved state describes. It
  // changes on a profile switch AND on any settings edit (binary/env/host/tty)
  // to the active profile, so resolved preflight/picker data is invalidated
  // whenever the underlying target changes, not just when the profile id does.
  const runnerKey = useMemo(() => runnerKeyForProfile(activeProfile), [activeProfile]);
  // Row keybinds are owned by exactly one source: the topmost OPEN folder in the
  // user's source order (null when every folder is closed).
  const ownerProfileId = useMemo(() => keybindOwnerId(profileState), [profileState]);
  // The folder-eligible sources in order, each with its runner identity, open
  // state, ownership, and committed-profile validity (the arm gate for non-active
  // sources, whose preflight is never probed).
  const liveSources = useMemo(
    () =>
      folderProfiles(profileState).map((profile) => ({
        profile,
        runnerKey: runnerKeyForProfile(profile),
        settings: runnerSettingsForProfile(profile),
        isOpen: profileState.openProfileIds.includes(profile.id),
        isOwner: profile.id === ownerProfileId,
        valid:
          validateProfileDraft(
            profile,
            profile.runner,
            profile.kind === "ssh" ? profile.host : "",
            profile.kind === "ssh" ? profile.clientTty : "",
            profileState.profiles,
          ).errors.length === 0,
      })),
    [profileState, ownerProfileId],
  );
  const ownerSource = useMemo(() => liveSources.find((s) => s.isOwner) ?? null, [liveSources]);
  const ownerProfile = ownerSource?.profile ?? null;
  const ownerRunnerKey = ownerSource?.runnerKey ?? null;
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
  // observes liveStatesAtom — a per-source map — and drives the service via
  // configure/reconnect/start, reading each open folder's entry by runnerKey. The
  // settings window mounts these too (separate webview/runtime) but never configures
  // a target, so its supervisors stay idle.
  const liveResult = useAtomValue(liveStatesAtom);
  const liveStates = Result.getOrElse(liveResult, () => EMPTY_LIVE_STATES);
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
  // Mid-drag source id for the footer order menu (the dock-side counterpart of
  // the settings rail's draggedSourceId).
  const [draggedMenuSourceId, setDraggedMenuSourceId] = useState<string | null>(null);
  const sourceMenuRef = useRef<HTMLDivElement | null>(null);
  // The local machine's short hostname, fetched once from the backend, shown as the
  // local source's label (the way a remote source is keyed by its SSH host). Empty
  // until it resolves; sourceLabel falls back to a generic label in the meantime.
  const [localHostLabel, setLocalHostLabel] = useState("");
  // The probed remote hostname as a label source: the dock from its own resolved
  // preflight, settings from the dock's mirror. sourceLabel only honors it for the
  // profile whose runnerKey matches, so a stale probe can never label a source.
  const labelPreflight: PreflightLabelSource | null =
    mode === "dock"
      ? preflightState.status === "ready"
        ? preflightState
        : null
      : syncedPreflight;
  // One label rule for every card/menu/header: sourceLabel sees the sibling
  // sources so a probed hostname that collides with another source's label is
  // dropped for the configured connection string.
  const labelFor = (profile: DesktopProfileConfig) =>
    sourceLabel(profile, localHostLabel, labelPreflight, profileState.profiles);
  // Debug log is a diagnostic panel — collapsed by default to keep Settings calm.
  const [isDebugOpen, setIsDebugOpen] = useState(false);
  // The source card being dragged in the settings rail (HTML5 drag-and-drop);
  // dropping on another card reorders the sources, which keybind ownership follows.
  const [draggedSourceId, setDraggedSourceId] = useState<string | null>(null);
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
  // The runnerKeys of the OPEN folders, render-synced so an in-flight activation's
  // completion can tell whether its source is still open (a closed/edited source
  // has no list to recover, so its failure is dropped instead of surfaced).
  const openRunnerKeys = useMemo(
    () => new Set(liveSources.filter((source) => source.isOpen).map((source) => source.runnerKey)),
    [liveSources],
  );
  const openRunnerKeysRef = useRef(openRunnerKeys);
  openRunnerKeysRef.current = openRunnerKeys;
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
  // click sees the in-flight activation and bails. It holds a per-activation
  // token (not a boolean) because the guard can be freed EARLY — closing the
  // in-flight source releases it so other sources aren't blocked behind a wedged
  // invoke — and the stale activation's settle must then leave a newer
  // activation's guard alone.
  const activationInFlightRef = useRef<symbol | null>(null);
  const validation = useMemo(
    () =>
      validateProfileDraft(
        activeProfile,
        settingsDraft,
        sshHostDraft,
        sshClientTtyDraft,
        profileState.profiles,
      ),
    [activeProfile, settingsDraft, sshHostDraft, sshClientTtyDraft, profileState.profiles],
  );
  const isSettingsDirty = useMemo(
    () =>
      sshHostDraft.trim() !== (activeProfile.kind === "ssh" ? activeProfile.host : "") ||
      sshClientTtyDraft.trim() !==
        (activeProfile.kind === "ssh" ? activeProfile.clientTty : "") ||
      !runnerSettingsEqual(settingsDraft, activeProfile.runner),
    [activeProfile, settingsDraft, sshHostDraft, sshClientTtyDraft],
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
        activeProfile.runner,
        activeProfile.kind === "ssh" ? activeProfile.host : "",
        activeProfile.kind === "ssh" ? activeProfile.clientTty : "",
        profileState.profiles,
      ),
    [activeProfile, profileState.profiles],
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

  // Drive the LiveConnection service to every OPEN folder's source: open folder =
  // live subscription, closed folder = none. The service owns the subscriptions,
  // epoch fencing, reconnect/latch backoff, and recovery; this just tells it WHICH
  // runners to track and WHETHER each is ready. The active (settings-selected)
  // source gates on its resolved preflight, but only once that resolution
  // describes the CURRENT runnerKey: the probe lags a switch by one async cycle,
  // and gating off during that window would bounce (teardown + full reconnect —
  // over SSH a whole remote process respawn) the healthy, already-armed
  // subscription of a source the user merely re-selected in Settings. While
  // unresolved, the previous armed value is carried; a key with no previous value
  // (launch, or an in-place edit that moved the runnerKey) stays gated off until
  // its probe resolves, exactly the old behavior. Other open sources are never
  // probed, so they arm on their committed-profile validity and surface failures
  // per folder through their keyed live state. configure diffs on runnerKey +
  // enabled, so re-running on an unrelated profileState change leaves running
  // keys alone.
  const prevLiveEnabledRef = useRef<ReadonlyMap<string, boolean>>(new Map());
  // THE invariant probe results live under: a probe gates STARTING a channel; an
  // ONLINE channel is never killed or masked by probe verdicts. Probes are one-shot
  // (they re-fire only on a runnerKey change) while the channel is continuous, so a
  // same-key probe failing — transiently or even resolving CLI-unavailable — after
  // the source is streaming must not tear down or hide healthy rows: there would be
  // no recovery probe to undo it. Config edits still tear down via the key change,
  // and a channel that drops falls back to probe gating. The channel reports its
  // own failures via LiveStrip.
  const activeLiveOnline = liveStateFor(liveStates, runnerKey).connection.status === "online";
  const liveTargets = useMemo(
    () =>
      liveSources
        .filter((source) => source.isOpen)
        .map((source) => ({
          settings: source.settings,
          runnerKey: source.runnerKey,
          enabled:
            source.runnerKey === runnerKey
              ? activeLiveOnline ||
                (preflightState.status === "ready" && preflightState.runnerKey === runnerKey
                  ? preflightState.preflight.ok && activeProfileValid
                  : (prevLiveEnabledRef.current.get(source.runnerKey) ?? false))
              : source.valid,
        })),
    [liveSources, runnerKey, activeLiveOnline, preflightState, activeProfileValid],
  );
  useEffect(() => {
    if (mode !== "dock") {
      return;
    }
    configureLive(liveTargets);
    // Record what was armed only after configuring, so the carry above always
    // reads the last value the service actually saw.
    prevLiveEnabledRef.current = new Map(
      liveTargets.map((target) => [target.runnerKey, target.enabled]),
    );
  }, [mode, liveTargets, configureLive]);

  // Resolve the local machine's hostname once for the local source label. Both the
  // dock and the settings window render it, so this runs ungated; a failure just
  // leaves the generic fallback in place.
  useEffect(() => {
    let cancelled = false;
    void invoke<string>("local_host_label")
      .then((label) => {
        if (!cancelled) {
          setLocalHostLabel(label);
        }
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

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
            sourceKey: null,
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
  // runnerKey matches the active runner, it belongs to the previous target, so the footer
  // tone treats that window as unknown.
  const activeReadyState =
    preflightState.status === "ready" && preflightState.runnerKey === runnerKey
      ? preflightState
      : null;
  // Tone for the footer status dot when the footer shows the active source, derived
  // from its resolved preflight (not a stale previous one).
  const sourceStatusTone = !activeReadyState
    ? "unknown"
    : activeReadyState.preflight.ok
      ? "idle"
      : "error";
  // The active runner's preflight is unusable when the probe resolved for the
  // CURRENT runner but reports the CLI unavailable (bad binary path / SSH target).
  // A stale ready state from a profile still switching (runnerKey mismatch) is not
  // an error — the folders keep rendering their keyed live states while the new
  // probe resolves.
  const dockPreflightUnusable =
    preflightState.status === "ready" &&
    preflightState.runnerKey === runnerKey &&
    !preflightState.preflight.ok;
  // The active source's failure surfaced inside its own folder (or the homeless
  // strip below): its live target is gated off (or left disarmed) on a failed
  // probe, so without this the folder's keyed state would read as a dishonest
  // perpetual "Waiting for a source". Covers a resolved-but-unusable probe, and
  // the probe itself failing ("failed" carries no runnerKey; like the boot screen,
  // we treat it as the active runner's) — unless the channel is already online,
  // per the probes-gate-starting invariant above liveTargets.
  const activePreflightError = activeLiveOnline
    ? null
    : dockPreflightUnusable
      ? (preflightState.preflight.error ?? `${profileKindLabel(activeProfile)} CLI unavailable`)
      : preflightState.status === "failed"
        ? preflightState.message
        : null;
  // The full-screen boot/recovery takeover is scoped to the states where no OTHER
  // open folder could render anyway: preflight is single-source (only the active
  // profile is probed), so blanking the whole dock for the active source's
  // boot/failure would hide healthy open folders — exactly the independence the
  // folder model exists for. With another folder open, the active source's failure
  // stays inside its own folder (status dot + error strip) instead.
  const hasOpenFolderBeyondActive = liveSources.some(
    (source) => source.isOpen && source.runnerKey !== runnerKey,
  );
  // …and it also requires the active source to be PARTICIPATING (its folder open):
  // a closed folder is header-only with no subscription, so its loading/failing
  // preflight must not take over a dock the user deliberately quieted — that would
  // hide the folder list (the only way to reopen anything). A homeless active
  // source (no folder) surfaces through the error strip above the folders instead.
  const activeFolderOpen = liveSources.some(
    (source) => source.isOpen && source.runnerKey === runnerKey,
  );
  const dockBootScreenVisible =
    mode === "dock" &&
    // No probe verdict blanks the dock over an online channel (probes gate
    // starting; the stream is ground truth while it runs).
    !activeLiveOnline &&
    (preflightState.status !== "ready" || dockPreflightUnusable) &&
    activeFolderOpen &&
    !hasOpenFolderBeyondActive;
  // One view per folder-eligible source: its keyed live state, the picker
  // projection of it, and the query-filtered workspace groups. The filter applies
  // across all open folders. A failed focus re-arms that source's live client
  // (activateRow's catch) to drop the now-dead pane; until the fresh snapshot
  // lands the keyed rows still carry it — reconnecting preserves rows to avoid a
  // flicker on a healthy manual reconnect — so THAT source's list is gated to
  // "loading" during the recovery (scoped by activation.sourceKey) instead of
  // leaving the known-dead row clickable and instantly re-triggerable.
  const sourceViews = useMemo(
    () =>
      liveSources.map((source) => {
        const live = liveStateFor(liveStates, source.runnerKey);
        const recovering =
          activation.status === "failed" &&
          activation.sourceKey === source.runnerKey &&
          (live.connection.status === "connecting" ||
            live.connection.status === "reconnecting");
        const state: PickerState = recovering
          ? { status: "loading" }
          : pickerStateFromLive(live, source.runnerKey);
        const allRows = state.status === "ready" ? state.rows : [];
        const rows = groupRowsByProject(filterPickerRows(allRows, pickerFilter)).flatMap(
          (group) => group.rows,
        );
        // Per-source live-pane marker, derived like the owner-level focusedPaneId
        // below (see that comment for the is_focused/is_active fallback rationale).
        const focusedPaneId =
          allRows.find((row) => row.is_focused)?.pane_id ??
          (allRows.some((row) => row.is_focused !== undefined)
            ? null
            : (allRows.find((row) => row.is_active)?.pane_id ?? null));
        return {
          ...source,
          live,
          state,
          allRows,
          rows,
          groups: groupRowsByProject(rows),
          focusedPaneId,
        };
      }),
    [liveSources, liveStates, pickerFilter, activation],
  );
  const ownerView = useMemo(
    () => sourceViews.find((view) => view.isOwner) ?? null,
    [sourceViews],
  );
  // The keybind owner's rows back everything single-source: keyboard nav and
  // selection, Ctrl+<key> routing, and the horizontal bar's presentation.
  const allPickerRows = ownerView?.allRows ?? EMPTY_PICKER_ROWS;
  const pickerRows = ownerView?.rows ?? EMPTY_PICKER_ROWS;
  const pickerStatus: PickerState["status"] = ownerView?.state.status ?? "ready";
  // Server-level count echoed on every row; >1 means focus-following is
  // best-effort, so we warn that the live-pane highlight may not be reliable.
  const attachedClientCount = allPickerRows[0]?.attached_client_count ?? 0;
  // Spin the reconnect affordance while any open source's live client is
  // (re)connecting.
  const isReconnecting = sourceViews.some(
    (view) =>
      view.isOpen &&
      (view.live.connection.status === "connecting" ||
        view.live.connection.status === "reconnecting"),
  );
  const selectedIndex = selectedPaneId
    ? Math.max(0, pickerRows.findIndex((row) => row.pane_id === selectedPaneId))
    : 0;
  const selectedRow = pickerRows[selectedIndex] ?? null;
  // Derived from the owner's unfiltered rows: the search filter must not change the
  // focus signal, or hiding the focused row would null it and spuriously reset
  // follow-state, yanking a manual selection when the filter is cleared.
  //
  // Prefer the collapsed `is_focused` signal. If no row carries it — an older or
  // remote `agentscan` (schema < 5) that doesn't emit the field — fall back to
  // the first `is_active` pane so the picker still defaults to/highlights a live
  // pane instead of going dark.
  const focusedPaneId = ownerView?.focusedPaneId ?? null;

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

  useEffect(() => {
    // Drafts follow the settings form's target on a switch (id) or an in-place edit
    // of its committed values (runnerKey). Keyed on those VALUES, not the
    // activeProfile object: every service commit re-reads storage (all-new object
    // identities), so an identity key would also fire on commits that don't retarget
    // the form — drag-reorder, open-toggle — and clobber unsaved edits. The dock
    // renders no form, so this is inert there. The search filter is cross-folder UI
    // now and deliberately survives profile changes.
    setSettingsDraft(activeProfile.runner);
    setSshHostDraft(activeProfile.kind === "ssh" ? activeProfile.host : "");
    setSshClientTtyDraft(activeProfile.kind === "ssh" ? activeProfile.clientTty : "");
    // activeProfile is fully determined by (id, runnerKey) where this reads it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeProfile.id, runnerKey]);

  // The pane selection is scoped to the keybind OWNER (keyboard nav runs over the
  // owner's rows, and tmux pane ids like %1 collide across hosts), so it clears
  // whenever the owner's underlying TARGET changes — an ownership handoff
  // (open/close/reorder) or an in-place runner/host edit (both move
  // ownerRunnerKey). Seeded to the initial owner so mount doesn't clobber the
  // selection the selection-keeper effect just established.
  const lastSelectionResetRunnerKeyRef = useRef(ownerRunnerKey);
  useEffect(() => {
    if (lastSelectionResetRunnerKeyRef.current !== ownerRunnerKey) {
      lastSelectionResetRunnerKeyRef.current = ownerRunnerKey;
      setSelectedPaneId(null);
      // Re-arm focus-follow so the new owner snaps to its own focused pane.
      prevFocusedPaneIdRef.current = null;
    }
  }, [ownerRunnerKey]);

  // Drop an activation pulse/error whose source is no longer an open folder
  // (closed, deleted, or retargeted by a settings edit) — there is no list left
  // for it to describe. Source-less failures (null) are global and stay.
  useEffect(() => {
    const sourceKey = activation.status === "idle" ? null : activation.sourceKey;
    if (sourceKey !== null && !openRunnerKeys.has(sourceKey)) {
      if (activation.status === "running") {
        // A still-running activation's invoke may be wedged until the Rust-side
        // focus timeout; "running" means the guard is held by exactly this
        // activation, so free it with the visible state — otherwise every
        // source's clicks/keys silently no-op behind an invisible in-flight call.
        activationInFlightRef.current = null;
      }
      setActivation({ status: "idle" });
    }
  }, [activation, openRunnerKeys]);

  // A failed activation is one-shot action feedback, not ongoing state — left
  // alone it outlives its moment and reads like a standing condition, so it
  // expires after a beat. Source-less failures (null sourceKey: the
  // summon-hotkey registration error) DO describe a persistent condition and
  // stay until resolved.
  useEffect(() => {
    if (activation.status !== "failed" || activation.sourceKey === null) {
      return;
    }
    const failed = activation;
    const timer = window.setTimeout(() => {
      // Identity guard: clear only the exact failure this timer was armed for
      // (the dep-change cleanup already covers replacement; this covers races).
      setActivation((current) => (current === failed ? { status: "idle" } : current));
    }, ACTIVATION_FAILURE_TTL_MS);
    return () => window.clearTimeout(timer);
  }, [activation]);

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

  // Focus one row against its OWN source's runner settings (rows are tagged with
  // their source by the folder that renders them; keyboard paths pass the keybind
  // owner). One activation runs at a time across all sources.
  async function activateRow(row: PickerRow, profile: DesktopProfileConfig) {
    if (activationInFlightRef.current !== null) {
      return;
    }
    const token = Symbol("activation");
    activationInFlightRef.current = token;

    const requestRunnerKey = runnerKeyForProfile(profile);
    setActivation({ status: "running", paneId: row.pane_id, sourceKey: requestRunnerKey });

    try {
      await runCommand(
        focusCommandLabel(profile, row.pane_id),
        () =>
          invoke("focus_picker_row", {
            paneId: row.pane_id,
            settings: runnerSettingsForProfile(profile),
          }),
        appendDebugEntry,
      );
      // A superseded activation (guard freed early on source close, possibly
      // re-acquired by a newer row) must not touch the shared activation state.
      if (activationInFlightRef.current !== token) {
        return;
      }
      // Persistent-window model: focusing a pane must not hide the desktop.
      // Reset activation to idle and leave the window visible.
      setActivation({ status: "idle" });
    } catch (error) {
      if (activationInFlightRef.current !== token) {
        return;
      }
      if (!openRunnerKeysRef.current.has(requestRunnerKey)) {
        // The source was closed/edited mid-flight; there is no list left to recover.
        setActivation({ status: "idle" });
        return;
      }
      setActivation({
        status: "failed",
        message: errorMessage(error),
        sourceKey: requestRunnerKey,
      });
      // A failed focus is strong evidence the row is stale (the pane is gone). The
      // daemon is event-driven with periodic reconcile OFF by default, so a missed
      // tmux close notification won't self-correct — agentscan's own design names the
      // connect/reconnect bootstrap as the ground-truth recovery (config.rs). Re-arm
      // the live client: re-subscribing makes the daemon publish a fresh initial
      // snapshot, which the worker re-derives via load_picker_rows, dropping the dead
      // row. This is the push-model equivalent of the old one-shot refetch.
      reconnectLive(requestRunnerKey);
    } finally {
      // Release only our own token: the activation-drop effect frees a wedged
      // guard early when this source closes mid-flight, and a newer activation
      // may already hold it by the time this stale invoke settles.
      if (activationInFlightRef.current === token) {
        activationInFlightRef.current = null;
      }
    }
  }

  async function activateSelectedRow() {
    if (selectedRow && ownerProfile) {
      await activateRow(selectedRow, ownerProfile);
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
    // The boot/recovery screen replaces the folder list entirely, but the owner's
    // keyed live state can still hold rows behind it — gate every picker key while
    // it shows so Ctrl+<key>/Enter can't activate rows the user cannot see.
    if (dockBootScreenVisible) {
      return;
    }
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
    // Resolves ONLY against the keybind owner's rows (pickerRows); other folders
    // render their <kbd> labels dimmed, as information.
    const ctrlActivate = event.ctrlKey && !event.metaKey && !event.altKey && !event.shiftKey;
    if (ctrlActivate && ownerProfile && (IS_MAC || !isInteractiveShortcutTarget(event.target))) {
      const target = pickerRowForKeyboardKey(pickerRows, event.key);
      if (target) {
        event.preventDefault();
        setSelectedPaneId(target.pane_id);
        void activateRow(target, ownerProfile);
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
      settingsDraft,
      sshHostDraft,
      sshClientTtyDraft,
      profileState.profiles,
    );
    if (validation.errors.length > 0) {
      appendDebugEntry({
        kind: "settings",
        label: `${labelFor(activeProfile)} settings rejected`,
        detail: validation.errors.join(" · "),
      });
      return;
    }

    // Normalize the draft (and reflect it in the form), then hand the edit to the
    // service, which merges it onto the latest persisted state, persists, and
    // broadcasts. Validation + the debug log stay here; persistence is the service's.
    const normalized = normalizeRunnerSettings(settingsDraft);
    setSettingsDraft(normalized);
    void applyRunnerSettingsSet({
      runner: normalized,
      sshHost: sshHostDraft,
      sshClientTty: sshClientTtyDraft,
    })
      .then((outcome) => {
        if (outcome === "duplicate-host") {
          // Commit-time refusal: another window claimed this host after the form
          // validated. The service reloaded the ref, so the inline validation now
          // shows the duplicate; the log must not claim the edit was applied.
          appendDebugEntry({
            kind: "settings",
            label: `${labelFor(activeProfile)} settings rejected`,
            detail: "A source for this connection already exists.",
          });
          return;
        }
        appendDebugEntry({
          kind: "settings",
          label: `${labelFor(activeProfile)} settings applied`,
          detail: `${runnerSummary(normalized)} · ${normalized.env.length} env ${normalized.env.length === 1 ? "name" : "names"}`,
        });
      })
      .catch((error: unknown) => {
        appendDebugEntry({
          kind: "settings",
          label: `${labelFor(activeProfile)} settings apply failed`,
          detail: errorMessage(error),
        });
      });
  }

  function resetProfileSettings() {
    setSettingsDraft(activeProfile.runner);
    setSshHostDraft(activeProfile.kind === "ssh" ? activeProfile.host : "");
    setSshClientTtyDraft(activeProfile.kind === "ssh" ? activeProfile.clientTty : "");
  }

  // One-click fix for the dock recovery screen: when a remote preflight reports
  // where the user's shell finds agentscan (preflight.suggestedBinaryPath), set
  // this profile's binary to that path and persist it. The Profiles service edits
  // the active profile and broadcasts, so the dock re-probes with the new path —
  // no need to open settings. Persists straight to the active profile (not the
  // settings form drafts, which the dock never populates).
  function applySuggestedBinaryPath(path: string) {
    void applyRunnerSettingsSet({
      runner: normalizeRunnerSettings({ ...activeProfile.runner, binaryPath: path }),
      sshHost: activeProfile.kind === "ssh" ? activeProfile.host : "",
      sshClientTty: activeProfile.kind === "ssh" ? activeProfile.clientTty : "",
    })
      .then((outcome) => {
        // Same commit-time refusal as the settings form: another window claimed
        // this host between probe and click, so nothing was persisted.
        appendDebugEntry({
          kind: "settings",
          label: `${labelFor(activeProfile)} binary ${
            outcome === "duplicate-host" ? "NOT set (duplicate connection)" : "set from probe"
          }`,
          detail: path,
        });
      })
      .catch((error: unknown) => {
        appendDebugEntry({
          kind: "settings",
          label: `${labelFor(activeProfile)} binary apply failed`,
          detail: errorMessage(error),
        });
      });
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
    void selectProfileSet(id);
  }

  function addSshProfile() {
    addSshProfileSet();
  }

  function deleteActiveProfile() {
    // Guard here too so the debug entry (and its reference to the about-to-be-deleted
    // profile's label) only fires for a real deletion; the service also no-ops on local.
    if (activeProfile.kind === "local") {
      return;
    }
    deleteActiveProfileSet();
    appendDebugEntry({
      kind: "settings",
      label: `${labelFor(activeProfile)} profile deleted`,
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

  // Boot/error screen: still probing, the probe itself failed (IPC error), or the
  // CLI is unavailable for the current runner — and no other open folder could
  // render (see dockBootScreenVisible). It surfaces the real preflight error and
  // the Open settings recovery path instead of a perpetual live banner.
  if (dockBootScreenVisible) {
    const probing = preflightState.status === "loading";
    const detail =
      preflightState.status === "failed"
        ? preflightState.message
        : preflightState.status === "ready"
          ? (preflightState.preflight.error ?? `${profileKindLabel(activeProfile)} CLI unavailable`)
          : "Waiting for the daemon…";
    // A remote not-found preflight may carry the path the user's shell resolves,
    // letting us offer a one-click fix instead of only routing to settings.
    const suggestedBinaryPath =
      preflightState.status === "ready" && !preflightState.preflight.ok
        ? preflightState.preflight.suggestedBinaryPath
        : null;
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
              user with no way to fix the binary path or host. When the remote probe
              resolved an absolute path, also offer to apply it in one click. */}
          <div className="boot-actions">
            {suggestedBinaryPath ? (
              <button type="button" onClick={() => applySuggestedBinaryPath(suggestedBinaryPath)}>
                Use this path
              </button>
            ) : null}
            <button type="button" onClick={openSettings}>
              Open settings
            </button>
          </div>
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
    const activeIsOpen = profileState.openProfileIds.includes(activeProfile.id);
    const detailActions = (
      <div className="detail-actions">
        {/* Open/close must be reachable from Settings too: the horizontal bar has
            no room for the dock's folder menu (a 56px window clips any popup), so
            without this a pinned-horizontal user who closed every folder could
            never arm a subscription again without switching layouts. Only
            folder-eligible sources can open (a draft has nothing to subscribe). */}
        {folderProfiles(profileState).some((profile) => profile.id === activeProfile.id) ? (
          <button
            className="ghost-button"
            type="button"
            aria-pressed={activeIsOpen}
            onClick={() => toggleProfileOpenSet(activeProfile.id)}
          >
            {activeIsOpen ? "Close in dock" : "Open in dock"}
          </button>
        ) : null}
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
                    className={`source-card${isActive ? " active" : ""}${
                      draggedSourceId === profile.id ? " dragging" : ""
                    }`}
                    key={profile.id}
                    type="button"
                    draggable
                    onClick={() => selectProfile(profile.id)}
                    onDragStart={(event) => {
                      event.dataTransfer.effectAllowed = "move";
                      setDraggedSourceId(profile.id);
                    }}
                    onDragEnd={() => setDraggedSourceId(null)}
                    onDragOver={(event) => {
                      // preventDefault marks this card as a valid drop target.
                      if (draggedSourceId && draggedSourceId !== profile.id) {
                        event.preventDefault();
                        event.dataTransfer.dropEffect = "move";
                      }
                    }}
                    onDrop={(event) => {
                      event.preventDefault();
                      if (draggedSourceId && draggedSourceId !== profile.id) {
                        reorderProfileSet({ id: draggedSourceId, targetId: profile.id });
                      }
                      setDraggedSourceId(null);
                    }}
                  >
                    <span
                      className="source-card-mark"
                      data-kind={profile.kind}
                      aria-hidden="true"
                    >
                      <SourceKindIcon kind={profile.kind} />
                    </span>
                    <span className="source-card-text">
                      <span className="source-card-name">
                        {labelFor(profile)}
                      </span>
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
                    <h2>{labelFor(activeProfile)}</h2>
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

  // The footer trigger presents the dock's primary source: the keybind owner, falling
  // back to the settings-selected active profile when every folder is closed. When that
  // is the active source (the common case — and always true right after the open-state
  // migration) the dot keeps today's preflight tone; a non-active owner is never
  // probed, so its tone comes from its keyed live connection instead.
  //
  // That single-source presentation only fits when one source is all there is: with
  // several, every folder header already carries its own label and dot, so the
  // vertical trigger stops impersonating one host and becomes a generic entry point
  // to the source order menu. The horizontal bar still displays only the owner, so
  // it keeps the owner label regardless.
  const triggerProfile = ownerProfile ?? activeProfile;
  const triggerShowsSource =
    effectiveOrientation === "horizontal" || liveSources.length <= 1;
  const triggerIsActive = triggerProfile.id === activeProfile.id;
  const triggerTone = triggerIsActive
    ? sourceStatusTone
    : ownerView
      ? connectionTone(ownerView.live.connection)
      : "unknown";
  const triggerTitle = triggerIsActive
    ? statusText
    : (ownerView?.live.connection.message ?? statusText);

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
          onClick={() => {
            // Re-arm every open source; closed folders have no subscription.
            for (const source of liveSources) {
              if (source.isOpen) {
                reconnectLive(source.runnerKey);
              }
            }
          }}
        >
          <span className={isReconnecting ? "spin" : undefined}>{"↻"}</span>
        </button>
      </header>

      {/* The horizontal bar keeps the single-source presentation (the keybind owner);
          its connection problems surface here. In the vertical strip each folder
          carries its own strip instead. */}
      {effectiveOrientation === "horizontal" &&
      ownerView &&
      ownerView.live.connection.status !== "online" ? (
        <LiveStrip
          status={ownerView.live.connection}
          onStart={() => startLive(ownerView.runnerKey)}
          onReconnect={() => reconnectLive(ownerView.runnerKey)}
        />
      ) : null}

      {/* A failed activation is a per-source event: in the vertical strip it renders
          inside the failing source's folder so healthy folders don't look broken.
          This global surface covers the horizontal bar (which shows one source) and
          source-less failures (sourceKey: null). */}
      {activation.status === "failed" &&
      (effectiveOrientation === "horizontal" || activation.sourceKey === null) ? (
        <div className="inline-error" role="alert">
          {activation.message}
        </div>
      ) : null}

      <div className="picker-scroll" aria-label="Agents" tabIndex={-1}>
        {effectiveOrientation === "horizontal" ? (
          <GroupedPicker
            activation={activation}
            filterQuery={pickerFilter}
            focusedPaneId={focusedPaneId}
            groups={ownerView?.groups ?? EMPTY_PICKER_GROUPS}
            keybindsOwned
            logoTheme={resolvedTheme}
            selectedPaneId={selectedPaneId}
            sourceKey={ownerRunnerKey ?? ""}
            state={ownerView?.state ?? { status: "ready", rows: EMPTY_PICKER_ROWS }}
            totalRows={allPickerRows.length}
            onActivate={(row) => {
              if (ownerProfile) {
                void activateRow(row, ownerProfile);
              }
            }}
            onClearFilter={() => setPickerFilter("")}
            onSelect={(row) => setSelectedPaneId(row.pane_id)}
          />
        ) : (
          // The vertical strip is a list of host folders: one collapsible section per
          // enabled source, in the user's order. Open = live subscription + that
          // source's workspace-grouped rows; closed = header only, no subscription.
          <div className="source-folders">
            {activePreflightError !== null &&
            !liveSources.some((source) => source.runnerKey === runnerKey) ? (
              // The active source can be folder-INeligible (e.g. a just-added remote
              // with no host yet): it renders no folder, and with another folder open
              // the boot screen is suppressed, so without this strip its failure has
              // no surface at all. Same recovery shape as the in-folder strip.
              <div className="live-strip error" aria-live="polite">
                <span className="status-dot" data-tone="error" />
                <span className="live-label">{labelFor(activeProfile)}</span>
                <span className="live-message">{activePreflightError}</span>
                <button className="live-action" type="button" onClick={openSettings}>
                  Open settings
                </button>
              </div>
            ) : null}
            {sourceViews.map((view) => {
              // The active source's resolved-failing preflight surfaces in its own
              // folder: its live target is gated off on a failed probe, so the keyed
              // connection (a perpetual "Waiting for a source") would lie about what
              // broke. Non-active sources are never probed; theirs stays null.
              const preflightError =
                view.runnerKey === runnerKey ? activePreflightError : null;
              return (
                <section className="source-folder" key={view.profile.id}>
                  <button
                    className="folder-header"
                    type="button"
                    aria-expanded={view.isOpen}
                    onClick={() => toggleProfileOpenSet(view.profile.id)}
                    title={
                      preflightError ??
                      (view.isOpen
                        ? view.live.connection.message
                        : "Closed — no live subscription")
                    }
                  >
                    <span
                      className={`status-dot${
                        preflightError === null &&
                        view.isOpen &&
                        (view.live.connection.status === "connecting" ||
                          view.live.connection.status === "reconnecting")
                          ? " pulsing"
                          : ""
                      }`}
                      data-tone={
                        preflightError !== null
                          ? "error"
                          : view.isOpen
                            ? connectionTone(view.live.connection)
                            : "unknown"
                      }
                      aria-hidden="true"
                    />
                    <span className="folder-mark" aria-hidden="true">
                      <SourceKindIcon kind={view.profile.kind} />
                    </span>
                    <span className="folder-label">
                      {labelFor(view.profile)}
                    </span>
                    {view.isOwner ? (
                      <kbd className="folder-kbd" title="Row hotkeys target this source">
                        {HOTKEY_MODIFIER_LABEL.trim()}
                      </kbd>
                    ) : null}
                    <span
                      className={`folder-caret${view.isOpen ? " open" : ""}`}
                      aria-hidden="true"
                    >
                      {"›"}
                    </span>
                  </button>
                  {view.isOpen ? (
                    <div className="folder-body">
                      {preflightError !== null ? (
                        // The gated-off target's keyed state (connecting, no rows) would
                        // render a perpetual loading skeleton under this, so the strip
                        // replaces the picker body too. Mirrors the boot screen's
                        // recovery path; LiveStrip's Start/Reconnect can't fix a
                        // preflight failure, so it doesn't render here.
                        <div className="live-strip error" aria-live="polite">
                          <span className="status-dot" data-tone="error" />
                          <span className="live-label">Unavailable</span>
                          <span className="live-message">{preflightError}</span>
                          <button className="live-action" type="button" onClick={openSettings}>
                            Open settings
                          </button>
                        </div>
                      ) : (
                        <>
                          {/* This source's own failed activation; source-less
                              failures use the global surface above the folders. */}
                          {activation.status === "failed" &&
                          activation.sourceKey === view.runnerKey ? (
                            <div className="inline-error" role="alert">
                              {activation.message}
                            </div>
                          ) : null}
                          {view.live.connection.status !== "online" ? (
                            <LiveStrip
                              status={view.live.connection}
                              onStart={() => startLive(view.runnerKey)}
                              onReconnect={() => reconnectLive(view.runnerKey)}
                            />
                          ) : null}
                          <GroupedPicker
                            activation={activation}
                            filterQuery={pickerFilter}
                            focusedPaneId={view.focusedPaneId}
                            groups={view.groups}
                            keybindsOwned={view.isOwner}
                            logoTheme={resolvedTheme}
                            selectedPaneId={view.isOwner ? selectedPaneId : null}
                            sourceKey={view.runnerKey}
                            state={view.state}
                            totalRows={view.allRows.length}
                            onActivate={(row) => void activateRow(row, view.profile)}
                            onClearFilter={() => setPickerFilter("")}
                            onSelect={(row) => {
                              // Selection (the keyboard cursor) is owner-scoped; clicks on
                              // other folders activate without moving it.
                              if (view.isOwner) {
                                setSelectedPaneId(row.pane_id);
                              }
                            }}
                          />
                        </>
                      )}
                    </div>
                  ) : null}
                </section>
              );
            })}
          </div>
        )}
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
              // Pre-select the source this trigger advertises (the owner can differ
              // from the settings-selected active profile): landing in Settings on a
              // different source than the label promised would manage the wrong one.
              // This is a deep-link INTO the settings selection, not a dock-side
              // quick-switch — the order menu below still never selects. The
              // retarget is deliberate and cheap: the probe moves to the source the
              // bar is DISPLAYING (open, so an online channel stays armed), no
              // subscription churns, and a user after the previous selection is one
              // card-click away. Await the commit so the window can't load the old
              // selection; open regardless of the outcome (Settings is the goal).
              // A DIRTY settings window deliberately wins over this deep-link: it
              // skips inbound syncs to protect unsaved edits, and its Apply/Delete
              // target the window's own ref (which mirrors the form), so no action
              // can hit a different source than the one its form displays. The
              // label/form mismatch resolves on the form's apply or reset.
              if (effectiveOrientation === "horizontal") {
                void selectProfileSet(triggerProfile.id)
                  .catch(() => {})
                  .then(() => openSettings());
              } else {
                setIsSourceMenuOpen((open) => !open);
              }
            }}
            title={
              triggerShowsSource
                ? triggerTitle
                : "Drag to reorder sources — the top open source owns row hotkeys"
            }
          >
            {triggerShowsSource ? (
              <span
                className="status-dot"
                data-tone={triggerTone}
                aria-hidden="true"
              />
            ) : null}
            <span className="source-label">
              {triggerShowsSource ? labelFor(triggerProfile) : "Manage sources"}
            </span>
            <span
              className={`source-caret${isSourceMenuOpen ? " open" : ""}`}
              aria-hidden="true"
            >
              {"›"}
            </span>
          </button>
          {isSourceMenuOpen ? (
            // Pure ordering surface: drag rows to reorder sources. The topmost
            // OPEN folder owns the row hotkeys, so this is where the dock decides
            // which source answers them. Nothing else is duplicated here —
            // open/close lives on the folder headers, and enable/disable/add/
            // remove live in Settings.
            //
            // Deliberately NOT a quick-switch: the dock never changes the active
            // source. The old single-select footer existed because the dock could
            // show one source at a time; folders replace that gesture with the open
            // set. "Active" now only means the settings-edit selection + the single
            // preflight target, and it changes in Settings.
            <div className="source-menu" role="menu">
              {liveSources.map(({ profile, isOwner }) => (
                <div
                  className={`source-option draggable${
                    draggedMenuSourceId === profile.id ? " dragging" : ""
                  }`}
                  key={profile.id}
                  role="menuitem"
                  draggable
                  onDragStart={(event) => {
                    event.dataTransfer.effectAllowed = "move";
                    setDraggedMenuSourceId(profile.id);
                  }}
                  onDragEnd={() => setDraggedMenuSourceId(null)}
                  onDragOver={(event) => {
                    // preventDefault marks this row as a valid drop target.
                    if (draggedMenuSourceId && draggedMenuSourceId !== profile.id) {
                      event.preventDefault();
                      event.dataTransfer.dropEffect = "move";
                    }
                  }}
                  onDrop={(event) => {
                    event.preventDefault();
                    if (draggedMenuSourceId && draggedMenuSourceId !== profile.id) {
                      reorderProfileSet({
                        id: draggedMenuSourceId,
                        targetId: profile.id,
                      });
                    }
                    setDraggedMenuSourceId(null);
                  }}
                >
                  <span className="source-option-mark" aria-hidden="true">
                    <SourceKindIcon kind={profile.kind} />
                  </span>
                  <span className="source-option-text">
                    <span className="source-option-name">
                      {labelFor(profile)}
                    </span>
                  </span>
                  {isOwner ? (
                    <kbd
                      className="folder-kbd"
                      title="Row hotkeys target this source"
                    >
                      {HOTKEY_MODIFIER_LABEL.trim()}
                    </kbd>
                  ) : null}
                  <svg
                    className="source-grip"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="2"
                    strokeLinecap="round"
                    aria-hidden="true"
                  >
                    <circle cx="9" cy="5" r="1" />
                    <circle cx="9" cy="12" r="1" />
                    <circle cx="9" cy="19" r="1" />
                    <circle cx="15" cy="5" r="1" />
                    <circle cx="15" cy="12" r="1" />
                    <circle cx="15" cy="19" r="1" />
                  </svg>
                </div>
              ))}
              <div className="source-menu-divider" role="separator" />
              <button
                className="source-option manage"
                role="menuitem"
                type="button"
                onClick={() => {
                  // Deliberately no selectProfileSet deep-link here, unlike the
                  // horizontal trigger above: that button names exactly one
                  // source, so it must land Settings on it. This item is plural
                  // and source-agnostic — it preserves the settings window's own
                  // edit selection rather than warping it (and the preflight
                  // probe) to whichever owner the footer happens to advertise.
                  // Settings shows its selection unambiguously (highlighted rail
                  // card + the form's fields), so Apply/Delete can't silently
                  // target a source other than the one displayed.
                  setIsSourceMenuOpen(false);
                  openSettings();
                }}
              >
                <span className="source-check" aria-hidden="true">
                  {"⚙"}
                </span>
                <span className="source-option-label">Add or edit sources…</span>
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

// Project one source's keyed live state onto the PickerState its folder renders.
// The service is the single owner of rows + connection status; this just picks the
// view: keep showing the last rows while (re)connecting so the list doesn't flash
// a skeleton on a brief blip, show the failure only when a fatal state has
// actually cleared the rows, and otherwise a loading skeleton.
//
// Rows are trusted only when their producing runner (rowsRunnerKey) matches the
// key being rendered. Within a keyed entry that always holds (frames are routed by
// sourceKey), so this is a defensive guard kept from the single-target days.
function pickerStateFromLive(live: LiveState, runnerKey: string): PickerState {
  const { connection } = live;
  const rows = live.rowsRunnerKey === runnerKey ? live.rows : [];
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

// Tone for a source's folder/footer status dot, from its keyed live connection.
function connectionTone(connection: ConnectionStatus): string {
  switch (connection.status) {
    case "online":
      return "idle";
    case "fatal":
      return "error";
    case "noDaemon":
      return "busy";
    case "connecting":
    case "reconnecting":
      return "unknown";
  }
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
  keybindsOwned,
  logoTheme,
  selectedPaneId,
  sourceKey,
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
  // Whether this source owns the row keybinds (Ctrl+<key>). Non-owners render
  // their <kbd> labels dimmed, as information only.
  keybindsOwned: boolean;
  logoTheme: LogoTheme;
  selectedPaneId: string | null;
  // This source's runnerKey; scopes the activation pulse (pane ids collide across hosts).
  sourceKey: string;
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
                activation.status === "running" &&
                activation.sourceKey === sourceKey &&
                activation.paneId === row.pane_id;
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
                  <kbd className={keybindsOwned ? undefined : "dimmed"}>
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
