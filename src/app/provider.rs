use std::fmt;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Provider {
    Codex,
    Claude,
    Gemini,
    Opencode,
    #[value(alias = "github-copilot")]
    Copilot,
    #[value(name = "cursor_cli", alias = "cursor-cli", alias = "cursor-agent")]
    CursorCli,
    #[value(alias = "pi-coding-agent")]
    Pi,
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(provider_info(*self).canonical_name)
    }
}

pub(crate) struct ProviderInfo {
    provider: Provider,
    canonical_name: &'static str,
    display_marker: &'static str,
    metadata_aliases: &'static [&'static str],
    command_aliases: &'static [ProviderCommandAlias],
    title_prefixes: &'static [&'static str],
    title_aliases: &'static [&'static str],
    generic_display_labels: &'static [&'static str],
}

#[derive(Clone, Copy)]
pub(crate) struct ProviderCommandAlias {
    name: &'static str,
    allow_suffix: bool,
}

impl ProviderCommandAlias {
    const fn new(name: &'static str, allow_suffix: bool) -> Self {
        Self { name, allow_suffix }
    }
}

const PROVIDER_INFOS: &[ProviderInfo] = &[
    ProviderInfo {
        provider: Provider::Codex,
        canonical_name: "codex",
        display_marker: "\u{f07b5}",
        metadata_aliases: &["codex"],
        command_aliases: &[ProviderCommandAlias::new("codex", true)],
        title_prefixes: &[],
        title_aliases: &[],
        generic_display_labels: &[],
    },
    ProviderInfo {
        provider: Provider::Claude,
        canonical_name: "claude",
        display_marker: "\u{e76f}",
        metadata_aliases: &["claude"],
        command_aliases: &[ProviderCommandAlias::new("claude", true)],
        title_prefixes: &["Claude Code | ", "Claude | "],
        title_aliases: &["Claude Code"],
        generic_display_labels: &[],
    },
    ProviderInfo {
        provider: Provider::Gemini,
        canonical_name: "gemini",
        display_marker: "\u{e7f0}",
        metadata_aliases: &["gemini"],
        command_aliases: &[ProviderCommandAlias::new("gemini", true)],
        title_prefixes: &[],
        title_aliases: &[],
        generic_display_labels: &[],
    },
    ProviderInfo {
        provider: Provider::Opencode,
        canonical_name: "opencode",
        display_marker: "\u{f07e2}",
        metadata_aliases: &["opencode"],
        command_aliases: &[ProviderCommandAlias::new("opencode", true)],
        title_prefixes: &["OC | "],
        title_aliases: &["OpenCode"],
        generic_display_labels: &["OpenCode"],
    },
    ProviderInfo {
        provider: Provider::Copilot,
        canonical_name: "copilot",
        display_marker: "\u{ec1e}",
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
        display_marker: "\u{f12e9}",
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
        display_marker: "\u{e22c}",
        metadata_aliases: &["pi", "pi-coding-agent", "pi coding agent"],
        command_aliases: &[ProviderCommandAlias::new("pi-coding-agent", false)],
        title_prefixes: &["π - ", "pi - "],
        title_aliases: &[],
        generic_display_labels: &[],
    },
];

pub(crate) fn provider_display_marker(provider: Option<Provider>) -> String {
    provider
        .map(|provider| provider_info(provider).display_marker.to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

pub(crate) fn provider_summary_order() -> impl Iterator<Item = Provider> {
    PROVIDER_INFOS.iter().map(|info| info.provider)
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
