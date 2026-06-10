// Hostname-label enrichment for ssh sources, extracted from the two App.tsx
// effects that owned it: recording the ACTIVE source's probed hostname from the
// preflight driver's resolved state, and one-shot background probes for
// never-probed NON-active sources once their channel is online. Both persist
// through Profiles.recordProbedHost, whose model-layer re-verification
// (id/kind/runnerKey/unchanged against latest persisted storage) is the single
// staleness safety net every path here leans on.
//
// Deliberate deviation from the old active recorder: it additionally required
// the resolved state to match the CURRENT active runnerKey and recorded only
// onto the active profile. This service records onto whichever profile still
// owns the probed runnerKey — ssh host uniqueness (commit-time collision
// refusal + load-time dedupe) makes that lookup unambiguous, runnerKey is the
// full connection identity, and a host edit clears probedHost AND moves the
// runnerKey, so a moved connection can never wear an old machine's label. The
// result is a strict superset of the old recordings with identical values
// (e.g. a probe resolving just after the user switched active sources now
// still lands, instead of being re-earned by a background probe later).
//
// Timing note the recorder relies on: it records on preflight.state TICKS
// (the old effect also re-ran on profile changes). That is lossless because
// the dock's preflight driver re-fires a gen-bumped configure on EVERY
// runnerKey change (see Preflight.configure), so any window where a recording
// could newly apply ends in a fresh resolved tick. If the driver ever gains
// per-runnerKey probe caching or debouncing, revisit this.

import { Effect, Fiber, Stream, SubscriptionRef } from "effect";
import { hostnameProbeCandidates, sourceRunnerKeys, type HostnameProbeCandidate } from "./enrichmentModel";
import { LiveConnection } from "./LiveConnection";
import { Preflight, PreflightIpc } from "./Preflight";
import { Profiles } from "./Profiles";
import { runnerKeyForProfile } from "./profileModel";

// Debug-log sink ("hostname probe (<host>)" + started/ok/error detail) — the
// debug panel is React state, so the dock routes entries, like SummonHotkey's
// configure-time press callback. The active recorder logs nothing (parity with
// the old effect, whose driver probe is logged elsewhere).
export type EnrichmentLog = (label: string, detail: string) => void;

