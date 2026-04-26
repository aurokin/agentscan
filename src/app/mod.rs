use std::env;
use std::fmt;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

pub mod bench_support;
mod cache;
mod classify;
mod commands;
mod daemon;
mod output;
mod popup_ui;
mod proc;
#[cfg(test)]
mod tests;
mod tmux;

pub use commands::run;

const PANE_DELIM: &str = "\u{001f}";
const TMUX_FORMAT_DELIM: &str = r"\037";
const CACHE_ENV_VAR: &str = "AGENTSCAN_CACHE_PATH";
const CACHE_RELATIVE_PATH: &str = "agentscan/cache-v1.json";
const CACHE_SCHEMA_VERSION: u32 = 3;
static CACHE_WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const CLAUDE_SPINNER_GLYPHS: &[char] = &[
    '⠁', '⠂', '⠄', '⡀', '⢀', '⠠', '⠐', '⠈', '⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏', '⣾',
    '⣽', '⣻', '⢿', '⡿', '⣟', '⣯', '⣷',
];
const IDLE_GLYPHS: &[char] = &['✳'];
const PANE_FORMAT: &str = concat!(
    "#{session_name}",
    r"\037",
    "#{window_index}",
    r"\037",
    "#{pane_index}",
    r"\037",
    "#{pane_id}",
    r"\037",
    "#{pane_pid}",
    r"\037",
    "#{pane_current_command}",
    r"\037",
    "#{pane_title}",
    r"\037",
    "#{pane_tty}",
    r"\037",
    "#{pane_current_path}",
    r"\037",
    "#{window_name}",
    r"\037",
    "#{session_id}",
    r"\037",
    "#{window_id}",
    r"\037",
    "#{@agent.provider}",
    r"\037",
    "#{@agent.label}",
    r"\037",
    "#{@agent.cwd}",
    r"\037",
    "#{@agent.state}",
    r"\037",
    "#{@agent.session_id}"
);
const DAEMON_SUBSCRIPTION_FORMAT: &str = concat!(
    "agentscan:%*:",
    "#{{pane_id}}:",
    "#{{pane_current_command}}:",
    "#{{pane_title}}:",
    "#{{@agent.provider}}:",
    "#{{@agent.label}}:",
    "#{{@agent.cwd}}:",
    "#{{@agent.state}}:",
    "#{{@agent.session_id}}"
);

#[derive(Parser, Debug)]
#[command(author, version, about = "Scan tmux panes for agent sessions")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    #[command(flatten)]
    list_args: ListArgs,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Take a direct snapshot from tmux.
    Scan(ListArgs),
    /// List panes using the best available state source.
    List(ListArgs),
    /// Open the interactive tmux popup UI. `popup` is interactive-only; use `list --format json` for automation.
    Popup(PopupArgs),
    /// Inspect one pane by pane id.
    Inspect(InspectArgs),
    /// Focus a pane by pane id.
    Focus(FocusArgs),
    /// Run daemon-related commands.
    Daemon(DaemonArgs),
    /// tmux-facing helper commands.
    Tmux(TmuxArgs),
    /// Inspect cache-related paths.
    Cache(CacheArgs),
}

#[derive(Args, Clone, Copy, Debug)]
struct ListArgs {
    #[command(flatten)]
    refresh: RefreshArgs,

    /// Include all tmux panes, not only likely agent panes.
    #[arg(long)]
    all: bool,

    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
}

#[derive(Args, Debug)]
struct InspectArgs {
    /// The tmux pane id, for example `%42`.
    pane_id: String,

    #[command(flatten)]
    refresh: RefreshArgs,

    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
}

#[derive(Args, Debug)]
struct FocusArgs {
    /// The tmux pane id, for example `%42`.
    pane_id: String,

    #[command(flatten)]
    refresh: RefreshArgs,

    /// The tmux client tty to target when switching panes.
    #[arg(long)]
    client_tty: Option<String>,
}

#[derive(Args, Debug)]
struct PopupArgs {
    #[command(flatten)]
    refresh: RefreshArgs,

    /// Include all tmux panes, not only likely agent panes, in the interactive picker.
    #[arg(long)]
    all: bool,
}

#[derive(Args, Debug)]
struct CacheArgs {
    #[command(subcommand)]
    command: CacheCommands,
}

#[derive(Args, Debug)]
struct DaemonArgs {
    #[command(subcommand)]
    command: DaemonCommands,
}

#[derive(Args, Debug)]
struct TmuxArgs {
    #[command(subcommand)]
    command: TmuxCommands,
}

#[derive(Subcommand, Debug)]
enum CacheCommands {
    /// Print the cache path.
    Path,
    /// Show cache contents or summary information.
    Show(CacheShowArgs),
    /// Validate the current cache file.
    Validate(CacheValidateArgs),
}

