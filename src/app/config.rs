use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use super::picker::PickerKeySet;

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
    pub(crate) picker_keys: PickerKeySet,
    pub(crate) config_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedIconConfig {
    pub(crate) icons: IconMode,
    pub(crate) config_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedPickerConfig {
    pub(crate) picker_keys: PickerKeySet,
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
    icons: Option<toml::Value>,
    picker_keys: Option<toml::Value>,
    disable_reconcile: Option<toml::Value>,
    disable_proc_fallback: Option<toml::Value>,
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
    let (config_path, file_config) = load_file_config(source)?;
    let icons = resolve_icon_mode(source, &file_config, config_path.as_deref())?;
    let picker_keys = resolve_picker_keys(&file_config, config_path.as_deref())?;

    Ok(ResolvedConfig {
        icons,
        picker_keys,
        config_path,
    })
}

pub(crate) fn resolve_icon_config(cli: CliConfigOverrides) -> Result<ResolvedIconConfig> {
    resolve_icon_config_from_source(&ConfigSource::from_env(cli))
}

pub(crate) fn resolve_icon_config_from_source(source: &ConfigSource) -> Result<ResolvedIconConfig> {
    let config_path = config_path(source);
    if let Some(icons) = resolve_icon_override(source)? {
        return Ok(ResolvedIconConfig { icons, config_path });
    }

    let file_config = read_file_config_for_path(config_path.as_deref())?;
    let icons = resolve_file_icon_mode(&file_config, config_path.as_deref())?;

    Ok(ResolvedIconConfig { icons, config_path })
}

pub(crate) fn resolve_picker_config() -> Result<ResolvedPickerConfig> {
    resolve_picker_config_from_source(&ConfigSource::from_env(CliConfigOverrides::default()))
}

pub(crate) fn resolve_picker_config_from_source(
    source: &ConfigSource,
) -> Result<ResolvedPickerConfig> {
    let (config_path, file_config) = load_file_config(source)?;
    let picker_keys = resolve_picker_keys(&file_config, config_path.as_deref())?;

    Ok(ResolvedPickerConfig {
        picker_keys,
        config_path,
    })
}

pub(crate) fn resolve_runtime_options() -> Result<ResolvedRuntimeOptions> {
    resolve_runtime_options_from_source(&ConfigSource::from_env(CliConfigOverrides::default()))
}

pub(crate) fn resolve_runtime_options_from_source(
    source: &ConfigSource,
) -> Result<ResolvedRuntimeOptions> {
    let (config_path, file_config) = load_file_config(source)?;

    Ok(ResolvedRuntimeOptions {
        // Periodic reconcile is disabled by default; the event-driven path is
        // authoritative and the connect/reconnect bootstrap still recovers
        // ground truth. Set `disable_reconcile = false` to re-enable polling.
        disable_reconcile: resolve_bool_option(
            source.env_disable_reconcile.as_deref(),
            file_config.disable_reconcile.as_ref(),
            true,
            "disable_reconcile",
            config_path.as_deref(),
        )?,
        disable_proc_fallback: resolve_bool_option(
            source.env_disable_proc_fallback.as_deref(),
            file_config.disable_proc_fallback.as_ref(),
            false,
            "disable_proc_fallback",
            config_path.as_deref(),
        )?,
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

fn load_file_config(source: &ConfigSource) -> Result<(Option<PathBuf>, FileConfig)> {
    let config_path = config_path(source);
    let file_config = read_file_config_for_path(config_path.as_deref())?;

    Ok((config_path, file_config))
}

fn read_file_config_for_path(config_path: Option<&Path>) -> Result<FileConfig> {
    match config_path {
        Some(path) if path.exists() => read_config_file(path),
        _ => Ok(FileConfig::default()),
    }
}

fn read_config_file(path: &Path) -> Result<FileConfig> {
    let contents =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    toml::from_str(&contents).with_context(|| format!("failed to parse {}", path.display()))
}

fn resolve_icon_mode(
    source: &ConfigSource,
    file_config: &FileConfig,
    config_path: Option<&Path>,
) -> Result<IconMode> {
    if let Some(icons) = resolve_icon_override(source)? {
        return Ok(icons);
    }

    resolve_file_icon_mode(file_config, config_path)
}

fn resolve_icon_override(source: &ConfigSource) -> Result<Option<IconMode>> {
    if let Some(icons) = source.cli.icons {
        return Ok(Some(icons));
    }

    let env_icons = source
        .env_icons
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(|value| parse_icon_mode(value, ICONS_ENV_VAR))
        .transpose()?;
    if let Some(icons) = env_icons {
        return Ok(Some(icons));
    }

    Ok(None)
}

fn resolve_file_icon_mode(
    file_config: &FileConfig,
    config_path: Option<&Path>,
) -> Result<IconMode> {
    with_config_context(
        file_config
            .icons
            .as_ref()
            .map(|value| parse_icon_mode_value(value, "icons"))
            .transpose(),
        config_path,
    )
    .map(|icons| icons.unwrap_or_default())
}

fn resolve_picker_keys(
    file_config: &FileConfig,
    config_path: Option<&Path>,
) -> Result<PickerKeySet> {
    with_config_context(
        file_config
            .picker_keys
            .as_ref()
            .map(parse_picker_key_set)
            .transpose(),
        config_path,
    )
    .map(|picker_keys| picker_keys.unwrap_or_default())
}

fn parse_icon_mode_value(value: &toml::Value, source_name: &str) -> Result<IconMode> {
    let Some(value) = value.as_str() else {
        bail!("invalid {source_name} value; expected one of: emoji, nerd-font, nerd-font-patched");
    };

    parse_icon_mode(value, source_name)
}

fn parse_icon_mode(value: &str, source_name: &str) -> Result<IconMode> {
    match value.trim() {
        "emoji" => Ok(IconMode::Emoji),
        "nerd-font" => Ok(IconMode::NerdFont),
        "nerd-font-patched" => Ok(IconMode::NerdFontPatched),
        other => bail!(
            "invalid {source_name} value `{other}`; expected one of: emoji, nerd-font, nerd-font-patched"
        ),
    }
}

fn parse_picker_key_set(value: &toml::Value) -> Result<PickerKeySet> {
    let Some(values) = value.as_array() else {
        bail!("picker_keys must be an array of strings");
    };

    let mut keys = Vec::with_capacity(values.len());
    for value in values {
        let Some(key) = value.as_str() else {
            bail!("picker_keys must be an array of strings");
        };
        keys.push(key.to_string());
    }

    PickerKeySet::from_config_values(&keys)
}

fn with_config_context<T>(result: Result<T>, config_path: Option<&Path>) -> Result<T> {
    match config_path {
        Some(path) => result.with_context(|| format!("failed to parse {}", path.display())),
        None => result,
    }
}

fn resolve_bool_option(
    env_value: Option<&str>,
    file_value: Option<&toml::Value>,
    default: bool,
    source_name: &str,
    config_path: Option<&Path>,
) -> Result<bool> {
    if let Some(env_value) = env_value {
        return Ok(parse_bool_env(env_value));
    }

    let Some(file_value) = file_value else {
        return Ok(default);
    };

    with_config_context(parse_bool_config(file_value, source_name), config_path)
}

fn parse_bool_config(value: &toml::Value, source_name: &str) -> Result<bool> {
    value
        .as_bool()
        .with_context(|| format!("invalid {source_name} value; expected true or false"))
}

fn parse_bool_env(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && !matches!(
            value.to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        )
}
