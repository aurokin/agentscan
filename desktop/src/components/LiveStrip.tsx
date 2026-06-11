import type { ConnectionStatus } from "../effect/types";
import { liveStateLabel } from "../effect/pickerViewModel";

// The banner shown for any non-online connection. `noDaemon` (the dock latched but
// found no daemon to attach to) offers Start agentscan; `fatal` offers both Start
// agentscan and a latch-only Reconnect (a Start refusal lands here, and Start is the
// action that actually retries it) — so the dock never wedges with a dead stream and
// no way out. connecting/reconnecting are transient and self-heal, so they show
// progress only.
export function LiveStrip({
  status,
  onStart,
  onReconnect,
}: {
  status: ConnectionStatus;
  onStart: () => void;
  onReconnect: () => void;
}) {
  const tone = status.status === "fatal" ? "error" : "warn";

  return (
    <div className={`live-strip ${tone}`} aria-live="polite">
      <span className="status-dot" data-tone={tone === "error" ? "error" : "busy"} />
      <span className="live-label">{liveStateLabel(status)}</span>
      <span className="live-message">{status.message}</span>
      {status.status === "noDaemon" ? (
        <button className="live-action" type="button" onClick={onStart}>
          Start agentscan
        </button>
      ) : status.status === "fatal" ? (
        // A fatal includes an explicit-Start refusal (e.g. macOS codesign/trust), whose
        // actual fix is to retry the start once resolved. Reconnect is latch-only and
        // can't spawn, so it would force a no-daemon round-trip before Start reappears.
        // Offer Start agentscan (start-or-latch — strictly more capable, recovers every
        // fatal cause the user fixes) alongside the latch-only Reconnect.
        <div className="live-actions">
          <button className="live-action" type="button" onClick={onStart}>
            Start agentscan
          </button>
          <button className="live-action" type="button" onClick={onReconnect}>
            Reconnect
          </button>
        </div>
      ) : null}
    </div>
  );
}