#[derive(Args, Debug)]
struct CacheShowArgs {
    #[command(flatten)]
    refresh: RefreshArgs,

    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
}

#[derive(Args, Debug)]
struct CacheValidateArgs {
    #[command(flatten)]
    refresh: RefreshArgs,

    /// Fail if the cache is older than this many seconds.
    #[arg(long)]
    max_age_seconds: Option<u64>,
}

#[derive(Subcommand, Debug)]
enum DaemonCommands {
    /// Run the long-lived daemon loop.
    Run,
    /// Report daemon-backed cache health.
    Status(DaemonStatusArgs),
}

#[derive(Args, Debug)]
struct DaemonStatusArgs {
    /// Mark the daemon cache unhealthy if it is older than this many seconds.
    #[arg(long)]
    max_age_seconds: Option<u64>,
}

#[derive(Subcommand, Debug)]
enum TmuxCommands {
    /// Publish explicit pane metadata for wrappers.
    SetMetadata(TmuxSetMetadataArgs),
    /// Clear explicit pane metadata.
    ClearMetadata(TmuxClearMetadataArgs),
}

#[derive(Args, Debug)]
struct TmuxSetMetadataArgs {
    /// The tmux pane id to target. Defaults to the current pane when inside tmux.
    #[arg(long)]
    pane_id: Option<String>,

    /// Explicit provider published by the wrapper.
    #[arg(long, value_enum)]
    provider: Option<Provider>,

    /// User-facing short label published by the wrapper.
    #[arg(long)]
    label: Option<String>,

    /// Explicit working directory published by the wrapper.
    #[arg(long)]
    cwd: Option<String>,

    /// Optional explicit state published by the wrapper.
    #[arg(long, value_enum)]
    state: Option<StatusKind>,

    /// Optional provider-specific session identifier.
    #[arg(long)]
    session_id: Option<String>,
}

#[derive(Args, Debug)]
struct TmuxClearMetadataArgs {
    /// The tmux pane id to target. Defaults to the current pane when inside tmux.
    #[arg(long)]
    pane_id: Option<String>,

    /// Clear only specific metadata fields. Defaults to all fields.
    #[arg(long, value_enum)]
    field: Vec<TmuxMetadataField>,
}

#[derive(Args, Clone, Copy, Debug, Default)]
struct RefreshArgs {
    /// Force a fresh tmux snapshot and rewrite the cache before running the command.
    #[arg(short = 'f', long = "refresh")]
    refresh: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum TmuxMetadataField {
    Provider,
    Label,
    Cwd,
    State,
    SessionId,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
enum Provider {
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
        let name = match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Gemini => "gemini",
            Self::Opencode => "opencode",
            Self::Copilot => "copilot",
            Self::CursorCli => "cursor_cli",
            Self::Pi => "pi",
        };

        f.write_str(name)
    }
}

