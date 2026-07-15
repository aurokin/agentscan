import { useEffect, useMemo, useRef, useState } from "react";
import {
  newProfileId,
  normalizeRunnerSettings,
  profileDraftDirty,
  runnerSummary,
  validateProfileDraft,
  type DesktopProfileConfig,
  type EnvironmentVariable,
  type RunnerSettings,
} from "./effect/profileModel";
import type { ApplyRunnerSettingsInput, ApplyRunnerSettingsResult } from "./effect/Profiles";
import type { DebugEntryInput } from "./effect/DebugLog";
import { errorMessage } from "./shared";

// Draft rows carry a render-only identity so React keys survive mid-list
// removals — an index key makes every row below a deleted entry remount and
// drop its caret/focus. `normalizeRunnerSettings` rebuilds bare {name, value}
// rows at the apply boundary, so the id never reaches persistence.
export type EnvironmentVariableDraft = EnvironmentVariable & { id: string };
export type RunnerSettingsDraft = Omit<RunnerSettings, "env"> & {
  env: EnvironmentVariableDraft[];
};

function withEnvRowIds(runner: RunnerSettings): RunnerSettingsDraft {
  return {
    ...runner,
    env: runner.env.map((variable) => ({ ...variable, id: newProfileId("env") })),
  };
}

// SettingsApp-only: the settings form's draft state, validation/dirty
// derivations, the draft-reset and clean-reconcile effects, and the apply/
// reset/env handlers. Deliberately atom-free — the atom-bound collaborators
// (the debug-log appender, the promise-mode apply action, the value-guarded
// reload) arrive as arguments — so the module needs no Tauri host and a
// renderHook test needs no mocks. The PREFS listener and the focus reconciler
// stay in SettingsApp; they read the returned isSettingsDirtyRef.
export function useSettingsForm({
  initialProfile,
  activeProfile,
  profiles,
  runnerKey,
  labelFor,
  appendDebugEntry,
  applyRunnerSettingsSet,
  reloadProfiles,
}: {
  // getActiveProfile(initialProfileState): the synchronous storage read the
  // window seeds from, so the drafts are right on the very first paint,
  // matching the service seed.
  initialProfile: DesktopProfileConfig;
  activeProfile: DesktopProfileConfig;
  profiles: DesktopProfileConfig[];
  runnerKey: string;
  // Recreated by SettingsApp every render on purpose (it closes over the live
  // hostname/preflight label sources); the handlers below are plain per-render
  // functions, so the labels they log never go stale. Do not memoize them.
  labelFor: (profile: DesktopProfileConfig) => string;
  appendDebugEntry: (entry: DebugEntryInput) => void;
  applyRunnerSettingsSet: (input: ApplyRunnerSettingsInput) => Promise<ApplyRunnerSettingsResult>;
  reloadProfiles: () => void;
}) {
  const [settingsDraft, setSettingsDraft] = useState<RunnerSettingsDraft>(() =>
    withEnvRowIds(initialProfile.runner),
  );
  const [sshHostDraft, setSshHostDraft] = useState(() =>
    initialProfile.kind === "ssh" ? initialProfile.host : "",
  );
  const [sshClientTtyDraft, setSshClientTtyDraft] = useState(() =>
    initialProfile.kind === "ssh" ? initialProfile.clientTty : "",
  );
  const validation = useMemo(
    () =>
      validateProfileDraft(activeProfile, settingsDraft, sshHostDraft, sshClientTtyDraft, profiles),
    [activeProfile, settingsDraft, sshHostDraft, sshClientTtyDraft, profiles],
  );
  const isSettingsDirty = useMemo(
    () => profileDraftDirty(activeProfile, settingsDraft, sshHostDraft, sshClientTtyDraft),
    [activeProfile, settingsDraft, sshHostDraft, sshClientTtyDraft],
  );
  // Render-synced mirror so the focus-reconcile listener can read the latest
  // dirty state without re-subscribing on every keystroke. A deliberate
  // render-phase write: the hook runs during SettingsApp's render, so the ref
  // is exactly as React-synchronous as the pre-hook inline version.
  const isSettingsDirtyRef = useRef(isSettingsDirty);
  isSettingsDirtyRef.current = isSettingsDirty;

  useEffect(() => {
    // Drafts follow the settings form's target on a switch (id) or an in-place edit
    // of its committed values (runnerKey). Keyed on those VALUES, not the
    // activeProfile object: every service commit re-reads storage (all-new object
    // identities), so an identity key would also fire on commits that don't retarget
    // the form — drag-reorder, open-toggle — and clobber unsaved edits.
    setSettingsDraft(withEnvRowIds(activeProfile.runner));
    setSshHostDraft(activeProfile.kind === "ssh" ? activeProfile.host : "");
    setSshClientTtyDraft(activeProfile.kind === "ssh" ? activeProfile.clientTty : "");
    // activeProfile is fully determined by (id, runnerKey) where this reads it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeProfile.id, runnerKey]);

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

  function applyRunnerSettings() {
    const validation = validateProfileDraft(
      activeProfile,
      settingsDraft,
      sshHostDraft,
      sshClientTtyDraft,
      profiles,
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
    setSettingsDraft(withEnvRowIds(normalized));
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
    setSettingsDraft(withEnvRowIds(activeProfile.runner));
    setSshHostDraft(activeProfile.kind === "ssh" ? activeProfile.host : "");
    setSshClientTtyDraft(activeProfile.kind === "ssh" ? activeProfile.clientTty : "");
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
      env: [...current.env, { name: "", value: "", id: newProfileId("env") }],
    }));
  }

  function removeEnvironmentVariable(index: number) {
    setSettingsDraft((current) => ({
      ...current,
      env: current.env.filter((_, variableIndex) => variableIndex !== index),
    }));
  }

  return {
    settingsDraft,
    // Returned raw: the binary field and the apply-normalize echo write
    // through it directly, so a handlers-only API would drop a writer.
    setSettingsDraft,
    sshHostDraft,
    setSshHostDraft,
    sshClientTtyDraft,
    setSshClientTtyDraft,
    validation,
    isSettingsDirty,
    isSettingsDirtyRef,
    applyRunnerSettings,
    resetProfileSettings,
    updateEnvironmentVariable,
    addEnvironmentVariable,
    removeEnvironmentVariable,
  };
}
