// Native window plumbing for the dock: the serialized operation queues, the
// summon/placement helpers, the queued-op bodies for the three chrome applies
// (shape/glass/frameless, with injected sinks so node-env tests can record the
// native/DOM interleaving), and the window sizing constants. Each webview
// realm gets its own module instance (matching the old App.tsx module-level
// queues), and only dock-gated effects use it. Dev notes: since the move out
// of App.tsx, an App hot update no longer resets these queues — only editing
// this module does. Node test suites import this module directly, which pulls
// in @tauri-apps/api; the pinned version is import-safe outside a Tauri host
// (no module-scope window access) — an api bump that breaks this breaks every
// node-env suite.

import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import type { WindowSizePlan } from "./effect/windowChromeModel";

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

// The subset of the Tauri window handle the shape apply touches; injectable so
// tests can record the call order without a Tauri host.
type WindowShapeHandle = {
  setMinSize(size: LogicalSize): Promise<void>;
  setMaxSize(size: LogicalSize | null): Promise<void>;
};

// Queued-op body for the sizing effect: apply one shape plan in a race-free
// sequence. Caps are lifted before min is raised so a larger min can never
// transiently exceed a stale max; then the real cap is applied (null re-unsets
// it in "auto" — the unconditional third call is deliberate) and we snap to
// the canonical strip/bar. The placement runs in THIS operation, after the
// matching min-size is applied, so a bar can actually shrink to its short
// height instead of fighting the tall startup min (a separate, earlier-queued
// placement would race and leave a horizontal layout in a tall window).
// `place` must be an execution-time thunk (the dock passes
// () => summonPlacementRef.current()) so it follows the live orientation at
// op-run time, not effect time. Never rejects (the window queue self-heals,
// but the contract is uniform across the applies); injected sinks must not
// throw outside the try.
export async function applyWindowShape({
  plan,
  place,
  getWindow = getCurrentWindow,
}: {
  plan: WindowSizePlan;
  place: () => Promise<void>;
  getWindow?: () => WindowShapeHandle;
}): Promise<void> {
  try {
    const win = getWindow();
    // Fully unbind first (null is Tauri's unset) so a larger min can't clash
    // with a stale max, then re-apply the real cap below.
    await win.setMaxSize(null);
    await win.setMinSize(new LogicalSize(plan.minSize.width, plan.minSize.height));
    await win.setMaxSize(
      plan.maxSize ? new LogicalSize(plan.maxSize.width, plan.maxSize.height) : null,
    );
    if (plan.shouldPlace) {
      await place();
    }
  } catch {
    // Best-effort: a failed update leaves the prior constraints/shape in place.
  }
}

// Queued-op body for the glass toggle. Order matters so we never flash the
// bare desktop through the transparent window: when enabling, raise the blur
// layer first, then mark the surface translucent; when disabling, go opaque
// first, then drop the blur. `currentClear` is an execution-time thunk (the
// dock passes () => glassClearFor(surfaceAlphaRef.current)) so a slider move
// while the op is queued still lands the fresh value — the surface-alpha
// effect won't correct it because data-glass isn't "on" yet. The catch runs
// even when cancelled (a superseded op's failure still forces opaque; the
// successor op corrects it — queue serialization makes this converge). The
// whole body never rejects (a rejection would silently skip every later op on
// the unhandled glass chain); sinks must not throw outside the try.
export async function applyGlass({
  enabled,
  radius,
  isCancelled,
  currentClear,
  setAttr,
  setClear,
  onError,
  invokeIpc = invoke,
}: {
  enabled: boolean;
  radius: number | null;
  isCancelled: () => boolean;
  currentClear: () => number;
  // Writes <html data-glass>; DOM ownership stays with the caller.
  setAttr: (value: "on" | "off") => void;
  setClear: (clear: number) => void;
  onError: (error: unknown) => void;
  invokeIpc?: (cmd: string, args: Record<string, unknown>) => Promise<unknown>;
}): Promise<void> {
  // A newer toggle superseded this one before it ran; skip the native call
  // entirely so the queue settles on the latest desired state.
  if (isCancelled()) {
    return;
  }
  try {
    if (enabled) {
      await invokeIpc("set_window_glass", { enabled: true, radius });
      if (!isCancelled()) {
        // Flip the surface translucent and arm the adaptive tokens together,
        // so `--glass-clear` is only nonzero once the blur is actually live.
        setAttr("on");
        setClear(currentClear());
      }
    } else {
      setAttr("off");
      setClear(0);
      await invokeIpc("set_window_glass", { enabled: false, radius });
    }
  } catch (error) {
    // Native call failed: keep the surface opaque AND the tokens un-adapted.
    setAttr("off");
    setClear(0);
    onError(error);
  }
}

// Queued-op body for the frameless toggle. The data-frameless attribute is
// what surfaces the custom drag region + window controls in styles.css, so it
// flips only once set_window_decorations resolves — the controls never render
// over a still-framed window, and a failed native call leaves the attribute
// "off" so we don't strip the only chrome without a working replacement. Same
// contracts as applyGlass: catch-despite-cancel, never rejects, sinks must
// not throw outside the try.
export async function applyFrameless({
  enabled,
  isCancelled,
  setAttr,
  setApplied,
  onError,
  invokeIpc = invoke,
}: {
  enabled: boolean;
  isCancelled: () => boolean;
  // Writes <html data-frameless>; DOM ownership stays with the caller.
  setAttr: (value: "on" | "off") => void;
  // The dock's setFramelessApplied state setter: custom chrome tracks the
  // real window state, never the pending preference.
  setApplied: (applied: boolean) => void;
  onError: (error: unknown) => void;
  invokeIpc?: (cmd: string, args: Record<string, unknown>) => Promise<unknown>;
}): Promise<void> {
  if (isCancelled()) {
    return;
  }
  try {
    await invokeIpc("set_window_decorations", { decorations: !enabled });
    if (!isCancelled()) {
      setAttr(enabled ? "on" : "off");
      setApplied(enabled);
    }
  } catch (error) {
    // The native call failed: assume the frame is still present and hide the
    // custom chrome, so we never stack our controls on a native titlebar.
    setAttr("off");
    setApplied(false);
    onError(error);
  }
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
