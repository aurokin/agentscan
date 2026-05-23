use std::fmt;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use super::IconMode;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Provider {
    Codex,
    Claude,
    Gemini,
    #[value(alias = "agy", alias = "google-antigravity")]
    Antigravity,
    Opencode,
    #[value(alias = "github-copilot")]
    Copilot,
    #[value(name = "cursor_cli", alias = "cursor-cli", alias = "cursor-agent")]
    CursorCli,
    #[value(alias = "pi-coding-agent")]
    Pi,
    Grok,
    #[value(alias = "hermes-agent")]
    Hermes,
    #[value(alias = "factory-droid")]
    Droid,
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(provider_info(*self).canonical_name)
    }
}

pub(crate) struct ProviderInfo {
    provider: Provider,
    canonical_name: &'static str,
    icons: ProviderIconSet,
    metadata_aliases: &'static [&'static str],
    command_aliases: &'static [ProviderCommandAlias],
    title_prefixes: &'static [&'static str],
    title_aliases: &'static [&'static str],
    generic_display_labels: &'static [&'static str],
}

#[derive(Serialize)]
pub(crate) struct ProviderSummary {
    pub(crate) provider: Provider,
    pub(crate) name: &'static str,
    pub(crate) display_marker: &'static str,
    pub(crate) display_marker_codepoints: Vec<String>,
    pub(crate) icons: ProviderIconSummary,
    pub(crate) metadata_aliases: &'static [&'static str],
    pub(crate) command_aliases: Vec<ProviderCommandAliasSummary>,
    pub(crate) title_prefixes: &'static [&'static str],
    pub(crate) title_aliases: &'static [&'static str],
}

#[derive(Clone, Copy)]
pub(crate) struct ProviderIconSet {
    emoji: &'static str,
    nerd_font: &'static str,
    nerd_font_patched: &'static str,
}

#[derive(Serialize)]
pub(crate) struct ProviderIconSummary {
    pub(crate) emoji: ProviderIconModeSummary,
    pub(crate) nerd_font: ProviderIconModeSummary,
    pub(crate) nerd_font_patched: ProviderIconModeSummary,
}

#[derive(Serialize)]
pub(crate) struct ProviderIconModeSummary {
    pub(crate) marker: &'static str,
    pub(crate) codepoints: Vec<String>,
}

#[derive(Clone, Copy)]
pub(crate) struct ProviderCommandAlias {
    name: &'static str,
    allow_suffix: bool,
}

#[derive(Serialize)]
pub(crate) struct ProviderCommandAliasSummary {
    pub(crate) name: &'static str,
    pub(crate) allow_suffix: bool,
}

impl ProviderCommandAlias {
    const fn new(name: &'static str, allow_suffix: bool) -> Self {
        Self { name, allow_suffix }
    }
}

impl ProviderIconSet {
    const fn new(
        emoji: &'static str,
        nerd_font: &'static str,
        nerd_font_patched: &'static str,
    ) -> Self {
        Self {
            emoji,
            nerd_font,
            nerd_font_patched,
        }
    }

    fn marker(self, mode: IconMode) -> &'static str {
        match mode {
            IconMode::Emoji => self.emoji,
            IconMode::NerdFont => self.nerd_font,
            IconMode::NerdFontPatched => self.nerd_font_patched,
        }
    }
}

