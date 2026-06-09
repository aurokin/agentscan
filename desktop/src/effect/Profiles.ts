import { Effect, SubscriptionRef } from "effect";
import { PrefsBridge } from "./PrefsBridge";
import {
  emptyRunnerSettings,
  getActiveProfile,
  isRunnableProfile,
  loadProfileState,
  newProfileId,
  normalizeProfileState,
  normalizeRunnerSettings,
  reorderProfile as reorderProfileState,
  sshHostCollides,
  storeProfileState,
  toggleProfileOpen as toggleProfileOpenState,
  updateProfileSettingsById,
  type ProfileState,
  type RunnerSettings,
  type SshProfileConfig,
} from "./profileModel";

export type ApplyRunnerSettingsInput = {
  // The form drafts, already validated by the caller. The service normalizes the
  // runner, then merges this onto the latest persisted state.
  readonly runner: RunnerSettings;
  readonly sshHost: string;
  readonly sshClientTty: string;
};

// The commit-time outcome, surfaced so the UI can report a refused apply instead
// of logging it as applied.
export type ApplyRunnerSettingsResult = "applied" | "duplicate-host";

// Owns the persisted profile/settings state (a SubscriptionRef the dock + settings
// windows observe via an atom), the persistence + cross-window broadcast on every
// change, and a value-guarded reload primitive.
//
// Every mutator merges onto the LATEST persisted state (not the in-memory ref): a
// warm settings window can hold a deliberately-stale ref (React skips inbound
// profile syncs while the form is dirty), so writing the whole ref back would
// clobber dock-side add/delete/source-switch changes already in storage.
// Persistence and the prefs channel are reached only through the injected
// PrefsBridge, so the whole service is pure logic over that boundary and a vitest
// layer can drive it. Running the mutators in an Effect fiber (not a React setState
// updater) also drops the StrictMode double-invoke hazard the old `addSshProfile`
// had to guard against with a generate-id-once dance.
//
// Inbound `{kind:"profiles"}` adoption is NOT owned here: it is gated on the
// settings form's unsaved-edit flag, which is React-synchronous state. React makes
// the adopt/skip decision (reading that flag with no async lag) and calls `reload`.
// Keeping the decision in React preserves the original synchronous gate; pushing the
// dirty flag into a service Ref would let an inbound sync race ahead of the push and
// clobber a just-started edit.
export class Profiles extends Effect.Service<Profiles>()("desktop/Profiles", {
  dependencies: [PrefsBridge.Default],
  scoped: Effect.gen(function* () {
    const bridge = yield* PrefsBridge;
    const stateRef = yield* SubscriptionRef.make<ProfileState>(loadProfileState(bridge.loadRaw));

    // Write a fresh state through: persist, mirror to the other window, publish to
    // observers. The single path every mutator funnels through.
    const commit = (next: ProfileState) =>
      Effect.gen(function* () {
        yield* Effect.sync(() => storeProfileState(bridge.storeRaw, next));
        yield* bridge.emit({ kind: "profiles" });
        yield* SubscriptionRef.set(stateRef, next);
      });

    // Value-guarded re-read of persisted profiles into the ref. Backs the dock's
    // inbound adoption and the settings window's focus/clean reconcilers (emitTo has
    // no replay, so a sync missed while hidden is recovered on the next focus).
    const reload = SubscriptionRef.update(stateRef, (current) => {
      const reloaded = loadProfileState(bridge.loadRaw);
      return JSON.stringify(current) === JSON.stringify(reloaded) ? current : reloaded;
    });

    const selectProfile = (id: string) =>
      Effect.gen(function* () {
        // Switch on the LATEST persisted state so a concurrent dock-side add/delete
        // isn't clobbered by this window's possibly-stale ref. (The dirty-window
        // same-card-reclick no-op is handled by the React caller, where the dirty
        // flag is synchronous.)
        const current = yield* SubscriptionRef.get(stateRef);
        const latest = loadProfileState(bridge.loadRaw);
        if (id === latest.activeProfileId) {
          // Clicking the already-persisted active source must not re-commit (a needless
          // write/broadcast that would also re-run every profileState-keyed effect in
          // both windows). Adopt latest only if our ref actually diverged (a dirty
          // window may lag).
          if (JSON.stringify(current) !== JSON.stringify(latest)) {
            yield* SubscriptionRef.set(stateRef, latest);
          }
          return;
        }

        const profile = latest.profiles.find((candidate) => candidate.id === id);
        if (!profile || !isRunnableProfile(profile)) {
          return;
        }

        yield* commit({ ...latest, activeProfileId: id });
      });

    const addSshProfile = Effect.gen(function* () {
      const latest = loadProfileState(bridge.loadRaw);
      // "Add remote" means "give me an unconfigured draft to fill in", and at most
      // one can exist: labels derive from the connection, so a second empty-host
      // draft would render as an identical card the user can't tell apart. Re-adding
      // routes back to the existing draft instead.
      const draft = latest.profiles.find(
        (profile) => profile.kind === "ssh" && profile.host.trim() === "",
      );
      if (draft) {
        if (draft.id !== latest.activeProfileId) {
          yield* commit({ ...latest, activeProfileId: draft.id });
        }
        return;
      }
      const profile: SshProfileConfig = {
        id: newProfileId("ssh"),
        kind: "ssh",
        host: "",
        clientTty: "",
        runner: emptyRunnerSettings(),
        enabled: true,
      };
      yield* commit({
        activeProfileId: profile.id,
        profiles: [...latest.profiles, profile],
        // New sources start open so the folder appears (and streams) the moment
        // the host is configured.
        openProfileIds: [...latest.openProfileIds, profile.id],
      });
    });

    const deleteActiveProfile = Effect.gen(function* () {
      const current = yield* SubscriptionRef.get(stateRef);
      const activeProfile = getActiveProfile(current);
      if (activeProfile.kind === "local") {
        return;
      }

      const targetId = activeProfile.id;
      const latest = loadProfileState(bridge.loadRaw);
      const profiles = latest.profiles.filter((profile) => profile.id !== targetId);
      const fallback = profiles.find((profile) => profile.kind === "local") ?? profiles[0];
      const next = normalizeProfileState({
        activeProfileId: profiles.some((profile) => profile.id === latest.activeProfileId)
          ? latest.activeProfileId
          : fallback?.id,
        profiles,
        // Pass the open set through (normalize drops the deleted id); omitting it
        // would trip the legacy migration and reset every folder to closed-but-active.
        openProfileIds: latest.openProfileIds,
      });
      yield* commit(next);
    });

    // Open/close one source's folder. Merges onto the LATEST persisted state like
    // every mutator; an unknown id is a no-op (no write, no broadcast).
    const toggleProfileOpen = (id: string) =>
      Effect.gen(function* () {
        const latest = loadProfileState(bridge.loadRaw);
        const next = toggleProfileOpenState(latest, id);
        if (next === latest) {
          return;
        }
        yield* commit(next);
      });

    // Drag-reorder one source onto another. Keybind ownership is derived from the
    // resulting profiles order; a no-op move commits nothing.
    const reorderProfile = (id: string, targetId: string) =>
      Effect.gen(function* () {
        const latest = loadProfileState(bridge.loadRaw);
        const next = reorderProfileState(latest, id, targetId);
        if (next === latest) {
          return;
        }
        yield* commit(next);
      });

    const applyRunnerSettings = (input: ApplyRunnerSettingsInput) =>
      Effect.gen(function* () {
        // Edit the profile this form targets (the ref's active id, which the form
        // mirrors), merged onto the latest persisted list + active source.
        const editedId = (yield* SubscriptionRef.get(stateRef)).activeProfileId;
        const latest = loadProfileState(bridge.loadRaw);
        // Re-check the one-host-per-source invariant at commit time: the form
        // validated against its own window's list, which can be stale while another
        // window edits, and a persisted duplicate would be silently dropped (source
        // deleted) by load-time dedupe. Refusing the write loses the edit, not data.
        const edited = latest.profiles.find((profile) => profile.id === editedId);
        if (edited?.kind === "ssh" && sshHostCollides(latest.profiles, editedId, input.sshHost)) {
          // Adopt the state that won so the form's live validation surfaces the
          // duplicate inline instead of leaving a silently-unsynced window.
          yield* reload;
          return "duplicate-host" as const;
        }
        const next = updateProfileSettingsById(
          latest,
          editedId,
          normalizeRunnerSettings(input.runner),
          input.sshHost,
          input.sshClientTty,
        );
        yield* commit(next);
        return "applied" as const;
      });

    return {
      state: stateRef,
      selectProfile,
      addSshProfile,
      deleteActiveProfile,
      applyRunnerSettings,
      toggleProfileOpen,
      reorderProfile,
      // Value-guarded reconcile from storage, driven by React on the cross-window
      // profiles sync and the settings focus/clean transitions.
      reload,
    };
  }),
}) {}
