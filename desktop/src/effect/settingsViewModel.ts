// The settings window's presentation derivations. Deliberately separate from
// preflightViewModel: that module is the DOCK's presentation, pure over its
// PreflightState, while this card derives from SyncedPreflight — the dock's
// resolved preflight mirrored over the prefs channel — with its own runnerKey
// guard. Keeping them apart also keeps dock-presentation modules out of the
// settings window's import graph.

import type { SyncedPreflight } from "./Preflight";

// The agentscan status card. The settings window never runs its own preflight
// (which for SSH would be a duplicate `ssh … --version`); it reuses the dock's
// mirror. That result is only trusted when its runnerKey matches this window's
// active source; otherwise it describes the previous one mid-switch and reads
// as "Checking" until the dock re-probes and pushes the matching one (or a
// focus-time replay request refreshes it). A failed dock status (IPC error)
// reads as "Unreachable".
export function settingsPreflightCard(
  synced: SyncedPreflight | null,
  runnerKey: string,
): { tone: "unknown" | "idle" | "error"; label: string; detail: string } {
  const syncMatches = synced !== null && synced.runnerKey === runnerKey;
  const preflight = syncMatches ? synced.preflight : null;
  const syncFailed = syncMatches && synced.status === "failed";
  return {
    tone: !preflight ? (syncFailed ? "error" : "unknown") : preflight.ok ? "idle" : "error",
    label: !preflight
      ? syncFailed
        ? "Unreachable"
        : "Checking"
      : preflight.ok
        ? "Ready"
        : "Unavailable",
    detail: !preflight
      ? syncFailed
        ? "Can’t reach agentscan"
        : "Probing agentscan…"
      : preflight.ok
        ? `${preflight.binary} · ${preflight.version ?? "ready"}`
        : (preflight.error ?? "agentscan unavailable"),
  };
}
