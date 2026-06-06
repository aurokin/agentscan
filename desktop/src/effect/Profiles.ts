import { Effect, SubscriptionRef } from "effect";
import { PrefsBridge } from "./PrefsBridge";
import {
  emptyRunnerSettings,
  getActiveProfile,
  isRunnableProfile,
  loadProfileState,
  newProfileId,
  nextRemoteProfileName,
  normalizeProfileState,
  normalizeRunnerSettings,
  storeProfileState,
  updateProfileSettingsById,
  type ProfileState,
  type RunnerSettings,
  type SshProfileConfig,
} from "./profileModel";

export type ApplyRunnerSettingsInput = {
  // The form drafts, already validated by the caller. The service normalizes the
  // runner and trims the name, then merges this onto the latest persisted state.
  readonly name: string;
  readonly runner: RunnerSettings;
  readonly sshHost: string;
  readonly sshClientTty: string;
};

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
          // write/broadcast momentarily flickers the dock through "Switching profile…").
          // Adopt latest only if our ref actually diverged (a dirty window may lag).
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
      const profile: SshProfileConfig = {
        id: newProfileId("ssh"),
        name: nextRemoteProfileName(latest.profiles),
        kind: "ssh",
        host: "",
        clientTty: "",
        runner: emptyRunnerSettings(),
        enabled: true,
      };
      yield* commit({
        activeProfileId: profile.id,
        profiles: [...latest.profiles, profile],
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
      });
      yield* commit(next);
    });

    const applyRunnerSettings = (input: ApplyRunnerSettingsInput) =>
      Effect.gen(function* () {
        // Edit the profile this form targets (the ref's active id, which the form
        // mirrors), merged onto the latest persisted list + active source.
        const editedId = (yield* SubscriptionRef.get(stateRef)).activeProfileId;
        const latest = loadProfileState(bridge.loadRaw);
        const next = updateProfileSettingsById(
          latest,
          editedId,
          input.name.trim(),
          normalizeRunnerSettings(input.runner),
          input.sshHost,
          input.sshClientTty,
        );
        yield* commit(next);
      });

    return {
      state: stateRef,
      selectProfile,
      addSshProfile,
      deleteActiveProfile,
      applyRunnerSettings,
      // Value-guarded reconcile from storage, driven by React on the cross-window
      // profiles sync and the settings focus/clean transitions.
      reload,
    };
  }),
}) {}