export class HostnameEnrichment extends Effect.Service<HostnameEnrichment>()(
  "desktop/HostnameEnrichment",
  {
    dependencies: [PreflightIpc.Default, Profiles.Default, Preflight.Default, LiveConnection.Default],
    scoped: Effect.gen(function* () {
      const ipc = yield* PreflightIpc;
      const profiles = yield* Profiles;
      const preflight = yield* Preflight;
      const lc = yield* LiveConnection;
      // The service scope: probe fibers are forked into it so they survive a
      // configure re-arm (StrictMode double-configures on every dock boot) and
      // settle like the old never-cancelled invoke promises did.
      const scope = yield* Effect.scope;
      // Serializes configures so two can't interleave their interrupt/fork pairs.
      const mutex = yield* Effect.makeSemaphore(1);
      // The probed-this-session runnerKeys, owned by the service (NOT a
      // configure closure): marks must survive a re-arm or the replay tick
      // would re-probe an in-flight or already-failed host. Like the old
      // App.tsx ref, it dies only with the window. Mutated only by the armed
      // background supervisor (one at a time, under the slot discipline).
      const attempted = new Set<string>();
      // The armed supervisors (one fiber running both). Mutated under `mutex`.
      let slot: Fiber.RuntimeFiber<void> | null = null;

      // Persist one probed hostname. Uninterruptible so a re-arm can't tear
      // Profiles.commit between its storage write and the ref/broadcast
      // publication; the model layer drops stale/unchanged/empty values.
      const record = (id: string, probedHost: string, runnerKey: string) =>
        Effect.uninterruptible(profiles.recordProbedHost(id, probedHost, runnerKey));

      // Recorder: persist the preflight driver's probed hostname onto the
      // profile that owns the probed runnerKey. Loop-free by construction: it
      // observes preflight.state only, recordings mutate profiles only, and an
      // unchanged value commits nothing. The ref lookup just resolves WHICH
      // profile id to offer (a missed lookup — deleted/retargeted before the
      // probe resolved — skips this emission); recordProbedHost re-verifies
      // identity against latest persisted storage at commit time.
      const recordDriverProbes = preflight.state.changes.pipe(
        Stream.changes,
        Stream.runForEach((state) =>
          Effect.gen(function* () {
            if (state.status !== "ready") {
              return;
            }
            const probed = state.preflight.remoteHostLabel?.trim() ?? "";
            if (!probed) {
              return;
            }
            const current = yield* SubscriptionRef.get(profiles.state);
            const owner = current.profiles.find(
              (profile) => runnerKeyForProfile(profile) === state.runnerKey,
            );
            if (!owner) {
              return;
            }
            yield* record(owner.id, probed, state.runnerKey);
            // A failed commit (storage write) must not end enrichment for the
            // session — the old per-call atom effect failed in isolation too.
          }).pipe(Effect.catchAllDefect(() => Effect.void)),
        ),
      );

      // One background probe, forked into the service scope by the supervisor
      // below. Mirrors the old runCommand sequence exactly: "started", then
      // "ok" (even when the preflight carries no hostname) or the raw error
      // detail; failures are swallowed beyond the log, and the attempt mark
      // stands until the key leaves the source list.
      const runProbe = (candidate: HostnameProbeCandidate, onLog: EnrichmentLog) =>
        Effect.gen(function* () {
          const label = `hostname probe (${candidate.host})`;
          yield* Effect.sync(() => onLog(label, "started"));
          const outcome = yield* ipc.probe(candidate.settings).pipe(
            Effect.map((result) => ({ ok: true as const, result })),
            // IpcError.message carries the raw Tauri rejection string, matching
            // the old errorMessage(error) log detail verbatim.
            Effect.catchAll((error) => Effect.succeed({ ok: false as const, message: error.message })),
          );
          if (!outcome.ok) {
            yield* Effect.sync(() => onLog(label, outcome.message));
            return;
          }
          yield* Effect.sync(() => onLog(label, "ok"));
          const probed = outcome.result.remoteHostLabel?.trim() ?? "";
          if (!probed) {
            return;
          }
          // The candidate's runnerKey rides along: recordProbedHost drops the
          // result if the profile was retargeted while this probe was in flight.
          yield* record(candidate.profileId, probed, candidate.runnerKey);
        }).pipe(Effect.catchAllDefect(() => Effect.void));

      // Background prober: on every profiles/live tick, prune the attempt set
      // to the current sources, then mark and fork a probe per candidate. The
      // triggers carry no payload — both refs are read fresh per tick, so a
      // stale-side pairing can't exist. runForEach (NOT a switch): lc.states
      // ticks on every rows frame, and an in-flight probe (a full ssh
      // round-trip) must never be interrupted by the next frame.
      const probeBackgroundSources = (onLog: EnrichmentLog) =>
        Stream.merge(
          profiles.state.changes.pipe(Stream.as(undefined)),
          lc.states.changes.pipe(Stream.as(undefined)),
        ).pipe(
          Stream.runForEach(() =>
            Effect.gen(function* () {
              const profileState = yield* SubscriptionRef.get(profiles.state);
              const liveStates = yield* SubscriptionRef.get(lc.states);
              const liveKeys = sourceRunnerKeys(profileState);
              for (const key of attempted) {
                if (!liveKeys.has(key)) {
                  attempted.delete(key);
                }
              }
              for (const candidate of hostnameProbeCandidates(profileState, liveStates, attempted)) {
                // Mark + fork as one uninterruptible step: a re-arm interrupt
                // landing between them would strand the key as attempted with
                // no probe ever launched — skipped for the whole session. The
                // mark still precedes the fork so a replayed tick can't
                // double-probe; only the fork op itself is shielded (the probe
                // fiber lives in the service scope and stays interruptible by
                // scope close alone).
                yield* Effect.uninterruptible(
                  Effect.suspend(() => {
                    attempted.add(candidate.runnerKey);
                    return runProbe(candidate, onLog).pipe(Effect.forkIn(scope));
                  }),
                );
              }
            }).pipe(Effect.catchAllDefect(() => Effect.void)),
          ),
        );

      return {
        // Arm both supervisors with the dock's debug-log sink. Only the dock
        // configures (the settings window stays inert, belt-and-braces on top
        // of its data inertness: its preflight never leaves loading and its
        // live map stays empty). No disarm path: unlike the summon hotkey, no
        // external resource is held, so the supervisors just die with the
        // runtime. A re-configure (StrictMode's double mount) swaps the
        // supervisors under the mutex; in-flight probe fibers live in the
        // service scope and are untouched.
        configure: (onLog: EnrichmentLog) =>
          mutex.withPermits(1)(
            Effect.gen(function* () {
              if (slot !== null) {
                yield* Fiber.interrupt(slot);
                slot = null;
              }
              slot = yield* Effect.all([recordDriverProbes, probeBackgroundSources(onLog)], {
                concurrency: "unbounded",
                discard: true,
              }).pipe(Effect.forkIn(scope));
            }),
          ),
      };
    }),
  },
) {}
