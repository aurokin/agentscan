import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { Result, useAtomSet, useAtomValue } from "@effect-atom/atom-react";
import { BootScreen } from "./components/BootScreen";
import { GroupedPicker } from "./components/GroupedPicker";
import { LiveStrip } from "./components/LiveStrip";
import { SourceFolders } from "./components/SourceFolders";
import { SourceSwitcher, type SourceMenuItem } from "./components/SourceSwitcher";
import { WindowControls } from "./components/WindowControls";
import { IS_MAC } from "./platform";
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
  localHostLabelAtom,
  preflightStateAtom,
  profilesAtom,
  pruneActivationAtom,
  reconnectAtom,
  reloadProfilesAtom,
  reorderProfileAtom,
  selectProfileAtom,
  setProfileEnabledAtom,
  startAtom,
  summonHotkeyAtom,
  toggleProfileOpenAtom,
} from "./effect/atoms";
import type { LiveState, PickerRow } from "./effect/types";
import { liveStateFor, type LiveStates } from "./effect/LiveConnection";
import { pickerKeyIntent } from "./effect/keybinds";
import type { PreflightState } from "./effect/Preflight";
import { loadAppearance } from "./effect/appearanceModel";
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
import { PREFS_SYNC_EVENT, type PrefsSync } from "./effect/prefs";
import {
  deriveSourceViews,
  footerTriggerView,
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
import { useWindowChrome } from "./useWindowChrome";
import { errorMessage, readLocalStorage } from "./shared";

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

// The dock's resolved preflight is owned by the Preflight Effect service (observed via
// preflightStateAtom as PreflightState); the picker rows + live connection status live
// in the LiveConnection service (liveStatesAtom).
const INITIAL_PREFLIGHT: PreflightState = { status: "loading" };

// Stable empty fallbacks for the no-owner case so effect dep arrays don't churn.
const EMPTY_PICKER_ROWS: PickerRow[] = [];
const EMPTY_PICKER_GROUPS: PickerGroup[] = [];

// The dock window: boot/recovery screen, the folder strip / horizontal bar
// picker, the live subscriptions, and the preflight prober. The native window
// chrome (sizing, glass, frameless, the summon hotkey) is driven by the
// useWindowChrome hook below. The settings window is SettingsApp.tsx; the two
// never import each other (see shared.ts).
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
  const setProfileEnabledSet = useAtomSet(setProfileEnabledAtom);
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
  const sourceMenuItems = useMemo<SourceMenuItem[]>(
    () =>
      profileState.profiles
        .filter((profile) => profile.kind === "local" || profile.host.trim().length > 0)
        .map((profile) => {
          const liveSource = liveSources.find((source) => source.profile.id === profile.id);
          return {
            profile,
            enabled: profile.kind === "local" || profile.enabled,
            canToggle: profile.kind === "ssh",
            isOwner: liveSource?.isOwner ?? false,
          };
        }),
    [liveSources, profileState.profiles],
  );
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
  // Appearance prefs (theme + dock-layout orientation + glass) are owned by the
  // Appearance Effect service; the settings window drives changes (which persist +
  // cross-window broadcast), the dock observes and applies. The first synchronous
  // render (before the runtime resolves the atom) falls back to a direct storage
  // read so layout/theme/glass are right on the first paint.
  const initialAppearance = useMemo(() => loadAppearance(readLocalStorage), []);
  const appearance = Result.getOrElse(useAtomValue(appearanceAtom), () => initialAppearance);
  const { themePref, orientationPref, glassEnabled, surfaceAlpha, framelessEnabled } =
    appearance;
  // Every native/DOM apply for those prefs — theme, orientation tracking, window
  // shape, glass, surface alpha, frameless, the summon-hotkey arming — lives in
  // useWindowChrome (dock-only; see its header). The render below consumes only
  // what it returns.
  const { effectiveOrientation, framelessApplied, resolvedTheme, dragRegion } =
    useWindowChrome({
      themePref,
      orientationPref,
      glassEnabled,
      surfaceAlpha,
      framelessEnabled,
      appendDebugEntry,
      configureSummonHotkey,
    });
  // The local machine's short hostname, resolved once per webview runtime by
  // the HostIpc-backed atom, shown as the local source's label (the way a
  // remote source is keyed by its SSH host). Empty while unresolved AND on
  // failure; sourceLabel falls back to a generic label for "".
  const localHostLabel = Result.getOrElse(useAtomValue(localHostLabelAtom), () => "");
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

  // Open the settings window (created hidden at launch, kept warm). The dock no
  // longer renders settings itself. (Closing the source menu is no longer this
  // function's job: the menu state lives in SourceSwitcher, whose own dismiss
  // paths cover every call site that can coexist with an open menu.)
  const openSettings = () => {
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
      <BootScreen
        probing={probing}
        detail={detail}
        suggestedBinaryPath={suggestedBinaryPath}
        orientation={effectiveOrientation}
        dragRegion={dragRegion}
        framelessApplied={framelessApplied}
        onApplySuggestedBinaryPath={applySuggestedBinaryPath}
        onOpenSettings={openSettings}
      />
    );
  }

  // Horizontal bar: collapse search to an icon unless the user expanded it or a query is
  // active. The vertical strip always shows the full field (searchCollapsed is never true).
  const searchCollapsed =
    effectiveOrientation === "horizontal" && !isSearchExpanded && !pickerFilter.trim();

  // Footer trigger presentation (owner-or-active profile, when the label shows
  // a source vs the generic menu entry, dot tone, hover title): derived +
  // tested in effect/pickerViewModel.
  const {
    profile: triggerProfile,
    showsSource: triggerShowsSource,
    tone: triggerTone,
    title: triggerTitle,
  } = footerTriggerView({
    ownerProfile,
    activeProfile,
    ownerConnection: ownerView?.live.connection ?? null,
    sourceStatusTone,
    statusText,
    orientation: effectiveOrientation,
    sourceCount: liveSources.length,
  });

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
            connectionOffline={!!ownerView && ownerView.live.connection.status !== "online"}
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
          <SourceFolders
            sourceViews={sourceViews}
            activation={activation}
            pickerFilter={pickerFilter}
            selectedPaneId={selectedPaneId}
            resolvedTheme={resolvedTheme}
            runnerKey={runnerKey}
            preflightError={preflightError}
            activeProfile={activeProfile}
            labelFor={labelFor}
            onOpenSettings={openSettings}
            onToggleFolder={(profileId) => toggleProfileOpenSet(profileId)}
            onActivate={(row, profile) => activateRow(row, profile)}
            onSelect={(row) => setSelectedPaneId(row.pane_id)}
            onStart={(key) => startLive(key)}
            onReconnect={(key) => reconnectLive(key)}
            onClearFilter={() => setPickerFilter("")}
          />
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
        <SourceSwitcher
          sourceMenuItems={sourceMenuItems}
          triggerProfile={triggerProfile}
          triggerShowsSource={triggerShowsSource}
          triggerTone={triggerTone}
          triggerTitle={triggerTitle}
          orientation={effectiveOrientation}
          labelFor={labelFor}
          selectProfile={(id) => selectProfileSet(id)}
          reorderProfile={(input) => reorderProfileSet(input)}
          setProfileEnabled={(input) => setProfileEnabledSet(input)}
          onOpenSettings={openSettings}
        />
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
