// The only app-shell module deliberately shared by DockApp and SettingsApp.
// Hard rule: DockApp and SettingsApp never import from each other — anything
// both windows need lives here, in src/components/, or in src/effect/. And
// effect/atoms.ts must stay the single runtime module both apps import: the
// settings window's service instances (LiveConnection, Preflight's prober,
// SummonHotkey, Activation) are inert only because both windows build the one
// merged layer whose construction is side-effect-free until a dock-only
// configure path drives it; per-app runtime modules would break that argument.

export function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

// Synchronous, best-effort localStorage read used only to seed the first paint
// (active profile / runnerKey / drafts / appearance) before the service atoms
// resolve. All WRITES and ongoing reads go through the services; this just
// matches their initial seed so the first render isn't a flash of default state.
export const readLocalStorage = (key: string): string | null => {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
};
