use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

pub(crate) const ICONS_ENV_VAR: &str = "AGENTSCAN_ICONS";
pub(crate) const DISABLE_RECONCILE_ENV_VAR: &str = "AGENTSCAN_DISABLE_RECONCILE";
pub(crate) const DISABLE_PROC_FALLBACK_ENV_VAR: &str = "AGENTSCAN_DISABLE_PROC_FALLBACK";
const CONFIG_FILE_NAME: &str = "config.toml";
const CONFIG_DIR_NAME: &str = "agentscan";

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum IconMode {
    #[default]
    Emoji,
    NerdFont,
    NerdFontPatched,
}

impl fmt::Display for IconMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl IconMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Emoji => "emoji",
            Self::NerdFont => "nerd-font",
            Self::NerdFontPatched => "nerd-font-patched",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct CliConfigOverrides {
    pub(crate) icons: Option<IconMode>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedConfig {
    pub(crate) icons: IconMode,
    pub(crate) config_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedRuntimeOptions {
    pub(crate) disable_reconcile: bool,
    pub(crate) disable_proc_fallback: bool,
    pub(crate) config_path: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    icons: Option<IconMode>,
    disable_reconcile: Option<bool>,
    disable_proc_fallback: Option<bool>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ConfigSource {
    pub(crate) cli: CliConfigOverrides,
    pub(crate) env_icons: Option<String>,
    pub(crate) env_disable_reconcile: Option<String>,
    pub(crate) env_disable_proc_fallback: Option<String>,
    pub(crate) xdg_config_home: Option<PathBuf>,
    pub(crate) home: Option<PathBuf>,
}

impl ConfigSource {
    pub(crate) fn from_env(cli: CliConfigOverrides) -> Self {
        Self {
            cli,
            env_icons: env::var(ICONS_ENV_VAR).ok(),
            env_disable_reconcile: env::var(DISABLE_RECONCILE_ENV_VAR).ok(),
            env_disable_proc_fallback: env::var(DISABLE_PROC_FALLBACK_ENV_VAR).ok(),
            xdg_config_home: env_path("XDG_CONFIG_HOME"),
            home: env_path("HOME"),
        }
    }
}

pub(crate) fn resolve_config(cli: CliConfigOverrides) -> Result<ResolvedConfig> {
    resolve_config_from_source(&ConfigSource::from_env(cli))
}

pub(crate) fn resolve_config_from_source(source: &ConfigSource) -> Result<ResolvedConfig> {
    let config_path = config_path(source);
    if let Some(icons) = source.cli.icons {
        return Ok(ResolvedConfig { icons, config_path });
    }

    let env_icons = source
        .env_icons
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(parse_icon_mode)
        .transpose()?;
    if let Some(icons) = env_icons {
        return Ok(ResolvedConfig { icons, config_path });
    }

    let file_config = match config_path.as_deref() {
        Some(path) if path.exists() => read_config_file(path)?,
        _ => FileConfig::default(),
    };

    Ok(ResolvedConfig {
        icons: file_config.icons.unwrap_or_default(),
        config_path,
    })
}

pub(crate) fn resolve_runtime_options() -> Result<ResolvedRuntimeOptions> {
    resolve_runtime_options_from_source(&ConfigSource::from_env(CliConfigOverrides::default()))
}

pub(crate) fn resolve_runtime_options_from_source(
    source: &ConfigSource,
) -> Result<ResolvedRuntimeOptions> {
    let config_path = config_path(source);
    let file_config = match config_path.as_deref() {
        Some(path) if path.exists() => read_config_file(path)?,
        _ => FileConfig::default(),
    };

    Ok(ResolvedRuntimeOptions {
        disable_reconcile: resolve_bool_option(
            source.env_disable_reconcile.as_deref(),
            file_config.disable_reconcile,
        ),
        disable_proc_fallback: resolve_bool_option(
            source.env_disable_proc_fallback.as_deref(),
            file_config.disable_proc_fallback,
        ),
        config_path,
    })
}

pub(crate) fn config_path(source: &ConfigSource) -> Option<PathBuf> {
    source
        .xdg_config_home
        .clone()
        .filter(|path| is_usable_config_root(path))
        .or_else(|| {
            source
                .home
                .as_ref()
                .filter(|home| is_usable_config_root(home))
                .map(|home| home.join(".config"))
        })
        .map(|config_home| config_home.join(CONFIG_DIR_NAME).join(CONFIG_FILE_NAME))
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn is_usable_config_root(path: &Path) -> bool {
    !path.as_os_str().is_empty() && path.is_absolute()
}

fn read_config_file(path: &Path) -> Result<FileConfig> {
    let contents =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    toml::from_str(&contents).with_context(|| format!("failed to parse {}", path.display()))
}

fn parse_icon_mode(value: &str) -> Result<IconMode> {
    match value.trim() {
        "emoji" => Ok(IconMode::Emoji),
        "nerd-font" => Ok(IconMode::NerdFont),
        "nerd-font-patched" => Ok(IconMode::NerdFontPatched),
        other => bail!(
            "invalid {ICONS_ENV_VAR} value `{other}`; expected one of: emoji, nerd-font, nerd-font-patched"
        ),
    }
}

fn resolve_bool_option(env_value: Option<&str>, file_value: Option<bool>) -> bool {
    env_value
        .map(parse_bool_env)
        .or(file_value)
        .unwrap_or(false)
}

fn parse_bool_env(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && !matches!(
            value.to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        )
}
