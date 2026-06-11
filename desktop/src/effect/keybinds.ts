// Picker keyboard handling, extracted from App.tsx so the routing contract —
// the keybind OWNER's rows are the only ones a press resolves against — and
// the full event→intent interpretation are unit-testable alongside
// keybindOwnerId.

import type { PickerRow } from "./types";

// The modifier/key fields the interpreter reads; a DOM KeyboardEvent
// satisfies this structurally.
export type PickerKeyEvent = {
  readonly key: string;
  readonly ctrlKey: boolean;
  readonly metaKey: boolean;
  readonly altKey: boolean;
  readonly shiftKey: boolean;
};

// What a picker keydown means. The dock applies it: "activate" jumps to and
// activates a row, "move" nudges the selection (the clamp against the visible
// rows stays dock-side — on empty rows the original handler preventDefaults
// but leaves a stale selection untouched, which a pre-clamped select-intent
// would clobber), "select" targets a pane directly (null on empty rows means
// clear), "activateSelection" fires the current selection, and "escape"
// carries whether a filter clear is due. The applier must preventDefault for
// every intent EXCEPT escape with clearFilter false, and must run its
// collapse-search side effect on every escape.
export type PickerKeyIntent =
  | { kind: "activate"; row: PickerRow }
  | { kind: "move"; delta: 1 | -1 }
  | { kind: "select"; paneId: string | null }
  | { kind: "activateSelection" }
  | { kind: "escape"; clearFilter: boolean };

// Interpret one keydown over the keybind owner's visible rows. isMac and
// isInteractiveTarget arrive as inputs: the platform flag so the mac branch is
// testable (platform.ts freezes it at import time), and the DOM
// interactive-target verdict because it needs the event's real target.
export function pickerKeyIntent(
  event: PickerKeyEvent,
  context: {
    readonly bootScreenVisible: boolean;
    readonly hasOwner: boolean;
    readonly isInteractiveTarget: boolean;
    readonly isMac: boolean;
    readonly rows: readonly PickerRow[];
    readonly filterActive: boolean;
  },
): PickerKeyIntent | null {
  // The boot/recovery screen replaces the folder list entirely, but the owner's
  // keyed live state can still hold rows behind it — gate every picker key while
  // it shows so Ctrl+<key>/Enter can't activate rows the user cannot see.
  if (context.bootScreenVisible) {
    return null;
  }

  // Control + a row's displayed hotkey jumps straight to that pane. Require
  // Control alone so we never shadow ⌘ shortcuts or Ctrl+⌘ combos. On macOS,
  // editing uses ⌘, so Ctrl is free even inside the search box — bypass the
  // interactive-target gate so you can filter then jump in one motion. On
  // Windows/Linux, Ctrl *is* the editing modifier (Ctrl+C/V/X/Z/F), so only
  // honor the hotkey when no input/button is focused; otherwise native
  // clipboard/find/undo wins. (Key match is character-based to mirror the kbd
  // label and the CLI's configured char hotkeys; non-US layouts that shift
  // digit keys may no-op on the default number row, which is a silent miss
  // rather than a wrong action.)
  // Resolves ONLY against the keybind owner's rows; other folders render their
  // <kbd> labels dimmed, as information. A miss falls THROUGH (never null
  // here): first to the interactive gate, then into the movement branches.
  const ctrlActivate = event.ctrlKey && !event.metaKey && !event.altKey && !event.shiftKey;
  if (ctrlActivate && context.hasOwner && (context.isMac || !context.isInteractiveTarget)) {
    const target = pickerRowForKeyboardKey(context.rows, event.key);
    if (target) {
      return { kind: "activate", row: target };
    }
  }

  if (context.isInteractiveTarget) {
    return null;
  }

  if (event.key === "ArrowDown" || event.key === "j") {
    return { kind: "move", delta: 1 };
  }
  if (event.key === "ArrowUp" || event.key === "k") {
    return { kind: "move", delta: -1 };
  }
  if (event.key === "Home") {
    return { kind: "select", paneId: context.rows[0]?.pane_id ?? null };
  }
  if (event.key === "End") {
    return { kind: "select", paneId: context.rows[context.rows.length - 1]?.pane_id ?? null };
  }
  if (event.key === "Enter") {
    return { kind: "activateSelection" };
  }
  if (event.key === "Escape") {
    // Persistent-window model: Escape never hides the window. It clears the
    // search filter when one is active; otherwise it's a no-op key (no
    // preventDefault) whose collapse-search side effect still runs.
    return { kind: "escape", clearFilter: context.filterActive };
  }

  return null;
}

export function pickerRowForKeyboardKey(
  rows: readonly PickerRow[],
  key: string,
): PickerRow | undefined {
  const normalizedKey = normalizePickerKeyboardKey(key);
  if (normalizedKey === null) {
    return undefined;
  }

  // Match the key returned by `agentscan hotkeys --format json`; this keeps
  // desktop activation tied to the user's configured picker_keys, not the
  // built-in default order.
  return rows.find((row) => normalizePickerKeyboardKey(row.key) === normalizedKey);
}

export function normalizePickerKeyboardKey(key: string): string | null {
  if (key.length !== 1) {
    return null;
  }

  const normalizedKey = key.toUpperCase();
  return /^[A-Z0-9]$/.test(normalizedKey) ? normalizedKey : null;
}
