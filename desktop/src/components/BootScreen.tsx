import { WindowControls } from "./WindowControls";
import type { Orientation } from "../effect/prefs";

// The dock's full-screen boot/recovery takeover: still probing, the probe
// itself failed (IPC error), or the CLI is unavailable for the current runner.
// Whether it shows at all (dockBootScreenVisible) and what it says
// (dockBootScreenContent) stay derived in the dock; this renders the result
// and the recovery affordances.
export function BootScreen({
  probing,
  detail,
  suggestedBinaryPath,
  orientation,
  dragRegion,
  framelessApplied,
  onApplySuggestedBinaryPath,
  onOpenSettings,
}: {
  probing: boolean;
  detail: string;
  suggestedBinaryPath: string | null;
  orientation: Orientation;
  dragRegion: string | undefined;
  framelessApplied: boolean;
  onApplySuggestedBinaryPath: (path: string) => void;
  onOpenSettings: () => void;
}) {
  return (
    // Recovery UI renders in the live orientation: a centered column in the vertical
    // strip, and a compact row in the horizontal bar (styles.css) so the heading and
    // the only "Open settings" path stay visible without clipping in the short bar.
    <main className="sidebar" data-orientation={orientation} data-tauri-drag-region={dragRegion}>
      {/* The drag region must sit on boot-state too, not just <main>: boot-state fills the
          window (height:100% / flex:1), so Tauri — which starts a drag only when the click
          target itself carries the attribute — would otherwise see every click land on this
          covering child and never drag. Clicks on the spinner/copy/button target those
          elements (no attribute), so they stay non-draggable. */}
      <div className="boot-state" aria-live="polite" data-tauri-drag-region={dragRegion}>
        <span className="boot-spinner" aria-hidden="true" />
        <div className="boot-copy">
          <h1>{probing ? "Connecting" : "Can’t reach agentscan"}</h1>
          <p>{detail}</p>
        </div>
        {/* Always offer a path into settings: a hung "loading" (e.g. a stalled
            profile/SSH preflight) or a CLI-unavailable runner otherwise traps the
            user with no way to fix the binary path or host. When the remote probe
            resolved an absolute path, also offer to apply it in one click. */}
        <div className="boot-actions">
          {suggestedBinaryPath ? (
            <button type="button" onClick={() => onApplySuggestedBinaryPath(suggestedBinaryPath)}>
              Use this path
            </button>
          ) : null}
          <button type="button" onClick={onOpenSettings}>
            Open settings
          </button>
        </div>
      </div>
      {/* Frameless mode strips the native frame, so the recovery screen would otherwise
          be a borderless window the user can't move/minimize/dismiss while connecting or
          after a failure. The boot screen has no footer, so float the controls instead. */}
      {framelessApplied ? (
        <div className="boot-window-controls">
          <WindowControls />
        </div>
      ) : null}
    </main>
  );
}
