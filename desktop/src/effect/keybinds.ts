// Ctrl+<key> row-hotkey resolution, extracted from App.tsx so the routing
// contract — the keybind OWNER's rows are the only ones a press resolves
// against — is unit-testable alongside keybindOwnerId.

import type { PickerRow } from "./types";

export function pickerRowForKeyboardKey(rows: PickerRow[], key: string): PickerRow | undefined {
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
