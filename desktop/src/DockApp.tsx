import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { Result, useAtomSet, useAtomValue } from "@effect-atom/atom-react";
import type { LogoTheme } from "./providerLogos";
import { GroupedPicker } from "./components/GroupedPicker";
import { LiveStrip } from "./components/LiveStrip";
import { SourceKindIcon } from "./components/SourceKindIcon";
import { WindowControls } from "./components/WindowControls";
import { HOTKEY_MODIFIER_LABEL, IS_MAC } from "./platform";
import {
  activateAtom,
  activationAtom,
  appearanceAtom,
  appendDebugEntryAtom,
  applyRunnerSettingsAtom,
  configureAtom,
  configureHostnameEnrichmentAtom,
  configurePreflightAtom,
  configureSummonHotkeyAtom,
  liveStatesAtom,
  preflightStateAtom,
  profilesAtom,
  pruneActivationAtom,
  reconnectAtom,
  reloadProfilesAtom,
  reorderProfileAtom,
  selectProfileAtom,
  startAtom,
  summonHotkeyAtom,
  toggleProfileOpenAtom,
} from "./effect/atoms";
import type { LiveState, PickerRow } from "./effect/types";
import { liveStateFor, type LiveStates } from "./effect/LiveConnection";
import { pickerKeyIntent } from "./effect/keybinds";
import type { PreflightState } from "./effect/Preflight";
import { glassClearFor, loadAppearance } from "./effect/appearanceModel";
import {
  commandPrefix,
  committedProfileValidation,
  focusCommandLabel,
  getActiveProfile,
  liveSourcesFor,
  loadProfileState,
  normalizeRunnerSettings,
  profileKindLabel,
  runnerKeyForProfile,
  runnerSettingsForProfile,
  sourceLabel,
  type DesktopProfileConfig,
  type PreflightLabelSource,
} from "./effect/profileModel";
import {
  PREFS_SYNC_EVENT,
  type Orientation,
  type PrefsSync,
  type ThemePreference,
} from "./effect/prefs";
import {
  connectionTone,
  deriveSourceViews,
  reconcileSelection,
  type PickerActivation,
  type PickerGroup,
  type PickerState,
} from "./effect/pickerViewModel";
import {
  activePreflightError,
  dockBootScreenContent,
  dockBootScreenVisible,
  liveTargetsFor,
  preflightSourceTone,
  preflightStatusText,
} from "./effect/preflightViewModel";
import type { SummonHotkeyState } from "./effect/SummonHotkey";
import {
  BAR_WINDOW_HEIGHT,
  enqueueFramelessOperation,
  enqueueGlassOperation,
  enqueueWindowOperation,
  FRAMELESS_CORNER_RADIUS,
  placeBarWindow,
  placePickerWindow,
  raisePickerWindow,
  WINDOW_MAX_UNBOUNDED,
  WINDOW_MAX_WIDTH_VERTICAL,
  WINDOW_MIN_HEIGHT_HORIZONTAL,
  WINDOW_MIN_HEIGHT_VERTICAL,
  WINDOW_MIN_WIDTH,
} from "./windowOperations";
import { errorMessage, readLocalStorage } from "./shared";

// Appearance prefs (storage keys, alpha bounds, glassClearFor, the parsers) live in
// effect/appearanceModel and are owned by the Appearance Effect service; the DOM apply
// (this setter, the theme/glass/sizing effects) stays here.
const setGlassClear = (clear: number) => {
  document.documentElement.style.setProperty("--glass-clear", clear.toFixed(3));
};

// Live-picker subscription state (connection status + rows + epoch fencing + the
// reconnect/latch policy) is owned by the Effect LiveConnection service, as a
// per-source map keyed by runnerKey. This component drives it via
// configure/reconnect/start and observes liveStatesAtom, reading the active
// runner's entry through liveStateFor (which supplies the initial fallback).
const EMPTY_LIVE_STATES: LiveStates = new Map<string, LiveState>();

