import { getCurrentWindow } from "@tauri-apps/api/window";

// Custom minimize/close window controls for frameless mode. Callers gate these
// on framelessApplied so they only appear once the native frame is actually
// gone. Close hides the window (dismiss) rather than destroying it, matching
// Escape and the summonable-dock model.
export function WindowControls() {
  return (
    <>
      <button
        className="icon-button"
        type="button"
        aria-label="Minimize window"
        title="Minimize"
        onClick={() => void getCurrentWindow().minimize()}
      >
        {"–"}
      </button>
      <button
        className="icon-button window-close"
        type="button"
        aria-label="Close window"
        title="Close"
        onClick={() => void getCurrentWindow().hide()}
      >
        {"×"}
      </button>
    </>
  );
}
