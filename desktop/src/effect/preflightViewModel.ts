// The dock's preflight presentation rules, extracted from App.tsx so the one
// staleness rule they all share — and the boot-screen predicate that can blank
// the entire dock — are unit-testable. Everything here is pure over
// PreflightState; the settings card is deliberately NOT routed through this
// module (it derives from SyncedPreflight, a different wire shape with its own
// runnerKey guard).
//
// The asymmetry to know about: a "ready" state names the runner it probed
// (runnerKey), but "failed" (the probe IPC itself broke) and "loading" carry no
// runnerKey — they are always treated as describing the ACTIVE runner. That is
// why matchedPreflight returns null for them while statusText/
// activePreflightError still surface them.

import type { PreflightState } from "./Preflight";

// THE staleness rule. `preflightState` lags the active runner by one async
// cycle after a switch or settings apply (the service resolves the new probe
// asynchronously). A resolved state describes the CURRENT runner only when its
// runnerKey matches; until then it belongs to the previous target and must not
// drive live decisions. Every dock-side consumer of a resolved preflight goes
// through this guard.
export function matchedPreflight(
  state: PreflightState,
  runnerKey: string,
): Extract<PreflightState, { status: "ready" }> | null {
  return state.status === "ready" && state.runnerKey === runnerKey ? state : null;
}

// Footer status line for the active source. Preflight from a not-yet-refreshed
// previous profile is untrustworthy, so report "Checking" until the resolved
// state matches the active profile.
export function preflightStatusText(
  state: PreflightState,
  runnerKey: string,
  kindLabel: string,
): string {
  if (state.status === "failed") {
    return "IPC failed";
  }
  const matched = matchedPreflight(state, runnerKey);
  if (matched === null) {
    return `Checking ${kindLabel} CLI`;
  }
  return matched.preflight.ok ? `${kindLabel} CLI ready` : `${kindLabel} CLI unavailable`;
}

// Tone for the footer status dot when the footer shows the active source,
// derived from its resolved preflight (not a stale previous one — the
// switching window reads as unknown).
export function preflightSourceTone(
  state: PreflightState,
  runnerKey: string,
): "unknown" | "idle" | "error" {
  const matched = matchedPreflight(state, runnerKey);
  if (matched === null) {
    return "unknown";
  }
  return matched.preflight.ok ? "idle" : "error";
}

// The active runner's preflight is unusable when the probe resolved for the
// CURRENT runner but reports the CLI unavailable (bad binary path / SSH
// target). A stale ready state from a profile still switching (runnerKey
// mismatch) is not an error — the folders keep rendering their keyed live
// states while the new probe resolves.
export function preflightUnusable(state: PreflightState, runnerKey: string): boolean {
  const matched = matchedPreflight(state, runnerKey);
  return matched !== null && !matched.preflight.ok;
}

// The active source's failure surfaced inside its own folder (or the homeless
// strip): its live target is gated off (or left disarmed) on a failed probe,
// so without this the folder's keyed state would read as a dishonest perpetual
// "Waiting for a source". Covers a resolved-but-unusable probe, and the probe
// itself failing ("failed" carries no runnerKey; like the boot screen, we
// treat it as the active runner's) — unless the channel is already online, per
// the probes-gate-starting invariant above liveTargets.
export function activePreflightError(
  state: PreflightState,
  runnerKey: string,
  activeLiveOnline: boolean,
  kindLabel: string,
): string | null {
  if (activeLiveOnline) {
    return null;
  }
  const matched = matchedPreflight(state, runnerKey);
  if (matched !== null && !matched.preflight.ok) {
    return matched.preflight.error ?? `${kindLabel} CLI unavailable`;
  }
  return state.status === "failed" ? state.message : null;
}

// The full-screen boot/recovery takeover. Scoped to the states where no OTHER
// open folder could render anyway: preflight is single-source (only the active
// profile is probed), so blanking the whole dock for the active source's
// boot/failure would hide healthy open folders — exactly the independence the
// folder model exists for. With another folder open, the active source's
// failure stays inside its own folder (status dot + error strip) instead.
//
// It also requires the active source to be PARTICIPATING (its folder open): a
// closed folder is header-only with no subscription, so its loading/failing
// preflight must not take over a dock the user deliberately quieted — that
// would hide the folder list (the only way to reopen anything). A homeless
// active source (no folder) surfaces through the error strip instead. And no
// probe verdict blanks the dock over an online channel (probes gate starting;
// the stream is ground truth while it runs).
export function dockBootScreenVisible(
  state: PreflightState,
  runnerKey: string,
  input: {
    readonly isDock: boolean;
    readonly activeLiveOnline: boolean;
    readonly activeFolderOpen: boolean;
    readonly hasOpenFolderBeyondActive: boolean;
  },
): boolean {
  return (
    input.isDock &&
    !input.activeLiveOnline &&
    (state.status !== "ready" || preflightUnusable(state, runnerKey)) &&
    input.activeFolderOpen &&
    !input.hasOpenFolderBeyondActive
  );
}

// Boot/error screen copy: still probing, the probe itself failed (IPC error),
// or the CLI is unavailable for the current runner. `suggestedBinaryPath`: a
// remote not-found preflight may carry the path the user's shell resolves,
// letting the screen offer a one-click fix instead of only routing to
// settings.
export function dockBootScreenContent(
  state: PreflightState,
  kindLabel: string,
): { probing: boolean; detail: string; suggestedBinaryPath: string | null } {
  return {
    probing: state.status === "loading",
    detail:
      state.status === "failed"
        ? state.message
        : state.status === "ready"
          ? (state.preflight.error ?? `${kindLabel} CLI unavailable`)
          : "Waiting for the daemon…",
    suggestedBinaryPath:
      state.status === "ready" && !state.preflight.ok
        ? state.preflight.suggestedBinaryPath
        : null,
  };
}
