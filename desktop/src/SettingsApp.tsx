import { useEffect, useMemo, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Result, useAtomSet, useAtomValue } from "@effect-atom/atom-react";
import { DebugLog } from "./components/DebugLog";
import { SourceKindIcon } from "./components/SourceKindIcon";
import { IS_MAC } from "./platform";
import {
  addSshProfileAtom,
  appearanceAtom,
  appendDebugEntryAtom,
  applyRunnerSettingsAtom,
  clearDebugLogAtom,
  debugLogAtom,
  deleteActiveProfileAtom,
  localHostLabelAtom,
  profilesAtom,
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
  syncedPreflightAtom,
  toggleProfileOpenAtom,
} from "./effect/atoms";
// Type-only: the DebugLog service class stays out of this file (the DebugLog
// component import above would collide with it).
import type { DebugEntry } from "./effect/DebugLog";
import {
  loadAppearance,
  SURFACE_ALPHA_MAX,
  SURFACE_ALPHA_MIN,
} from "./effect/appearanceModel";
import {
  folderProfiles,
  getActiveProfile,
  loadProfileState,
  normalizeRunnerSettings,
  profileDraftDirty,
  runnerKeyForProfile,
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
  type OrientationPreference,
  type PrefsSync,
  type ThemePreference,
} from "./effect/prefs";
import { settingsPreflightCard } from "./effect/settingsViewModel";
import { errorMessage, readLocalStorage } from "./shared";

// First-paint fallback for debugLogAtom before the runtime resolves it.
const EMPTY_DEBUG_ENTRIES: ReadonlyArray<DebugEntry> = [];

