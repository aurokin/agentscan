// Native window plumbing for the dock: the serialized operation queues, the
// summon/placement helpers, and the window sizing constants their callers
// apply. Each webview realm gets its own module instance (matching the old
// App.tsx module-level queues), and only dock-gated effects use it. Dev note:
// since the move out of App.tsx, an App hot update no longer resets these
// queues — only editing this module does.

import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

// Window min-size floors, applied at runtime per orientation. The vertical pair
// mirrors the startup floor in tauri.{macos.,}conf.json; horizontal drops the
// height floor so the bar can shrink to dock height instead of a tall slab.
export const WINDOW_MIN_WIDTH = 220;
export const WINDOW_MIN_HEIGHT_VERTICAL = 520;
// Auto-mode floor when the window is wider than tall: lets a freely-dragged window get
// short without collapsing the chip strip. A PINNED horizontal bar ignores this and locks
// to BAR_WINDOW_HEIGHT (below) instead, so its height isn't resizable at all.
export const WINDOW_MIN_HEIGHT_HORIZONTAL = 44;
// Locked height for the pinned horizontal bar: min == max == this, so the bar resizes only
// horizontally (the layout is tuned for this exact height). Mirrors BAR_WINDOW_HEIGHT in
// src-tauri/src/lib.rs (the snap height place_bar_window applies) — keep the two in sync.
export const BAR_WINDOW_HEIGHT = 56;
// Max-size caps per pinned orientation: vertical stays a strip (width capped, height free);
// the pinned horizontal bar locks height at BAR_WINDOW_HEIGHT (above) with free width.
// "auto" clears the cap. The free axis uses a value larger than any display.
export const WINDOW_MAX_WIDTH_VERTICAL = 520;
export const WINDOW_MAX_UNBOUNDED = 10000;
// Corner radius (logical px) for frameless mode, matching the macOS window rounding the
// native frame would otherwise draw. Mirrors --frameless-radius in styles.css and is passed
// to the native glass backdrop so the vibrancy view rounds to the same curve as the webview.
export const FRAMELESS_CORNER_RADIUS = 12;

let windowOperationQueue = Promise.resolve();
// Serializes set_window_glass invokes so a fast off→on toggle can't land its
// native calls out of order and leave the blur layer out of sync with the UI.
let glassOperationQueue = Promise.resolve();
// Same discipline for the frameless decorations toggle: serialize the native
// set_window_decorations calls so a fast toggle settles on the latest desired state.
let framelessOperationQueue = Promise.resolve();

export function enqueueWindowOperation(operation: () => Promise<void>) {
  windowOperationQueue = windowOperationQueue.then(operation, operation);
  return windowOperationQueue;
}

// The glass/frameless chains have no rejection handler (unlike the window queue
// above, which self-heals): queued operations must not reject, or every later
// operation on that chain is skipped. Today's callers wrap their entire bodies
// in try/catch, keeping that invariant.
export function enqueueGlassOperation(operation: () => Promise<void>): void {
  glassOperationQueue = glassOperationQueue.then(operation);
}

export function enqueueFramelessOperation(operation: () => Promise<void>): void {
  framelessOperationQueue = framelessOperationQueue.then(operation);
}

// Persistent-window model: the global hotkey raises/focuses the window; it
// never toggles it away. The caller passes the placement for the live orientation
// so summoning a pinned/auto horizontal bar re-docks it as a bar, not a strip.
export async function raisePickerWindow(place: () => Promise<void> = placePickerWindow) {
  await enqueueWindowOperation(async () => {
    const appWindow = getCurrentWindow();
    // Restore first: macOS show() does NOT un-minimize a minimized window, so summoning a
    // dock the user minimized (now reachable via the frameless minimize button) would
    // silently no-op without this. Mirrors the unminimize-before-show order of App.tsx's
    // openSettings.
    await appWindow.unminimize();
    await place();
    await appWindow.show();
    await appWindow.setFocus();
  });
}

export async function placePickerWindow() {
  try {
    await invoke("place_picker_window");
  } catch {
    // Placement is best-effort; showing and focusing the picker is more important.
  }
}

export async function placeBarWindow() {
  try {
    await invoke("place_bar_window");
  } catch {
    // Placement is best-effort; the layout still follows the pinned orientation.
  }
}
