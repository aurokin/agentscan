// The candidate derivation for one-shot background hostname probes, extracted
// pure so its guard set — the exact conditions under which the dock spends an
// ssh round-trip on a label — is unit-testable. The HostnameEnrichment service
// is a thin imperative shell over these (it owns the attempt set, the probe
// fibers, and the persistence).

import { liveStateFor, type LiveStates } from "./LiveConnection";
import {
  folderProfiles,
  getActiveProfile,
  runnerKeyForProfile,
  runnerSettingsForProfile,
  type ProfileState,
} from "./profileModel";
import type { DesktopRunnerSettings } from "./types";

export type HostnameProbeCandidate = {
  readonly profileId: string;
  // The configured connection string, for the debug-log label.
  readonly host: string;
  readonly runnerKey: string;
  readonly settings: DesktopRunnerSettings;
};

// The folder-eligible sources' runner identities. The attempt set is pruned to
// these: a key that left the list (host edit or delete) forgets its attempt, so
// a round-trip edit back to the same connection — which restores the old
// runnerKey with probedHost cleared — re-probes instead of being blocked until
// a relaunch.
export function sourceRunnerKeys(state: ProfileState): Set<string> {
  return new Set(folderProfiles(state).map(runnerKeyForProfile));
}

// Which sources deserve a background hostname probe right now: an ssh source
// that was never probed (no probedHost), is NOT the active source (the
// preflight driver already probes that one), has not been attempted this
// session, and whose channel is ONLINE — proof its SSH path works without
// interactive prompts, so a background probe can't strand a passphrase prompt.
// An absent live key reads as the initial connecting state and is skipped.
export function hostnameProbeCandidates(
  state: ProfileState,
  liveStates: LiveStates,
  attempted: ReadonlySet<string>,
): HostnameProbeCandidate[] {
  const activeKey = runnerKeyForProfile(getActiveProfile(state));
  return folderProfiles(state).flatMap((profile) => {
    if (profile.kind !== "ssh" || profile.probedHost) {
      return [];
    }
    const runnerKey = runnerKeyForProfile(profile);
    if (
      runnerKey === activeKey ||
      attempted.has(runnerKey) ||
      liveStateFor(liveStates, runnerKey).connection.status !== "online"
    ) {
      return [];
    }
    return [
      {
        profileId: profile.id,
        host: profile.host,
        runnerKey,
        settings: runnerSettingsForProfile(profile),
      },
    ];
  });
}