// The settings window: source rail + form drafts, the appearance controls, and
// the debug log panel. It never probes, never subscribes to live pickers, and
// never binds the summon hotkey — those are DockApp's; its instances of the
// dock-driven services stay inert because no configure path here ever drives
// them (and the runtime layer build itself is side-effect-free; see shared.ts).
//
// Deliberately absent, with the invariants that make the absence safe: no
// orientation resize listener (the settings <main> carries no data-orientation
// attribute and every orientation rule in styles.css scopes to the dock's
// .sidebar[data-orientation=...]), and no --surface-alpha writer (the var's
// only consumer is :root[data-glass="on"] .sidebar, and data-glass is written
// only by the dock-gated glass effect). If settings UI ever adopts either,
// bring the dock's effects over.
function SettingsApp() {
  // This window never runs its own preflight; it reuses the dock's resolved
  // result, mirrored over the prefs channel into the service's `synced` ref
  // (observed here). This avoids a second `ssh … --version` for remote profiles
  // (an extra round-trip and a possible duplicate passphrase prompt) every time
  // Settings is opened, and keeps the card current even while the window is
  // visible-but-unfocused. requestPreflightSync asks the dock to re-emit on
  // focus (emitTo has no replay).
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
  // Identity of the exact runner configuration a resolved state describes. It
  // changes on a profile switch AND on any settings edit (binary/env/host/tty)
  // to the active profile; the synced-preflight card only trusts a mirror whose
  // runnerKey matches it.
  const runnerKey = useMemo(() => runnerKeyForProfile(activeProfile), [activeProfile]);
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
  // The debug log lives in the DebugLog service (per-window instance: this
  // window renders and clears its own log; the dock writes to its own). The
  // append setter is registry-stable, unlike the old per-render closure, so
  // logging effects can list it in their dep arrays.
  const debugEntries = Result.getOrElse(useAtomValue(debugLogAtom), () => EMPTY_DEBUG_ENTRIES);
  const appendDebugEntry = useAtomSet(appendDebugEntryAtom);
  const clearDebugLog = useAtomSet(clearDebugLogAtom);
  // The local machine's short hostname, resolved once per webview runtime by
  // the HostIpc-backed atom, shown as the local source's label (the way a
  // remote source is keyed by its SSH host). Empty while unresolved AND on
  // failure; sourceLabel falls back to a generic label for "".
  const localHostLabel = Result.getOrElse(useAtomValue(localHostLabelAtom), () => "");
  // The probed remote hostname as a label source: this window reuses the dock's
  // mirror. sourceLabel only honors it for the profile whose runnerKey matches,
  // so a stale probe can never label a source.
  const labelPreflight: PreflightLabelSource | null = syncedPreflight;
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
  // Appearance prefs (theme + dock-layout orientation + glass) are owned by the
  // Appearance Effect service; both windows observe its state via an atom, and this
  // window drives changes through these setters (which persist + cross-window
  // broadcast — the dock applies the native/DOM side). The first synchronous render
  // (before the runtime resolves the atom) falls back to a direct storage read so the
  // controls are right on the first paint.
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
    () => profileDraftDirty(activeProfile, settingsDraft, sshHostDraft, sshClientTtyDraft),
    [activeProfile, settingsDraft, sshHostDraft, sshClientTtyDraft],
  );
  // Render-synced mirror so the focus-reconcile listener can read the latest
  // dirty state without re-subscribing on every keystroke.
  const isSettingsDirtyRef = useRef(isSettingsDirty);
  isSettingsDirtyRef.current = isSettingsDirty;

  useEffect(() => {
    // Drafts follow the settings form's target on a switch (id) or an in-place edit
    // of its committed values (runnerKey). Keyed on those VALUES, not the
    // activeProfile object: every service commit re-reads storage (all-new object
    // identities), so an identity key would also fire on commits that don't retarget
    // the form — drag-reorder, open-toggle — and clobber unsaved edits.
    setSettingsDraft(activeProfile.runner);
    setSshHostDraft(activeProfile.kind === "ssh" ? activeProfile.host : "");
    setSshClientTtyDraft(activeProfile.kind === "ssh" ? activeProfile.clientTty : "");
    // activeProfile is fully determined by (id, runnerKey) where this reads it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeProfile.id, runnerKey]);

  // Apply the theme to <html data-theme>. "system" resolves from prefers-color-scheme
  // and re-resolves live when the OS appearance changes. Persistence + the cross-window
  // broadcast are owned by the Appearance service (driven by the setter); this effect
  // only applies the resolved theme to this window's DOM. (Per-theme logo variant
  // selection is dock-only — nothing here renders provider logos.)
  useEffect(() => {
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

  // This window closes by hiding (kept warm), so a back/Done press just hides it.
  // It never probes (it reuses the dock's synced preflight), so there's no
  // per-window probe to stop on hide.
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
        // window is hidden, not closed, precisely to preserve it). The skipped change
        // is reconciled later via the focus/clean reload paths.
        //
        // The reload applies via an async hop (atom dispatch -> Effect fiber ->
        // service ref -> re-render). The trigger stays synchronously dirty-gated, and
        // the reload is value-guarded (an equal snapshot leaves the ref untouched, so
        // the [activeProfile] reset effect never fires). The only residual is a sub-ms
        // window where an edit begun between this dirty check and a genuine-change
        // reload landing could be reset; the focus/clean reconcilers recover state on
        // next focus.
        if (isSettingsDirtyRef.current) {
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
  }, [reloadAppearance, reloadProfiles, requestPreflightSync]);

  // The React listener drops dock-side syncs while this window is dirty, and the focus-
  // reconcile only runs on a later focus — so a change skipped mid-edit would linger
  // after Reset/Apply clears the drafts (still focused, no new focus event). Reconcile
  // from storage whenever the window is clean. The service's reload is value-guarded,
  // so an unchanged snapshot doesn't churn state or reset the picker selection.
  useEffect(() => {
    if (isSettingsDirty) {
      return;
    }
    reloadProfiles();
  }, [isSettingsDirty, reloadProfiles]);

  // Keep the settings window warm: intercept its close so the red button / Cmd-W
  // hides it (instant reopen, drafts preserved) rather than destroying it.
  useEffect(() => {
    const win = getCurrentWindow();
    const unlistenPromise = win.onCloseRequested((event) => {
      event.preventDefault();
      void win.hide();
    });
    return () => {
      void unlistenPromise.then((unlisten) => unlisten());
    };
  }, []);

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
    if (isSettingsDirty && id === activeProfile.id) {
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

  // The profile list comes from profileState (the live source of truth) so
  // add/delete/switch are reflected immediately. The status card's trust rule
  // (the mirror counts only when its runnerKey matches this window's active
  // source) lives in settingsPreflightCard, tested in effect/settingsViewModel.
  const preflightCard = settingsPreflightCard(syncedPreflight, runnerKey);
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

            <div className="detail-status" data-tone={preflightCard.tone}>
              <span
                className={`status-dot${preflightCard.tone === "unknown" ? " pulsing" : ""}`}
                data-tone={preflightCard.tone}
                aria-hidden="true"
              />
              <span className="detail-status-text">
                <strong>{preflightCard.label}</strong>
                <span className="mono detail-status-detail">{preflightCard.detail}</span>
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
              <button className="ghost-button" type="button" onClick={() => clearDebugLog()}>
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

export default SettingsApp;
