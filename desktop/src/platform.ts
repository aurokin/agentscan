// Per-row picker hotkeys are triggered with Control rather than Command. The
// default key set overlaps macOS ⌘ shortcuts — ⌘Q quits, ⌘C/V/X are clipboard,
// ⌘F/Z/R are find/undo/refresh — so ⌘ would be hostile. Control has no such
// collisions (only emacs text-nav in inputs, which we override on a match).
export const IS_MAC =
  typeof navigator !== "undefined" && /Mac|iP(hone|ad|od)/.test(navigator.platform);
// The trailing space on the non-mac value is load-bearing: GroupedPicker renders
// it raw inside <kbd> as the key's prefix ("Ctrl A"), while standalone badge
// sites call .trim() on it.
export const HOTKEY_MODIFIER_LABEL = IS_MAC ? "⌃" : "Ctrl ";