const PROVIDER_INFOS: &[ProviderInfo] = &[
    ProviderInfo {
        provider: Provider::Codex,
        canonical_name: "codex",
        icons: ProviderIconSet::new("🤖", "\u{f07b5}", "\u{f07b5}"),
        metadata_aliases: &["codex"],
        command_aliases: &[ProviderCommandAlias::new("codex", true)],
        title_prefixes: &[],
        title_aliases: &[],
        generic_display_labels: &[],
    },
    ProviderInfo {
        provider: Provider::Claude,
        canonical_name: "claude",
        icons: ProviderIconSet::new("🧠", "\u{e76f}", "\u{e76f}"),
        metadata_aliases: &["claude"],
        command_aliases: &[ProviderCommandAlias::new("claude", true)],
        title_prefixes: &["Claude Code | ", "Claude | "],
        title_aliases: &["Claude Code"],
        generic_display_labels: &[],
    },
    ProviderInfo {
        provider: Provider::Gemini,
        canonical_name: "gemini",
        icons: ProviderIconSet::new("✨", "\u{e7f0}", "\u{e7f0}"),
        metadata_aliases: &["gemini"],
        command_aliases: &[ProviderCommandAlias::new("gemini", true)],
        title_prefixes: &[],
        title_aliases: &[],
        generic_display_labels: &[],
    },
    ProviderInfo {
        provider: Provider::Antigravity,
        canonical_name: "antigravity",
        icons: ProviderIconSet::new("🛸", "A", "A"),
        metadata_aliases: &["antigravity", "agy", "google antigravity"],
        command_aliases: &[ProviderCommandAlias::new("agy", false)],
        title_prefixes: &[],
        title_aliases: &[],
        generic_display_labels: &["Antigravity", "Antigravity CLI", "agy"],
    },
    ProviderInfo {
        provider: Provider::Opencode,
        canonical_name: "opencode",
        icons: ProviderIconSet::new("🧰", "\u{f07e2}", "\u{f07e2}"),
        metadata_aliases: &["opencode"],
        command_aliases: &[ProviderCommandAlias::new("opencode", true)],
        title_prefixes: &["OC | "],
        title_aliases: &["OpenCode"],
        generic_display_labels: &["OpenCode"],
    },
    ProviderInfo {
        provider: Provider::Copilot,
        canonical_name: "copilot",
        icons: ProviderIconSet::new("🧭", "\u{ec1e}", "\u{ec1e}"),
        metadata_aliases: &["copilot", "github-copilot", "github copilot"],
        command_aliases: &[
            ProviderCommandAlias::new("copilot", false),
            ProviderCommandAlias::new("github-copilot", false),
        ],
        title_prefixes: &["GitHub Copilot | ", "Copilot | "],
        title_aliases: &["GitHub Copilot"],
        generic_display_labels: &["GitHub Copilot"],
    },
    ProviderInfo {
        provider: Provider::CursorCli,
        canonical_name: "cursor_cli",
        icons: ProviderIconSet::new("🖱️", "\u{f12e9}", "\u{f12e9}"),
        metadata_aliases: &["cursor_cli", "cursor-cli", "cursor cli", "cursor-agent"],
        command_aliases: &[
            ProviderCommandAlias::new("cursor-cli", false),
            ProviderCommandAlias::new("cursor-agent", false),
        ],
        title_prefixes: &["Cursor CLI | ", "Cursor Agent | ", "Cursor | "],
        title_aliases: &["Cursor Agent", "Cursor CLI", "Cursor"],
        generic_display_labels: &["Cursor Agent", "cursor-agent", "Cursor CLI", "Cursor"],
    },
    ProviderInfo {
        provider: Provider::Pi,
        canonical_name: "pi",
        icons: ProviderIconSet::new("🥧", "\u{e22c}", "\u{e22c}"),
        metadata_aliases: &["pi", "pi-coding-agent", "pi coding agent"],
        command_aliases: &[ProviderCommandAlias::new("pi-coding-agent", false)],
        title_prefixes: &["π - ", "pi - "],
        title_aliases: &[],
        generic_display_labels: &[],
    },
    ProviderInfo {
        provider: Provider::Grok,
        canonical_name: "grok",
        icons: ProviderIconSet::new("💡", "G", "G"),
        metadata_aliases: &["grok", "grok-build", "grok build"],
        command_aliases: &[ProviderCommandAlias::new("grok", true)],
        title_prefixes: &[],
        title_aliases: &[],
        generic_display_labels: &["grok"],
    },
    ProviderInfo {
        provider: Provider::Hermes,
        canonical_name: "hermes",
        icons: ProviderIconSet::new("⚕️", "⚕", "⚕"),
        metadata_aliases: &["hermes", "hermes-agent", "hermes agent"],
        command_aliases: &[
            ProviderCommandAlias::new("hermes", false),
            ProviderCommandAlias::new("hermes-agent", false),
        ],
        title_prefixes: &[],
        title_aliases: &[],
        generic_display_labels: &["Hermes", "Hermes Agent", "hermes", "hermes-agent"],
    },
    ProviderInfo {
        provider: Provider::Droid,
        canonical_name: "droid",
        icons: ProviderIconSet::new("🏭", "⛬", "⛬"),
        metadata_aliases: &["droid", "factory-droid", "factory droid"],
        command_aliases: &[ProviderCommandAlias::new("droid", false)],
        title_prefixes: &["⛬ "],
        title_aliases: &[],
        generic_display_labels: &["Droid", "New Session"],
    },
];

