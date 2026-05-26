// Provider brand logos rendered in the picker. Bundled locally (not fetched)
// so the desktop works offline. Keys match the backend's serde provider names
// (snake_case) carried on PickerRow.provider.
import antigravity from "./assets/providers/antigravity.svg";
import claude from "./assets/providers/claude.svg";
import codex from "./assets/providers/codex.svg";
import copilot from "./assets/providers/copilot.svg";
import cursorCli from "./assets/providers/cursor_cli.svg";
import droid from "./assets/providers/droid.svg";
import gemini from "./assets/providers/gemini.svg";
import grok from "./assets/providers/grok.svg";
import hermes from "./assets/providers/hermes.svg";
import opencode from "./assets/providers/opencode.svg";
import pi from "./assets/providers/pi.svg";

const PROVIDER_LOGOS: Record<string, string> = {
  antigravity,
  claude,
  codex,
  copilot,
  cursor_cli: cursorCli,
  droid,
  gemini,
  grok,
  hermes,
  opencode,
  pi,
};

// Returns the bundled logo URL for a provider, or undefined for an
// unknown/unclassified pane (caller renders no logo).
export function providerLogo(provider: string | null | undefined): string | undefined {
  if (!provider) {
    return undefined;
  }
  return PROVIDER_LOGOS[provider];
}