// First-paint fallback for summonHotkeyAtom before the runtime resolves it.
const SUMMON_HOTKEY_INACTIVE: SummonHotkeyState = { status: "inactive" };

// First-paint fallback for activationAtom before the runtime resolves it.
const IDLE_ACTIVATION: PickerActivation = { status: "idle" };

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

// The dock's resolved preflight is owned by the Preflight Effect service (observed via
// preflightStateAtom as PreflightState); the picker rows + live connection status live
// in the LiveConnection service (liveStatesAtom).
const INITIAL_PREFLIGHT: PreflightState = { status: "loading" };

// Stable empty fallbacks for the no-owner case so effect dep arrays don't churn.
const EMPTY_PICKER_ROWS: PickerRow[] = [];
const EMPTY_PICKER_GROUPS: PickerGroup[] = [];

// The dock window: boot/recovery screen, the folder strip / horizontal bar
// picker, and every native window apply (sizing, glass, frameless, the summon
// hotkey, live subscriptions, the preflight prober). The settings window is
// SettingsApp.tsx; the two never import each other (see shared.ts).
function DockApp() {
  // The dock's resolved CLI preflight is owned by the Preflight Effect service. The dock
  // observes preflightStateAtom and drives the probe via configurePreflight; the service
  // also mirrors each result to the settings window over the shared prefs channel.
  const preflightState = Result.getOrElse(
    useAtomValue(preflightStateAtom),
    () => INITIAL_PREFLIGHT,
  );
  const configurePreflight = useAtomSet(configurePreflightAtom);
  // Profile/settings persistence + cross-window adoption are owned by the Profiles
  // Effect service; this window observes its state via an atom and drives changes
  // through the action atoms below. The first synchronous render (before the runtime
  // resolves the atom) falls back to a direct storage read so the active profile /
  // runnerKey / drafts are correct on the very first paint, matching the service seed.
  const initialProfileState = useMemo(() => loadProfileState(readLocalStorage), []);
  const profileStateResult = useAtomValue(profilesAtom);
  const profileState = Result.getOrElse(profileStateResult, () => initialProfileState);
  // Promise mode so the horizontal footer's settings deep-link can await the
  // selection commit before opening the window.
  const selectProfileSet = useAtomSet(selectProfileAtom, { mode: "promise" });
  // Promise mode: the apply outcome ("applied" | "duplicate-host") drives the
  // debug-log entry, so a commit-time refusal is never reported as applied.
  const applyRunnerSettingsSet = useAtomSet(applyRunnerSettingsAtom, { mode: "promise" });
  const toggleProfileOpenSet = useAtomSet(toggleProfileOpenAtom);
  const reorderProfileSet = useAtomSet(reorderProfileAtom);
  const configureHostnameEnrichment = useAtomSet(configureHostnameEnrichmentAtom);
  const reloadProfiles = useAtomSet(reloadProfilesAtom);
  const activeProfile = useMemo(() => getActiveProfile(profileState), [profileState]);
  const runnerSettings = useMemo(() => runnerSettingsForProfile(activeProfile), [activeProfile]);
  // Identity of the exact runner configuration a resolved state describes. It
  // changes on a profile switch AND on any settings edit (binary/env/host/tty)
  // to the active profile, so resolved preflight/picker data is invalidated
  // whenever the underlying target changes, not just when the profile id does.
  const runnerKey = useMemo(() => runnerKeyForProfile(activeProfile), [activeProfile]);
  // The folder-eligible sources in order, each with its runner identity, open
  // state, keybind ownership, and committed-profile validity (the arm gate for
  // non-active sources, whose preflight is never probed). Derived + tested in
  // effect/profileModel.
  const liveSources = useMemo(() => liveSourcesFor(profileState), [profileState]);
  const ownerSource = useMemo(() => liveSources.find((s) => s.isOwner) ?? null, [liveSources]);
  const ownerProfile = ownerSource?.profile ?? null;
  const ownerRunnerKey = ownerSource?.runnerKey ?? null;
  // The dock only WRITES its per-window debug log (command lifecycles, native
  // apply failures); the settings window renders its own instance. The append
  // setter is registry-stable, unlike the old per-render closure, so logging
  // effects can list it in their dep arrays.
  const appendDebugEntry = useAtomSet(appendDebugEntryAtom);
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
  // The summon hotkey (registration + in-use retry loop) is owned by the
  // SummonHotkey service; the dock arms it below and renders its standing
  // failure state as the global banner.
  const summonHotkey = Result.getOrElse(
    useAtomValue(summonHotkeyAtom),
    () => SUMMON_HOTKEY_INACTIVE,
  );
  const configureSummonHotkey = useAtomSet(configureSummonHotkeyAtom);
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
  // Appearance Effect service; the settings window drives changes (which persist +
  // cross-window broadcast), the dock observes and applies. The DOM/Tauri apply
  // (data-theme, set_window_glass, window shaping, CSS vars) lives in the effects
  // below. The first synchronous render (before the runtime resolves the atom) falls
  // back to a direct storage read so layout/theme/glass are right on the first paint.
  const initialAppearance = useMemo(() => loadAppearance(readLocalStorage), []);
  const appearance = Result.getOrElse(useAtomValue(appearanceAtom), () => initialAppearance);
  const { themePref, orientationPref, glassEnabled, surfaceAlpha, framelessEnabled } =
    appearance;
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
  // The probed remote hostname as a label source, from this window's own resolved
  // preflight (the settings window reuses the mirror instead). sourceLabel only
  // honors it for the profile whose runnerKey matches, so a stale probe can never
  // label a source.
  const labelPreflight: PreflightLabelSource | null =
    preflightState.status === "ready" ? preflightState : null;
  // One label rule for every card/menu/header: sourceLabel sees the sibling
  // sources so a probed hostname that collides with another source's label is
  // dropped for the configured connection string.
  const labelFor = (profile: DesktopProfileConfig) =>
    sourceLabel(profile, localHostLabel, labelPreflight, profileState.profiles);
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
  // The runnerKeys of the OPEN folders; the activation-prune effect below
  // reconciles the Activation service against them so a pulse/error whose
  // source closed is dropped. The render-synced ref additionally backs the
  // isSourceOpen probe activateRow hands the service: it observes a close one
  // render before the prune effect can, so a focus failure settling in that
  // window is dropped instead of surfaced (and never re-arms the closed
  // source's live client).
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
  // The pane-activation lifecycle (the one-at-a-time in-flight guard, the
  // failure surface + TTL, and its interplay with the failed source's recovery)
  // is owned by the Activation Effect service; this window renders its state
  // and drives it via activateRow/the prune effect.
  const activation = Result.getOrElse(useAtomValue(activationAtom), () => IDLE_ACTIVATION);
  const activate = useAtomSet(activateAtom);
  const pruneActivation = useAtomSet(pruneActivationAtom);

  // The active profile's own validation (its committed values, not the form drafts).
  // Both the live-picker gate and the preflight target read it: a synchronously-invalid
  // profile is gated off the picker in the same render (no flash of the bad target) and
  // resolves to a synthetic failed preflight without an IPC probe.
  const activeProfileValidation = useMemo(
    () => committedProfileValidation(activeProfile, profileState.profiles),
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
  }, [runnerKey, configurePreflight]);

  // Hostname-label enrichment (persisting the driver's probed hostnames and
  // one-shot background probes for never-probed online remotes) is owned by the
  // HostnameEnrichment service; this effect only arms it with the debug-log
  // sink. All deps are registry-stable setters, so this fires once per dock
  // boot; StrictMode's double configure is absorbed by the service's mutex'd
  // supervisor slot (in-flight probes live in the service scope and survive
  // the swap).
  useEffect(() => {
    configureHostnameEnrichment({
      onLog: (label, detail) => appendDebugEntry({ kind: "command", label, detail }),
    });
  }, [configureHostnameEnrichment, appendDebugEntry]);

  const activeLiveOnline = liveStateFor(liveStates, runnerKey).connection.status === "online";
  // The configure inputs are derived + tested in effect/preflightViewModel
  // (which carries THE probes-gate-starting invariant). liveTargetsFor must be
  // called inside this memo: a bare per-render call would mint a new array
  // identity every frame and re-fire configureLive on every rows update.
  const liveTargets = useMemo(
    () =>
      liveTargetsFor(liveSources, runnerKey, activeLiveOnline, preflightState, activeProfileValid),
    [liveSources, runnerKey, activeLiveOnline, preflightState, activeProfileValid],
  );
  // Drive the LiveConnection service to every OPEN folder's source: open folder =
  // live subscription, closed folder = none. The service owns the subscriptions,
  // epoch fencing, reconnect/latch backoff, and recovery; this just tells it WHICH
  // runners to track and WHETHER each is ready. The active (settings-selected)
  // source gates on its resolved preflight, but only once that resolution
  // describes the CURRENT runnerKey: the probe lags a switch by one async cycle,
  // and gating off during that window would bounce (teardown + full reconnect —
  // over SSH a whole remote process respawn) the healthy, already-armed
  // subscription of a source the user merely re-selected in Settings. While
  // unresolved, "carry" tells the service to keep the enabled value it last saw
  // for the key; a key with no history (launch, or an in-place edit that moved
  // the runnerKey) stays gated off until its probe resolves. Other open sources
  // are never probed, so they arm on their committed-profile validity and
  // surface failures per folder through their keyed live state. configure diffs
  // on runnerKey + enabled, so re-running on an unrelated profileState change
  // leaves running keys alone.
  useEffect(() => {
    configureLive(liveTargets);
  }, [liveTargets, configureLive]);

  // Resolve the local machine's hostname once for the local source label. Each
  // window runs its own fetch (per-webview, as before the split); a failure just
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
    // The global summon hotkey belongs to the dock alone (the settings window
    // never configures it — a second registration would double-bind the
    // shortcut). Registration, the in-use retry loop, and the failure banner
    // state live in the SummonHotkey service — this effect only points it at
    // the summon action. The callback reads summonPlacementRef at press time,
    // so the one registration always places by the LIVE orientation.
    configureSummonHotkey({
      onPress: () => {
        void raisePickerWindow(summonPlacementRef.current);
      },
    });
    return () => configureSummonHotkey({ onPress: null });
  }, [configureSummonHotkey]);

  // Footer status line, dot tone, and the per-folder error strip for the active
  // source: all derived in effect/preflightViewModel around its single
  // matchedPreflight staleness rule (and tested there).
  const statusText = useMemo(
    () => preflightStatusText(preflightState, runnerKey, profileKindLabel(activeProfile)),
    [activeProfile, preflightState, runnerKey],
  );
  const sourceStatusTone = preflightSourceTone(preflightState, runnerKey);
  const preflightError = activePreflightError(
    preflightState,
    runnerKey,
    activeLiveOnline,
    profileKindLabel(activeProfile),
  );
  const hasOpenFolderBeyondActive = liveSources.some(
    (source) => source.isOpen && source.runnerKey !== runnerKey,
  );
  const activeFolderOpen = liveSources.some(
    (source) => source.isOpen && source.runnerKey === runnerKey,
  );
  // The full-screen boot/recovery takeover; the five interacting invariants that
  // scope it (and why) live with dockBootScreenVisible in preflightViewModel.
  const bootScreenVisible = dockBootScreenVisible(preflightState, runnerKey, {
    isDock: true,
    activeLiveOnline,
    activeFolderOpen,
    hasOpenFolderBeyondActive,
  });
  // One view per folder-eligible source: its keyed live state, the picker
  // projection of it, and the query-filtered workspace groups. The derivation —
  // including the recovering mask and the per-source focus marker — lives in
  // effect/pickerViewModel (deriveSourceViews) with its contracts under test;
  // this wrapper only memoizes it on the same inputs.
  const sourceViews = useMemo(
    () => deriveSourceViews(liveSources, liveStates, pickerFilter, activation),
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
  // follow-state, yanking a manual selection when the filter is cleared. The
  // is_focused/is_active (schema < 5) fallback lives in focusedPaneIdOf
  // (pickerViewModel).
  const focusedPaneId = ownerView?.focusedPaneId ?? null;

  // Selection keeper: the decision (focus-follow vs manual-pick survival vs
  // validity repair) lives in reconcileSelection (pickerViewModel, tested
  // there); this effect just feeds it the current state and applies the step.
  // An absent field means leave untouched; null means clear — so both applies
  // check !== undefined, never truthiness.
  useEffect(() => {
    const step = reconcileSelection({
      status: pickerStatus,
      allRowsCount: allPickerRows.length,
      rows: pickerRows,
      selectedPaneId,
      focusedPaneId,
      prevFocusedPaneId: prevFocusedPaneIdRef.current,
    });
    if (step.prevFocusedPaneId !== undefined) {
      prevFocusedPaneIdRef.current = step.prevFocusedPaneId;
    }
    if (step.selectedPaneId !== undefined) {
      setSelectedPaneId(step.selectedPaneId);
    }
  }, [allPickerRows.length, pickerRows, pickerStatus, selectedPaneId, focusedPaneId]);

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

  // Reconcile the Activation service against the open folders: it drops a
  // pulse/error whose source is no longer open (closed, deleted, or retargeted
  // by a settings edit) and frees a wedged in-flight guard. The failure TTL and
  // its hold-while-recovering interplay live in the service too.
  useEffect(() => {
    pruneActivation(Array.from(openRunnerKeys));
  }, [openRunnerKeys, pruneActivation]);

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
  }, [orientationPref, effectiveOrientation]);

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
  // Apply the cross-window `profiles` sync. Appearance (theme/orientation/glass) and
  // preflight syncs are consumed by their own Effect services over the same channel
  // (PrefsBridge installs its own listener), so only the `profiles` adoption remains
  // here; the handler never re-broadcasts, so A -> B -> A can't loop. The dock always
  // adopts so its live picker tracks the current profile config (only the settings
  // window dirty-gates its adoption, in SettingsApp); the reload is value-guarded, so
  // an unchanged snapshot is a no-op. The handler closes only over the registry-stable
  // reloadProfiles setter, so the empty dep array binds it once.
  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | null = null;
    void listen<PrefsSync>(PREFS_SYNC_EVENT, (event) => {
      if (event.payload.kind === "profiles") {
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

  // Closing the dock means quitting. The settings window is kept warm (hidden, never
  // self-destroys), so it must be torn down before the dock goes — otherwise that
  // hidden window keeps the process alive with no visible UI. preventDefault() holds
  // the dock open until the (awaited) settings teardown finishes, then we force the
  // dock closed; without it the dock webview can be destroyed mid-IPC and strand the
  // hidden window. destroy() forces teardown without firing either hide-handler, and
  // the dock is destroyed even if the settings lookup throws, so the app always exits.
  useEffect(() => {
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
  }, []);

  // Toggle the macOS glass backdrop. Order matters so we never flash the bare
  // desktop through the transparent window: when enabling, raise the blur layer
  // first, then mark the surface translucent; when disabling, go opaque first,
  // then drop the blur. macOS-only — the toggle isn't offered anywhere else.
  // Persistence + the cross-window mirror are owned by the Appearance service; this
  // effect only applies the native vibrancy, which lives on the dock (the settings
  // window is a solid, normally-chromed window and never frosts itself).
  useEffect(() => {
    if (!IS_MAC) {
      return;
    }

    let cancelled = false;
    enqueueGlassOperation(async () => {
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
  }, [glassEnabled, framelessApplied]);

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
    let cancelled = false;
    enqueueFramelessOperation(async () => {
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
  }, [framelessEnabled]);

  // Focus one row against its OWN source's runner settings (rows are tagged with
  // their source by the folder that renders them; keyboard paths pass the keybind
  // owner). The Activation service runs one activation at a time across all
  // sources, owns the failure surface/TTL, and re-arms the failed source's live
  // client; this just shapes the request and routes the command lifecycle into
  // the debug log.
  function activateRow(row: PickerRow, profile: DesktopProfileConfig) {
    const label = focusCommandLabel(profile, row.pane_id);
    const sourceKey = runnerKeyForProfile(profile);
    activate({
      paneId: row.pane_id,
      sourceKey,
      settings: runnerSettingsForProfile(profile),
      isSourceOpen: () => openRunnerKeysRef.current.has(sourceKey),
      onLog: (detail) => appendDebugEntry({ kind: "command", label, detail }),
    });
  }

  function activateSelectedRow() {
    if (selectedRow && ownerProfile) {
      activateRow(selectedRow, ownerProfile);
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
    // The event→intent interpretation (boot gate, Ctrl+<key> routing with its
    // platform rules and fall-through, movement/Home/End/Enter/Escape) lives in
    // pickerKeyIntent (effect/keybinds, tested there); this applies the intent.
    const intent = pickerKeyIntent(event, {
      bootScreenVisible,
      hasOwner: ownerProfile !== null,
      isInteractiveTarget: isInteractiveShortcutTarget(event.target),
      isMac: IS_MAC,
      rows: pickerRows,
      filterActive: Boolean(pickerFilter),
    });
    if (intent === null) {
      return;
    }
    // Every intent claims the key except an escape with nothing to clear (its
    // collapse-search side effect below still runs).
    if (intent.kind !== "escape" || intent.clearFilter) {
      event.preventDefault();
    }
    switch (intent.kind) {
      case "activate":
        if (ownerProfile) {
          setSelectedPaneId(intent.row.pane_id);
          activateRow(intent.row, ownerProfile);
        }
        break;
      case "move":
        moveSelection(intent.delta);
        break;
      case "select":
        setSelectedPaneId(intent.paneId);
        break;
      case "activateSelection":
        activateSelectedRow();
        break;
      case "escape":
        if (intent.clearFilter) {
          setPickerFilter("");
        }
        // In the horizontal bar this collapses search back to its icon (inert
        // in the always-expanded vertical strip).
        setIsSearchExpanded(false);
        break;
    }
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

  // Hold the latest handler in a ref so the global listener binds once instead
  // of churning on every render (live row updates re-render frequently).
  const pickerKeyDownRef = useRef(handlePickerKeyDown);
  pickerKeyDownRef.current = handlePickerKeyDown;
  useEffect(() => {
    const handler = (event: KeyboardEvent) => pickerKeyDownRef.current(event);
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  // Move focus into the search field the moment it expands (horizontal bar), so a click
  // on the search icon lands the caret without a second click. Only fires on the
  // false->true transition; the field is unmounted while collapsed.
  useEffect(() => {
    if (isSearchExpanded) {
      searchInputRef.current?.focus();
    }
  }, [isSearchExpanded]);

  // Custom window-chrome drag handle for frameless mode, shared by the boot/recovery
  // screen and the picker below (the matching minimize/close controls are the
  // WindowControls component). Gated on framelessApplied like the controls, so chrome
  // only appears once the native frame is actually gone. data-tauri-drag-region=""
  // adds the drag handle; undefined omits it (the chrome bands only become draggable
  // when frameless).
  const dragRegion = framelessApplied ? "" : undefined;

  // Boot/error screen: still probing, the probe itself failed (IPC error), or the
  // CLI is unavailable for the current runner — and no other open folder could
  // render (see dockBootScreenVisible in preflightViewModel). It surfaces the real
  // preflight error and the Open settings recovery path instead of a perpetual
  // live banner.
  if (bootScreenVisible) {
    const { probing, detail, suggestedBinaryPath } = dockBootScreenContent(
      preflightState,
      profileKindLabel(activeProfile),
    );
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
          <div className="boot-window-controls">
            <WindowControls />
          </div>
        ) : null}
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

      {/* The summon hotkey's registration failure is a standing condition with its
          own surface, so it no longer competes with one-shot activation feedback
          for a single slot (it used to ride PickerActivation with a null sourceKey
          and could only land when the slot was idle). */}
      {summonHotkey.status === "failed" ? (
        <div className="inline-error" role="alert">
          {summonHotkey.message}
        </div>
      ) : null}

      {/* A failed activation is a per-source event: in the vertical strip it renders
          inside the failing source's folder so healthy folders don't look broken.
          This global surface covers the horizontal bar (which shows one source). */}
      {activation.status === "failed" && effectiveOrientation === "horizontal" ? (
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
                activateRow(row, ownerProfile);
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
            {preflightError !== null &&
            !liveSources.some((source) => source.runnerKey === runnerKey) ? (
              // The active source can be folder-INeligible (e.g. a just-added remote
              // with no host yet): it renders no folder, and with another folder open
              // the boot screen is suppressed, so without this strip its failure has
              // no surface at all. Same recovery shape as the in-folder strip.
              <div className="live-strip error" aria-live="polite">
                <span className="status-dot" data-tone="error" />
                <span className="live-label">{labelFor(activeProfile)}</span>
                <span className="live-message">{preflightError}</span>
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
              const folderPreflightError =
                view.runnerKey === runnerKey ? preflightError : null;
              return (
                <section className="source-folder" key={view.profile.id}>
                  <button
                    className="folder-header"
                    type="button"
                    aria-expanded={view.isOpen}
                    onClick={() => toggleProfileOpenSet(view.profile.id)}
                    title={
                      folderPreflightError ??
                      (view.isOpen
                        ? view.live.connection.message
                        : "Closed — no live subscription")
                    }
                  >
                    <span
                      className={`status-dot${
                        folderPreflightError === null &&
                        view.isOpen &&
                        (view.live.connection.status === "connecting" ||
                          view.live.connection.status === "reconnecting")
                          ? " pulsing"
                          : ""
                      }`}
                      data-tone={
                        folderPreflightError !== null
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
                      {folderPreflightError !== null ? (
                        // The gated-off target's keyed state (connecting, no rows) would
                        // render a perpetual loading skeleton under this, so the strip
                        // replaces the picker body too. Mirrors the boot screen's
                        // recovery path; LiveStrip's Start/Reconnect can't fix a
                        // preflight failure, so it doesn't render here.
                        <div className="live-strip error" aria-live="polite">
                          <span className="status-dot" data-tone="error" />
                          <span className="live-label">Unavailable</span>
                          <span className="live-message">{folderPreflightError}</span>
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
                            onActivate={(row) => activateRow(row, view.profile)}
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
                // Vertical: only the order menu toggles — no selection happens
                // on this branch (the deep-link above is horizontal-exclusive,
                // where the trigger's label names exactly one source).
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
              {/* Draggable rows are safe inside the footer's frameless drag
                  region: Tauri's data-tauri-drag-region handler only fires
                  when the mousedown TARGET itself carries the attribute, so
                  descendants start HTML5 drags, never window drags. */}
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
          {framelessApplied ? <WindowControls /> : null}
        </div>
      </footer>
    </main>
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

export default DockApp;