fn provider_display_marker(provider: Option<Provider>) -> String {
    match provider {
        Some(Provider::Codex) => "\u{f07b5}".to_string(),
        Some(Provider::Claude) => "\u{e76f}".to_string(),
        Some(Provider::Gemini) => "\u{e7f0}".to_string(),
        Some(Provider::Copilot) => "\u{ec1e}".to_string(),
        Some(Provider::CursorCli) => "\u{f12e9}".to_string(),
        Some(Provider::Pi) => "\u{e22c}".to_string(),
        Some(Provider::Opencode) => "\u{f07e2}".to_string(),
        None => "unknown".to_string(),
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum SourceKind {
    Snapshot,
    Daemon,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
enum StatusKind {
    Idle,
    Busy,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum StatusSource {
    PaneMetadata,
    TmuxTitle,
    NotChecked,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ClassificationConfidence {
    High,
    Medium,
    Low,
}

#[allow(clippy::enum_variant_names)]
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ClassificationMatchKind {
    PaneMetadata,
    PaneCurrentCommand,
    PaneTitle,
    ProcProcessTree,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct SnapshotEnvelope {
    schema_version: u32,
    generated_at: String,
    source: SnapshotSource,
    panes: Vec<PaneRecord>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SnapshotSource {
    kind: SourceKind,
    tmux_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    daemon_generated_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct PaneRecord {
    pane_id: String,
    location: PaneLocation,
    tmux: TmuxPaneMetadata,
    display: DisplayMetadata,
    provider: Option<Provider>,
    status: PaneStatus,
    classification: PaneClassification,
    agent_metadata: AgentMetadata,
    diagnostics: PaneDiagnostics,
}

impl PaneRecord {
    fn display_label(&self) -> &str {
        &self.display.label
    }

    fn location_tag(&self) -> String {
        self.location.tag()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PaneLocation {
    session_name: String,
    window_index: u32,
    pane_index: u32,
    window_name: String,
}

impl PaneLocation {
    fn tag(&self) -> String {
        format!(
            "{}:{}.{}",
            self.session_name, self.window_index, self.pane_index
        )
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct TmuxPaneMetadata {
    pane_pid: u32,
    pane_tty: String,
    pane_current_path: String,
    pane_current_command: String,
    pane_title_raw: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct DisplayMetadata {
    label: String,
    activity_label: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PaneStatus {
    kind: StatusKind,
    source: StatusSource,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PaneClassification {
    matched_by: Option<ClassificationMatchKind>,
    confidence: Option<ClassificationConfidence>,
    reasons: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ProcFallbackOutcome {
    NotRun,
    Skipped,
    NoMatch,
    Error,
    Resolved,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ProcFallbackDiagnostics {
    outcome: ProcFallbackOutcome,
    reason: String,
    commands: Vec<String>,
}

impl Default for ProcFallbackDiagnostics {
    fn default() -> Self {
        Self {
            outcome: ProcFallbackOutcome::NotRun,
            reason: "proc fallback was not evaluated".to_string(),
            commands: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct AgentMetadata {
    provider: Option<String>,
    label: Option<String>,
    cwd: Option<String>,
    state: Option<String>,
    session_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PaneDiagnostics {
    cache_origin: String,
    #[serde(default)]
    proc_fallback: ProcFallbackDiagnostics,
}

#[derive(Debug)]
struct CacheSummary {
    generated_at: OffsetDateTime,
    pane_count: usize,
    agent_pane_count: usize,
    provider_counts: Vec<(Provider, usize)>,
    status_counts: Vec<(StatusKind, usize)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DaemonCacheStatus {
    Healthy,
    Stale,
    SnapshotOnly,
    Unavailable,
}

#[derive(Debug)]
struct CacheDiagnostics {
    cache_age_seconds: u64,
    daemon_age_seconds: Option<u64>,
    daemon_cache_status: DaemonCacheStatus,
    daemon_status_reason: String,
}

#[derive(Clone, Debug)]
struct TmuxPaneRow {
    session_name: String,
    window_index: u32,
    pane_index: u32,
    pane_id: String,
    pane_pid: u32,
    pane_current_command: String,
    pane_title_raw: String,
    pane_tty: String,
    pane_current_path: String,
    window_name: String,
    session_id: Option<String>,
    window_id: Option<String>,
    agent_provider: Option<String>,
    agent_label: Option<String>,
    agent_cwd: Option<String>,
    agent_state: Option<String>,
    agent_session_id: Option<String>,
}

#[derive(Debug)]
struct ProviderMatch {
    provider: Provider,
    matched_by: ClassificationMatchKind,
    confidence: ClassificationConfidence,
    reasons: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct TmuxClientRow {
    client_tty: String,
    client_activity: i64,
}

fn status_kind_name(status: StatusKind) -> &'static str {
    match status {
        StatusKind::Idle => "idle",
        StatusKind::Busy => "busy",
        StatusKind::Unknown => "unknown",
    }
}

fn status_source_name(source: StatusSource) -> &'static str {
    match source {
        StatusSource::PaneMetadata => "pane_metadata",
        StatusSource::TmuxTitle => "tmux_title",
        StatusSource::NotChecked => "not_checked",
    }
}

fn classification_match_kind_name(kind: ClassificationMatchKind) -> &'static str {
    match kind {
        ClassificationMatchKind::PaneMetadata => "pane_metadata",
        ClassificationMatchKind::PaneCurrentCommand => "pane_current_command",
        ClassificationMatchKind::PaneTitle => "pane_title",
        ClassificationMatchKind::ProcProcessTree => "proc_process_tree",
    }
}

fn classification_confidence_name(confidence: ClassificationConfidence) -> &'static str {
    match confidence {
        ClassificationConfidence::High => "high",
        ClassificationConfidence::Medium => "medium",
        ClassificationConfidence::Low => "low",
    }
}

fn proc_fallback_outcome_name(outcome: ProcFallbackOutcome) -> &'static str {
    match outcome {
        ProcFallbackOutcome::NotRun => "not_run",
        ProcFallbackOutcome::Skipped => "skipped",
        ProcFallbackOutcome::NoMatch => "no_match",
        ProcFallbackOutcome::Error => "error",
        ProcFallbackOutcome::Resolved => "resolved",
    }
}

fn daemon_cache_status_name(status: DaemonCacheStatus) -> &'static str {
    match status {
        DaemonCacheStatus::Healthy => "healthy",
        DaemonCacheStatus::Stale => "stale",
        DaemonCacheStatus::SnapshotOnly => "snapshot_only",
        DaemonCacheStatus::Unavailable => "unavailable",
    }
}

fn default_if_empty<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.trim().is_empty() {
        fallback
    } else {
        value
    }
}