pub(crate) fn provider_display_marker(provider: Option<Provider>, icon_mode: IconMode) -> String {
    provider
        .map(|provider| provider_info(provider).icons.marker(icon_mode).to_string())
        .unwrap_or_else(|| "?".to_string())
}

pub(crate) fn provider_summary_order() -> impl Iterator<Item = Provider> {
    PROVIDER_INFOS.iter().map(|info| info.provider)
}

pub(crate) fn provider_summaries() -> Vec<ProviderSummary> {
    PROVIDER_INFOS
        .iter()
        .map(|info| ProviderSummary {
            provider: info.provider,
            name: info.canonical_name,
            display_marker: info.icons.nerd_font,
            display_marker_codepoints: marker_codepoints(info.icons.nerd_font),
            icons: icon_summary(info.icons),
            metadata_aliases: info.metadata_aliases,
            command_aliases: info
                .command_aliases
                .iter()
                .map(|alias| ProviderCommandAliasSummary {
                    name: alias.name,
                    allow_suffix: alias.allow_suffix,
                })
                .collect(),
            title_prefixes: info.title_prefixes,
            title_aliases: info.title_aliases,
        })
        .collect()
}

pub(crate) fn provider_marker(provider: Provider, icon_mode: IconMode) -> &'static str {
    provider_info(provider).icons.marker(icon_mode)
}

pub(crate) fn provider_title_prefixes(provider: Provider) -> &'static [&'static str] {
    provider_info(provider).title_prefixes
}

pub(crate) fn provider_title_aliases(provider: Provider) -> &'static [&'static str] {
    provider_info(provider).title_aliases
}

pub(crate) fn provider_generic_display_labels(provider: Provider) -> &'static [&'static str] {
    provider_info(provider).generic_display_labels
}

pub(crate) fn provider_from_metadata(provider: Option<&str>) -> Option<Provider> {
    let normalized = provider?.trim().to_ascii_lowercase();
    PROVIDER_INFOS
        .iter()
        .find(|info| info.metadata_aliases.contains(&normalized.as_str()))
        .map(|info| info.provider)
}

pub(crate) fn provider_from_command(command: &str) -> Option<(Provider, bool)> {
    PROVIDER_INFOS.iter().find_map(|info| {
        info.command_aliases.iter().find_map(|alias| {
            matches_binary(command, alias.name, alias.allow_suffix)
                .map(|exact| (info.provider, exact))
        })
    })
}

fn provider_info(provider: Provider) -> &'static ProviderInfo {
    PROVIDER_INFOS
        .iter()
        .find(|info| info.provider == provider)
        .expect("provider metadata table should include every provider")
}

fn icon_summary(icons: ProviderIconSet) -> ProviderIconSummary {
    ProviderIconSummary {
        emoji: icon_mode_summary(icons.emoji),
        nerd_font: icon_mode_summary(icons.nerd_font),
        nerd_font_patched: icon_mode_summary(icons.nerd_font_patched),
    }
}

fn icon_mode_summary(marker: &'static str) -> ProviderIconModeSummary {
    ProviderIconModeSummary {
        marker,
        codepoints: marker_codepoints(marker),
    }
}

pub(crate) fn marker_codepoints(marker: &str) -> Vec<String> {
    marker
        .chars()
        .map(|character| format!("U+{:04X}", character as u32))
        .collect()
}

fn matches_binary(command: &str, provider: &str, allow_suffix: bool) -> Option<bool> {
    if command == provider {
        return Some(true);
    }
    if allow_suffix
        && command
            .strip_prefix(provider)
            .is_some_and(|suffix| suffix.starts_with('-'))
    {
        return Some(false);
    }
    None
}
