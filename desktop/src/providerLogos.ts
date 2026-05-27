// Provider brand logos rendered in the picker. Bundled locally (not fetched)
// so the desktop works offline. Keys match the backend's serde provider names
// (snake_case) carried on PickerRow.provider.
//
// Most marks are monochrome and vanish in one theme, so they ship per-theme
// variants: `<name>-light.svg` is the dark/colored mark for the light background,
// `<name>-dark.svg` is the light/colored mark for the dark background. Full-color
// marks (gemini, antigravity, claude) read on both and stay a single file.
import antigravity from "./assets/providers/antigravity.svg";
import claude from "./assets/providers/claude.svg";
import gemini from "./assets/providers/gemini.svg";

import codexLight from "./assets/providers/codex-light.svg";
import codexDark from "./assets/providers/codex-dark.svg";
import copilotLight from "./assets/providers/copilot-light.svg";
import copilotDark from "./assets/providers/copilot-dark.svg";
import cursorCliLight from "./assets/providers/cursor_cli-light.svg";
import cursorCliDark from "./assets/providers/cursor_cli-dark.svg";
import droidLight from "./assets/providers/droid-light.svg";
import droidDark from "./assets/providers/droid-dark.svg";
import grokLight from "./assets/providers/grok-light.svg";
import grokDark from "./assets/providers/grok-dark.svg";
import hermesLight from "./assets/providers/hermes-light.svg";
import hermesDark from "./assets/providers/hermes-dark.svg";
import opencodeLight from "./assets/providers/opencode-light.svg";
import opencodeDark from "./assets/providers/opencode-dark.svg";
import piLight from "./assets/providers/pi-light.svg";
import piDark from "./assets/providers/pi-dark.svg";

export type LogoTheme = "light" | "dark";

// A logo is either a single asset (renders on both themes) or a per-theme pair.
type LogoEntry = string | { light: string; dark: string };

const PROVIDER_LOGOS: Record<string, LogoEntry> = {
  antigravity,
  claude,
  gemini,
  codex: { light: codexLight, dark: codexDark },
  copilot: { light: copilotLight, dark: copilotDark },
  cursor_cli: { light: cursorCliLight, dark: cursorCliDark },
  droid: { light: droidLight, dark: droidDark },
  grok: { light: grokLight, dark: grokDark },
  hermes: { light: hermesLight, dark: hermesDark },
  opencode: { light: opencodeLight, dark: opencodeDark },
  pi: { light: piLight, dark: piDark },
};

// Returns the bundled logo URL for a provider in the given theme, or undefined
// for an unknown/unclassified pane (caller renders no logo).
export function providerLogo(
  provider: string | null | undefined,
  theme: LogoTheme,
): string | undefined {
  if (!provider) {
    return undefined;
  }
  const entry = PROVIDER_LOGOS[provider];
  if (entry === undefined) {
    return undefined;
  }
  return typeof entry === "string" ? entry : entry[theme];
}
