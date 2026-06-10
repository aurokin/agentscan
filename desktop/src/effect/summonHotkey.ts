// Summon-hotkey registration failure messaging, extracted from App.tsx so the
// in-use detection and wording are unit-testable.

// The OS grants a global hotkey to one process. macOS reports a combo held by
// another process as a RegisterEventHotKey failure; the plugin reports a combo
// it already holds in this process as "already registered". Both mean "someone
// owns the key", and in practice that someone is usually a second agentscan
// instance (e.g. a dev build alongside the installed app).
const HOTKEY_IN_USE_PATTERN = /RegisterEventHotKey failed|already registered/i;

// Mac-first display form of PICKER_HOTKEY (CommandOrControl+Shift+A).
const SUMMON_HOTKEY_LABEL = "⌘⇧A";

export function summonHotkeyFailureMessage(error: unknown): string {
  const detail = error instanceof Error ? error.message : String(error);
  if (HOTKEY_IN_USE_PATTERN.test(detail)) {
    return `${SUMMON_HOTKEY_LABEL} is in use — another agentscan instance may be running. Retrying until it frees up.`;
  }
  return `Unable to register ${SUMMON_HOTKEY_LABEL}: ${detail}`;
}
