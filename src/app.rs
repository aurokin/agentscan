use std::env;
use std::fmt;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const PANE_DELIM: char = '\u{001f}';
const CACHE_ENV_VAR: &str = "AGENTSCAN_CACHE_PATH";
const CACHE_RELATIVE_PATH: &str = "agentscan/cache-v1.json";
const CACHE_SCHEMA_VERSION: u32 = 1;
const CLAUDE_SPINNER_GLYPHS: &[char] = &[
    '⠁', '⠂', '⠄', '⡀', '⢀', '⠠', '⠐', '⠈', '⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏', '⣾',
    '⣽', '⣻', '⢿', '⡿', '⣟', '⣯', '⣷',
];
const IDLE_GLYPHS: &[char] = &['✳'];
const PANE_FORMAT: &str = concat!(
    "#{session_name}",
    "\x1f",
    "#{window_index}",
    "\x1f",
    "#{pane_index}",
    "\x1f",
    "#{pane_id}",
    "\x1f",
    "#{pane_pid}",
    "\x1f",
    "#{pane_current_command}",
    "\x1f",
    "#{pane_title}",
    "\x1f",
    "#{pane_tty}",
    "\x1f",
    "#{pane_current_path}",
    "\x1f",
    "#{window_name}",
    "\x1f",
    "#{@agent.provider}",
    "\x1f",
    "#{@agent.label}",
    "\x1f",
    "#{@agent.cwd}",
    "\x1f",
    "#{@agent.state}",
    "\x1f",
    "#{@agent.session_id}"
);
const DAEMON_SUBSCRIPTION_FORMAT: &str = concat!(
    "agentscan:%*:",
    "#{{pane_id}}:",
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
    /// Force a fresh tmux snapshot and rewrite the cache before running the command.
    #[arg(short = 'f', long = "refresh", global = true)]
    refresh: bool,

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

#[derive(Args, Clone, Debug)]
struct ListArgs {
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

    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
}

#[derive(Args, Debug)]
struct FocusArgs {
    /// The tmux pane id, for example `%42`.
    pane_id: String,

    /// The tmux client tty to target when switching panes.
    #[arg(long)]
    client_tty: Option<String>,
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
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
}

#[derive(Args, Debug)]
struct CacheValidateArgs {
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
    /// Emit popup-oriented pane output.
    Popup(TmuxPopupArgs),
    /// Publish explicit pane metadata for wrappers.
    SetMetadata(TmuxSetMetadataArgs),
    /// Clear explicit pane metadata.
    ClearMetadata(TmuxClearMetadataArgs),
}

#[derive(Args, Debug)]
struct TmuxPopupArgs {
    /// Include all tmux panes, not only likely agent panes.
    #[arg(long)]
    all: bool,

    /// Output format for popup consumers.
    #[arg(long, value_enum, default_value_t = PopupOutputFormat::Tsv)]
    format: PopupOutputFormat,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum PopupOutputFormat {
    Tsv,
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
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Gemini => "gemini",
            Self::Opencode => "opencode",
        };

        f.write_str(name)
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
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct SnapshotEnvelope {
    schema_version: u32,
    generated_at: String,
    source: SnapshotSource,
    panes: Vec<PaneRecord>,
}

#[derive(Debug, Deserialize, Serialize)]
struct SnapshotSource {
    kind: SourceKind,
    tmux_version: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
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
}

#[derive(Debug, Deserialize, Serialize)]
struct PaneLocation {
    session_name: String,
    window_index: u32,
    pane_index: u32,
    window_name: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct TmuxPaneMetadata {
    pane_pid: u32,
    pane_tty: String,
    pane_current_path: String,
    pane_current_command: String,
    pane_title_raw: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct DisplayMetadata {
    label: String,
    activity_label: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PaneStatus {
    kind: StatusKind,
    source: StatusSource,
}

#[derive(Debug, Deserialize, Serialize)]
struct PaneClassification {
    matched_by: Option<ClassificationMatchKind>,
    confidence: Option<ClassificationConfidence>,
    reasons: Vec<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct AgentMetadata {
    provider: Option<String>,
    label: Option<String>,
    cwd: Option<String>,
    state: Option<String>,
    session_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PaneDiagnostics {
    cache_origin: String,
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
    Unavailable,
}

#[derive(Debug, Serialize)]
pub(crate) struct PopupEntry {
    pane_id: String,
    provider: Option<Provider>,
    status: StatusKind,
    session_name: String,
    window_index: u32,
    pane_index: u32,
    display_label: String,
}

#[derive(Clone, Debug)]
pub(crate) struct TmuxPaneRow {
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
struct TmuxClientRow {
    client_tty: String,
    client_activity: i64,
}

pub(crate) fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Scan(args)) => command_scan(&args, cli.refresh),
        Some(Commands::List(args)) => command_list(&args, cli.refresh),
        Some(Commands::Inspect(args)) => command_inspect(&args, cli.refresh),
        Some(Commands::Focus(args)) => command_focus(&args, cli.refresh),
        Some(Commands::Daemon(args)) => command_daemon(&args),
        Some(Commands::Tmux(args)) => command_tmux(&args, cli.refresh),
        Some(Commands::Cache(args)) => command_cache(&args, cli.refresh),
        None => command_list(&cli.list_args, cli.refresh),
    }
}

fn command_scan(args: &ListArgs, refresh: bool) -> Result<()> {
    let mut snapshot = snapshot_from_tmux()?;
    if refresh {
        write_snapshot_to_cache(&snapshot)?;
    }
    filter_snapshot(&mut snapshot, args.all);
    emit_snapshot(&snapshot, args.format)
}

fn command_list(args: &ListArgs, refresh: bool) -> Result<()> {
    let mut snapshot = load_snapshot(refresh)?;
    filter_snapshot(&mut snapshot, args.all);
    emit_snapshot(&snapshot, args.format)
}

fn command_inspect(args: &InspectArgs, refresh: bool) -> Result<()> {
    let snapshot = load_snapshot(refresh)?;
    let pane = snapshot
        .panes
        .into_iter()
        .find(|pane| pane.pane_id == args.pane_id)
        .with_context(|| format!("pane {} not found in tmux snapshot", args.pane_id))?;

    match args.format {
        OutputFormat::Text => print_inspect_text(&pane),
        OutputFormat::Json => print_json(&pane)?,
    }

    Ok(())
}

fn command_focus(args: &FocusArgs, refresh: bool) -> Result<()> {
    if refresh {
        let _ = refresh_cache_from_tmux()?;
    }
    focus_tmux_pane(&args.pane_id, args.client_tty.as_deref())
}

fn command_daemon(args: &DaemonArgs) -> Result<()> {
    match args.command {
        DaemonCommands::Run => daemon_run(),
        DaemonCommands::Status(ref args) => command_daemon_status(args),
    }
}

fn command_daemon_status(args: &DaemonStatusArgs) -> Result<()> {
    let path = cache_path()?;
    let snapshot = read_snapshot_from_cache()?;
    let summary = summarize_snapshot(&snapshot)?;
    let age_seconds = cache_age_seconds(summary.generated_at);
    let status = daemon_cache_status(&snapshot, age_seconds, args.max_age_seconds);

    println!("daemon_cache_status: {}", daemon_cache_status_name(status));
    println!("path: {}", path.display());
    println!("generated_at: {}", snapshot.generated_at);
    println!("age_seconds: {age_seconds}");
    println!("source: {:?}", snapshot.source.kind);
    println!("pane_count: {}", summary.pane_count);

    if let Some(max_age_seconds) = args.max_age_seconds {
        println!("max_age_seconds: {max_age_seconds}");
    }

    match status {
        DaemonCacheStatus::Healthy => Ok(()),
        DaemonCacheStatus::Stale => bail!("daemon cache is stale"),
        DaemonCacheStatus::Unavailable => {
            bail!("daemon cache is unavailable because the cache source is not daemon-backed")
        }
    }
}

fn command_cache(args: &CacheArgs, refresh: bool) -> Result<()> {
    match args.command {
        CacheCommands::Path => {
            println!("{}", cache_path()?.display());
        }
        CacheCommands::Show(ref args) => {
            let snapshot = load_snapshot(refresh)?;
            match args.format {
                OutputFormat::Text => print_cache_summary_text(&snapshot)?,
                OutputFormat::Json => print_json(&snapshot)?,
            }
        }
        CacheCommands::Validate(ref args) => {
            let path = cache_path()?;
            let snapshot = load_snapshot(refresh)?;
            let summary = validate_snapshot(&snapshot, args.max_age_seconds)?;
            print_cache_validate_text(&path, &snapshot, &summary, args.max_age_seconds);
        }
    }

    Ok(())
}

fn command_tmux(args: &TmuxArgs, refresh: bool) -> Result<()> {
    match &args.command {
        TmuxCommands::Popup(args) => command_tmux_popup(args, refresh),
        TmuxCommands::SetMetadata(args) => command_tmux_set_metadata(args),
        TmuxCommands::ClearMetadata(args) => command_tmux_clear_metadata(args),
    }
}

fn emit_snapshot(snapshot: &SnapshotEnvelope, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Text => {
            print_list_text(&snapshot.panes);
            Ok(())
        }
        OutputFormat::Json => print_json(snapshot),
    }
}

fn command_tmux_popup(args: &TmuxPopupArgs, refresh: bool) -> Result<()> {
    let mut snapshot = load_snapshot(refresh)?;
    filter_snapshot(&mut snapshot, args.all);
    let entries = popup_entries(&snapshot.panes);

    match args.format {
        PopupOutputFormat::Tsv => {
            print_popup_tsv(&entries);
            Ok(())
        }
        PopupOutputFormat::Json => print_json(&entries),
    }
}

fn command_tmux_set_metadata(args: &TmuxSetMetadataArgs) -> Result<()> {
    let pane_id = resolve_tmux_target_pane(args.pane_id.as_deref(), "set-metadata")?;

    let updates = tmux_metadata_updates(args);
    if updates.is_empty() {
        bail!("no metadata fields were provided");
    }

    for (option_name, value) in updates {
        set_tmux_pane_option(&pane_id, option_name, &value)?;
    }

    println!("updated pane metadata for {pane_id}");
    Ok(())
}

fn command_tmux_clear_metadata(args: &TmuxClearMetadataArgs) -> Result<()> {
    let pane_id = resolve_tmux_target_pane(args.pane_id.as_deref(), "clear-metadata")?;
    let fields = tmux_metadata_fields_to_clear(&args.field);

    for option_name in fields {
        unset_tmux_pane_option(&pane_id, option_name)?;
    }

    println!("cleared pane metadata for {pane_id}");
    Ok(())
}

fn print_list_text(panes: &[PaneRecord]) {
    if panes.is_empty() {
        println!("No matching tmux panes.");
        return;
    }

    for pane in panes {
        let provider = pane
            .provider
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".to_string());

        println!(
            "{} {}:{}.{} - {}",
            provider,
            pane.location.session_name,
            pane.location.window_index,
            pane.location.pane_index,
            pane.display_label()
        );
    }
}

fn print_inspect_text(pane: &PaneRecord) {
    println!("pane_id: {}", pane.pane_id);
    println!(
        "location: {}:{}.{} ({})",
        pane.location.session_name,
        pane.location.window_index,
        pane.location.pane_index,
        pane.location.window_name
    );
    println!(
        "provider: {}",
        pane.provider
            .map(|provider| provider.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    );
    println!("display_label: {}", pane.display.label);
    println!("status: {:?}", pane.status.kind);
    println!("status_source: {:?}", pane.status.source);
    println!(
        "command: {}",
        default_if_empty(&pane.tmux.pane_current_command, "<empty>")
    );
    println!(
        "title_raw: {}",
        default_if_empty(&pane.tmux.pane_title_raw, "<empty>")
    );
    println!(
        "cwd: {}",
        default_if_empty(&pane.tmux.pane_current_path, "<empty>")
    );
    println!("tty: {}", default_if_empty(&pane.tmux.pane_tty, "<empty>"));

    if pane.agent_metadata.provider.is_some()
        || pane.agent_metadata.label.is_some()
        || pane.agent_metadata.cwd.is_some()
        || pane.agent_metadata.state.is_some()
        || pane.agent_metadata.session_id.is_some()
    {
        println!("agent_metadata:");
        println!(
            "  provider: {}",
            default_if_empty(
                pane.agent_metadata.provider.as_deref().unwrap_or(""),
                "<empty>"
            )
        );
        println!(
            "  label: {}",
            default_if_empty(
                pane.agent_metadata.label.as_deref().unwrap_or(""),
                "<empty>"
            )
        );
        println!(
            "  cwd: {}",
            default_if_empty(pane.agent_metadata.cwd.as_deref().unwrap_or(""), "<empty>")
        );
        println!(
            "  state: {}",
            default_if_empty(
                pane.agent_metadata.state.as_deref().unwrap_or(""),
                "<empty>"
            )
        );
        println!(
            "  session_id: {}",
            default_if_empty(
                pane.agent_metadata.session_id.as_deref().unwrap_or(""),
                "<empty>"
            )
        );
    }

    if pane.classification.reasons.is_empty() {
        println!("classification: none");
    } else {
        println!("classification:");
        for reason in &pane.classification.reasons {
            println!("  - {reason}");
        }
    }
}

fn print_popup_tsv(entries: &[PopupEntry]) {
    for entry in entries {
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            tsv_escape(&entry.pane_id),
            tsv_escape(
                &entry
                    .provider
                    .map(|provider| provider.to_string())
                    .unwrap_or_default()
            ),
            tsv_escape(status_kind_name(entry.status)),
            tsv_escape(&entry.session_name),
            entry.window_index,
            entry.pane_index,
            tsv_escape(&entry.display_label)
        );
    }
}

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).context("failed to serialize JSON output")?
    );
    Ok(())
}

fn print_cache_summary_text(snapshot: &SnapshotEnvelope) -> Result<()> {
    let path = cache_path()?;
    let summary = summarize_snapshot(snapshot)?;

    println!("path: {}", path.display());
    println!("schema_version: {}", snapshot.schema_version);
    println!("generated_at: {}", snapshot.generated_at);
    println!("source: {:?}", snapshot.source.kind);
    println!(
        "tmux_version: {}",
        snapshot
            .source
            .tmux_version
            .as_deref()
            .unwrap_or("<unknown>")
    );
    println!("pane_count: {}", summary.pane_count);
    println!("agent_pane_count: {}", summary.agent_pane_count);
    println!(
        "providers: {}",
        format_provider_counts(&summary.provider_counts)
    );
    println!("statuses: {}", format_status_counts(&summary.status_counts));

    Ok(())
}

fn print_cache_validate_text(
    path: &Path,
    snapshot: &SnapshotEnvelope,
    summary: &CacheSummary,
    max_age_seconds: Option<u64>,
) {
    println!("cache_valid: yes");
    println!("path: {}", path.display());
    println!("schema_version: {}", snapshot.schema_version);
    println!("generated_at: {}", snapshot.generated_at);
    println!("source: {:?}", snapshot.source.kind);
    println!("pane_count: {}", summary.pane_count);

    if let Some(max_age_seconds) = max_age_seconds {
        let age_seconds = cache_age_seconds(summary.generated_at);
        println!("age_seconds: {age_seconds}");
        println!("max_age_seconds: {max_age_seconds}");
    }
}

fn snapshot_from_tmux() -> Result<SnapshotEnvelope> {
    let rows = tmux_list_panes()?;
    let panes = rows.into_iter().map(pane_from_row).collect();

    Ok(SnapshotEnvelope {
        schema_version: CACHE_SCHEMA_VERSION,
        generated_at: now_rfc3339()?,
        source: SnapshotSource {
            kind: SourceKind::Snapshot,
            tmux_version: tmux_version(),
        },
        panes,
    })
}

fn refresh_cache_from_tmux() -> Result<SnapshotEnvelope> {
    let snapshot = snapshot_from_tmux()?;
    write_snapshot_to_cache(&snapshot)?;
    Ok(snapshot)
}

fn load_snapshot(refresh: bool) -> Result<SnapshotEnvelope> {
    if refresh {
        return refresh_cache_from_tmux();
    }

    read_snapshot_from_cache()
}

fn read_snapshot_from_cache() -> Result<SnapshotEnvelope> {
    let path = cache_path()?;
    let contents = fs::read_to_string(&path).with_context(|| {
        format!(
            "failed to read cache at {}. Run `agentscan daemon run` first",
            path.display()
        )
    })?;

    let snapshot: SnapshotEnvelope = serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse cache at {}", path.display()))?;
    validate_snapshot(&snapshot, None)
        .with_context(|| format!("cache validation failed for {}", path.display()))?;
    Ok(snapshot)
}

fn write_snapshot_to_cache(snapshot: &SnapshotEnvelope) -> Result<()> {
    let path = cache_path()?;
    let parent = path
        .parent()
        .context("cache path did not have a parent directory")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create cache directory {}", parent.display()))?;

    let temp_path = path.with_extension("tmp");
    let contents =
        serde_json::to_vec_pretty(snapshot).context("failed to serialize cache snapshot")?;
    fs::write(&temp_path, contents)
        .with_context(|| format!("failed to write temporary cache {}", temp_path.display()))?;
    fs::rename(&temp_path, &path).with_context(|| {
        format!(
            "failed to move temporary cache {} into place at {}",
            temp_path.display(),
            path.display()
        )
    })?;

    Ok(())
}

fn filter_snapshot(snapshot: &mut SnapshotEnvelope, include_all: bool) {
    if !include_all {
        snapshot.panes.retain(|pane| pane.provider.is_some());
    }
}

pub(crate) fn popup_entries(panes: &[PaneRecord]) -> Vec<PopupEntry> {
    panes
        .iter()
        .map(|pane| PopupEntry {
            pane_id: pane.pane_id.clone(),
            provider: pane.provider,
            status: pane.status.kind,
            session_name: pane.location.session_name.clone(),
            window_index: pane.location.window_index,
            pane_index: pane.location.pane_index,
            display_label: pane.display.label.clone(),
        })
        .collect()
}

pub(crate) fn pane_from_row(row: TmuxPaneRow) -> PaneRecord {
    let agent_metadata = AgentMetadata {
        provider: row.agent_provider.clone(),
        label: row.agent_label.clone(),
        cwd: row.agent_cwd.clone(),
        state: row.agent_state.clone(),
        session_id: row.agent_session_id.clone(),
    };
    let provider_match = classify_provider(
        agent_metadata.provider.as_deref(),
        &row.pane_current_command,
        &row.pane_title_raw,
    );
    let provider = provider_match.as_ref().map(|matched| matched.provider);
    let title_status = infer_title_status(provider, &row.pane_title_raw);
    let status = infer_status(title_status, agent_metadata.state.as_deref());

    PaneRecord {
        pane_id: row.pane_id,
        location: PaneLocation {
            session_name: row.session_name,
            window_index: row.window_index,
            pane_index: row.pane_index,
            window_name: row.window_name.clone(),
        },
        tmux: TmuxPaneMetadata {
            pane_pid: row.pane_pid,
            pane_tty: row.pane_tty,
            pane_current_path: row.pane_current_path,
            pane_current_command: row.pane_current_command.clone(),
            pane_title_raw: row.pane_title_raw.clone(),
        },
        display: DisplayMetadata {
            label: display_label(
                agent_metadata.label.as_deref(),
                &row.pane_title_raw,
                &row.pane_current_command,
                &row.window_name,
            ),
            activity_label: None,
        },
        provider,
        status,
        classification: PaneClassification {
            matched_by: provider_match.as_ref().map(|matched| matched.matched_by),
            confidence: provider_match.as_ref().map(|matched| matched.confidence),
            reasons: provider_match
                .map(|matched| matched.reasons)
                .unwrap_or_default(),
        },
        agent_metadata,
        diagnostics: PaneDiagnostics {
            cache_origin: "direct_snapshot".to_string(),
        },
    }
}

fn tmux_list_panes() -> Result<Vec<TmuxPaneRow>> {
    let output = Command::new("tmux")
        .args(["list-panes", "-a", "-F", PANE_FORMAT])
        .output()
        .context("failed to execute tmux")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        if stderr.is_empty() {
            bail!("tmux list-panes failed with status {}", output.status);
        }
        bail!("tmux list-panes failed: {stderr}");
    }

    let stdout = String::from_utf8(output.stdout).context("tmux output was not valid UTF-8")?;
    parse_pane_rows(&stdout)
}

pub(crate) fn parse_pane_rows(input: &str) -> Result<Vec<TmuxPaneRow>> {
    let mut panes = Vec::new();

    for (line_number, line) in input.lines().enumerate() {
        if line.is_empty() {
            continue;
        }

        let fields: Vec<_> = line.split(PANE_DELIM).collect();
        if fields.len() != 10 && fields.len() != 15 {
            bail!(
                "unexpected tmux pane field count on line {}: expected 10 or 15, got {}",
                line_number + 1,
                fields.len()
            );
        }

        let (agent_provider, agent_label, agent_cwd, agent_state, agent_session_id) =
            if fields.len() == 15 {
                (
                    empty_to_none(fields[10]),
                    empty_to_none(fields[11]),
                    empty_to_none(fields[12]),
                    empty_to_none(fields[13]),
                    empty_to_none(fields[14]),
                )
            } else {
                (None, None, None, None, None)
            };

        panes.push(TmuxPaneRow {
            session_name: fields[0].to_string(),
            window_index: parse_u32(fields[1], "window_index", line_number + 1)?,
            pane_index: parse_u32(fields[2], "pane_index", line_number + 1)?,
            pane_id: fields[3].to_string(),
            pane_pid: parse_u32(fields[4], "pane_pid", line_number + 1)?,
            pane_current_command: fields[5].to_string(),
            pane_title_raw: fields[6].to_string(),
            pane_tty: fields[7].to_string(),
            pane_current_path: fields[8].to_string(),
            window_name: fields[9].to_string(),
            agent_provider,
            agent_label,
            agent_cwd,
            agent_state,
            agent_session_id,
        });
    }

    Ok(panes)
}

fn parse_u32(value: &str, field_name: &str, line_number: usize) -> Result<u32> {
    value.parse::<u32>().with_context(|| {
        format!("failed to parse {field_name} as u32 on tmux output line {line_number}")
    })
}

fn empty_to_none(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn classify_provider(
    published_provider: Option<&str>,
    command: &str,
    title: &str,
) -> Option<ProviderMatch> {
    let title = title.trim();
    let command = command.trim();

    if let Some(provider) = provider_from_metadata(published_provider) {
        return Some(ProviderMatch {
            provider,
            matched_by: ClassificationMatchKind::PaneMetadata,
            confidence: ClassificationConfidence::High,
            reasons: vec![format!(
                "agent.provider={}",
                published_provider.unwrap_or_default().trim()
            )],
        });
    }

    if let Some(provider) = provider_from_title(title) {
        return Some(ProviderMatch {
            provider,
            matched_by: ClassificationMatchKind::PaneTitle,
            confidence: ClassificationConfidence::High,
            reasons: vec![format!("pane_title={title}")],
        });
    }

    if let Some(provider) = provider_from_command(command) {
        return Some(ProviderMatch {
            provider,
            matched_by: ClassificationMatchKind::PaneCurrentCommand,
            confidence: ClassificationConfidence::Medium,
            reasons: vec![format!("pane_current_command={command}")],
        });
    }

    None
}

fn provider_from_metadata(provider: Option<&str>) -> Option<Provider> {
    let normalized = provider?.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "codex" => Some(Provider::Codex),
        "claude" => Some(Provider::Claude),
        "gemini" => Some(Provider::Gemini),
        "opencode" => Some(Provider::Opencode),
        _ => None,
    }
}

fn provider_from_title(title: &str) -> Option<Provider> {
    let title = title.trim();
    if title.is_empty() {
        return None;
    }

    let stripped = strip_known_status_glyph(title);
    if stripped.starts_with("Claude Code | ")
        || stripped.starts_with("Claude | ")
        || stripped == "Claude Code"
    {
        return Some(Provider::Claude);
    }

    if stripped.starts_with("OC | ") {
        return Some(Provider::Opencode);
    }

    if looks_like_codex_title(stripped) {
        return Some(Provider::Codex);
    }

    let lower = stripped.to_ascii_lowercase();
    if lower.contains("gemini") {
        return Some(Provider::Gemini);
    }

    None
}

fn provider_from_command(command: &str) -> Option<Provider> {
    if matches_provider_name(command, "codex") {
        return Some(Provider::Codex);
    }
    if matches_provider_name(command, "claude") {
        return Some(Provider::Claude);
    }
    if matches_provider_name(command, "gemini") {
        return Some(Provider::Gemini);
    }
    if matches_provider_name(command, "opencode") {
        return Some(Provider::Opencode);
    }

    None
}

fn infer_title_status(provider: Option<Provider>, title: &str) -> PaneStatus {
    let title = title.trim();
    let stripped = strip_known_status_glyph(title);

    if matches!(provider, Some(Provider::Claude)) {
        if has_spinner_glyph(title) {
            return PaneStatus {
                kind: StatusKind::Busy,
                source: StatusSource::TmuxTitle,
            };
        }
        if has_idle_glyph(title) {
            return PaneStatus {
                kind: StatusKind::Idle,
                source: StatusSource::TmuxTitle,
            };
        }
        if let Some(rest) = strip_claude_title_prefix(stripped) {
            if rest == "Working" || rest.starts_with("Working ") {
                return PaneStatus {
                    kind: StatusKind::Busy,
                    source: StatusSource::TmuxTitle,
                };
            }
            if rest == "Ready" || rest.starts_with("Ready ") {
                return PaneStatus {
                    kind: StatusKind::Idle,
                    source: StatusSource::TmuxTitle,
                };
            }
        }
    }

    if matches!(provider, Some(Provider::Codex)) {
        if stripped == "Working" || stripped.ends_with("| Working") {
            return PaneStatus {
                kind: StatusKind::Busy,
                source: StatusSource::TmuxTitle,
            };
        }
        if stripped == "Ready"
            || stripped == "Waiting"
            || stripped.ends_with("| Ready")
            || stripped.ends_with("| Waiting")
        {
            return PaneStatus {
                kind: StatusKind::Idle,
                source: StatusSource::TmuxTitle,
            };
        }
    }

    if matches!(provider, Some(Provider::Gemini)) {
        if title.contains("Working") {
            return PaneStatus {
                kind: StatusKind::Busy,
                source: StatusSource::TmuxTitle,
            };
        }
        if title.contains("Ready") {
            return PaneStatus {
                kind: StatusKind::Idle,
                source: StatusSource::TmuxTitle,
            };
        }
    }

    if matches!(provider, Some(Provider::Opencode))
        && let Some(rest) = stripped.strip_prefix("OC | ")
    {
        if rest == "Working" || rest.starts_with("Working ") {
            return PaneStatus {
                kind: StatusKind::Busy,
                source: StatusSource::TmuxTitle,
            };
        }
        if rest == "Ready" || rest.starts_with("Ready ") {
            return PaneStatus {
                kind: StatusKind::Idle,
                source: StatusSource::TmuxTitle,
            };
        }
    }

    PaneStatus {
        kind: StatusKind::Unknown,
        source: StatusSource::NotChecked,
    }
}

fn infer_status(title_status: PaneStatus, published_state: Option<&str>) -> PaneStatus {
    if title_status.kind != StatusKind::Unknown {
        return title_status;
    }

    match published_state.map(|value| value.trim().to_ascii_lowercase()) {
        Some(state) if state == "busy" => PaneStatus {
            kind: StatusKind::Busy,
            source: StatusSource::PaneMetadata,
        },
        Some(state) if state == "idle" => PaneStatus {
            kind: StatusKind::Idle,
            source: StatusSource::PaneMetadata,
        },
        Some(state) if state == "unknown" => PaneStatus {
            kind: StatusKind::Unknown,
            source: StatusSource::PaneMetadata,
        },
        _ => title_status,
    }
}

fn display_label(
    published_label: Option<&str>,
    raw_title: &str,
    current_command: &str,
    window_name: &str,
) -> String {
    if let Some(label) = published_label
        .map(str::trim)
        .filter(|label| !label.is_empty())
    {
        return label.to_string();
    }

    let title = raw_title.trim();
    if !title.is_empty() {
        return normalize_title_for_display(title);
    }
    if !window_name.trim().is_empty() {
        return window_name.trim().to_string();
    }

    current_command.trim().to_string()
}

fn normalize_title_for_display(title: &str) -> String {
    let stripped = strip_known_status_glyph(title).trim();
    if let Some(stripped) = strip_claude_title_prefix(stripped) {
        return stripped.to_string();
    }
    if let Some(stripped) = strip_opencode_title_prefix(stripped) {
        return stripped.to_string();
    }
    let codex_normalized = normalize_codex_invocation_in_title(stripped);
    strip_codex_args_from_title(&codex_normalized)
}

fn strip_claude_title_prefix(title: &str) -> Option<&str> {
    title
        .strip_prefix("Claude Code | ")
        .or_else(|| title.strip_prefix("Claude | "))
}

fn strip_opencode_title_prefix(title: &str) -> Option<&str> {
    title.strip_prefix("OC | ")
}

fn strip_known_status_glyph(title: &str) -> &str {
    let trimmed = title.trim_start();
    let Some(first) = trimmed.chars().next() else {
        return trimmed;
    };

    if !(CLAUDE_SPINNER_GLYPHS.contains(&first) || IDLE_GLYPHS.contains(&first)) {
        return trimmed;
    }

    let rest = &trimmed[first.len_utf8()..];
    rest.trim_start()
}

fn has_spinner_glyph(title: &str) -> bool {
    title
        .trim_start()
        .chars()
        .next()
        .is_some_and(|glyph| CLAUDE_SPINNER_GLYPHS.contains(&glyph))
}

fn has_idle_glyph(title: &str) -> bool {
    title
        .trim_start()
        .chars()
        .next()
        .is_some_and(|glyph| IDLE_GLYPHS.contains(&glyph))
}

fn normalize_codex_invocation_in_title(title: &str) -> String {
    if (title.contains("lgpt.sh") || title.ends_with(": gpt") || title.ends_with(": codex"))
        && let Some((prefix, _)) = title.rsplit_once(':')
    {
        return format!("{}: codex", prefix.trim_end());
    }

    title.to_string()
}

fn strip_codex_args_from_title(title: &str) -> String {
    if let Some((prefix, _suffix)) = title.split_once(" codex ") {
        return format!("{prefix} codex");
    }

    title.to_string()
}

fn matches_provider_name(command: &str, provider: &str) -> bool {
    command == provider
        || command.strip_prefix(provider) == Some("")
        || command
            .strip_prefix(provider)
            .is_some_and(|suffix| suffix.starts_with('-'))
}

fn looks_like_codex_title(title: &str) -> bool {
    if title.contains("lgpt.sh") {
        return true;
    }

    let Some((_, suffix)) = title.rsplit_once(':') else {
        return false;
    };

    let suffix = suffix.trim();
    suffix == "codex"
        || suffix.starts_with("codex ")
        || suffix.ends_with("/codex")
        || suffix.ends_with("/codex.sh")
}

fn tmux_version() -> Option<String> {
    let output = Command::new("tmux").arg("-V").output().ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    stdout
        .trim()
        .strip_prefix("tmux ")
        .map(|version| version.to_string())
        .or_else(|| Some(stdout.trim().to_string()))
}

fn now_rfc3339() -> Result<String> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .context("failed to format current time")
}

fn validate_snapshot(
    snapshot: &SnapshotEnvelope,
    max_age_seconds: Option<u64>,
) -> Result<CacheSummary> {
    if snapshot.schema_version != CACHE_SCHEMA_VERSION {
        bail!(
            "unsupported cache schema version {} (expected {})",
            snapshot.schema_version,
            CACHE_SCHEMA_VERSION
        );
    }

    let summary = summarize_snapshot(snapshot)?;
    if let Some(max_age_seconds) = max_age_seconds {
        let age_seconds = cache_age_seconds(summary.generated_at);
        if age_seconds > max_age_seconds {
            bail!(
                "cache is stale: age {}s exceeds max {}s",
                age_seconds,
                max_age_seconds
            );
        }
    }

    Ok(summary)
}

fn summarize_snapshot(snapshot: &SnapshotEnvelope) -> Result<CacheSummary> {
    let generated_at = OffsetDateTime::parse(&snapshot.generated_at, &Rfc3339)
        .context("generated_at was not valid RFC3339")?;

    let pane_count = snapshot.panes.len();
    let agent_pane_count = snapshot
        .panes
        .iter()
        .filter(|pane| pane.provider.is_some())
        .count();

    let provider_counts = [
        Provider::Codex,
        Provider::Claude,
        Provider::Gemini,
        Provider::Opencode,
    ]
    .into_iter()
    .filter_map(|provider| {
        let count = snapshot
            .panes
            .iter()
            .filter(|pane| pane.provider == Some(provider))
            .count();
        (count > 0).then_some((provider, count))
    })
    .collect();

    let status_counts = [StatusKind::Busy, StatusKind::Idle, StatusKind::Unknown]
        .into_iter()
        .filter_map(|status| {
            let count = snapshot
                .panes
                .iter()
                .filter(|pane| pane.status.kind == status)
                .count();
            (count > 0).then_some((status, count))
        })
        .collect();

    Ok(CacheSummary {
        generated_at,
        pane_count,
        agent_pane_count,
        provider_counts,
        status_counts,
    })
}

fn cache_age_seconds(generated_at: OffsetDateTime) -> u64 {
    let age_seconds = (OffsetDateTime::now_utc() - generated_at).whole_seconds();
    if age_seconds.is_negative() {
        0
    } else {
        age_seconds as u64
    }
}

fn daemon_cache_status(
    snapshot: &SnapshotEnvelope,
    age_seconds: u64,
    max_age_seconds: Option<u64>,
) -> DaemonCacheStatus {
    if snapshot.source.kind != SourceKind::Daemon {
        return DaemonCacheStatus::Unavailable;
    }

    if max_age_seconds.is_some_and(|max_age| age_seconds > max_age) {
        return DaemonCacheStatus::Stale;
    }

    DaemonCacheStatus::Healthy
}

fn daemon_run() -> Result<()> {
    let mut snapshot = snapshot_from_tmux()?;
    snapshot.source.kind = SourceKind::Daemon;
    write_snapshot_to_cache(&snapshot)?;

    let session_target = default_session_target()?;
    let mut child = Command::new("tmux")
        .args(["-C", "attach-session", "-t", &session_target])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("failed to start tmux control-mode client")?;

    let mut stdin = child
        .stdin
        .take()
        .context("tmux control-mode client did not provide stdin")?;
    writeln!(stdin, "refresh-client -B {DAEMON_SUBSCRIPTION_FORMAT}")
        .context("failed to subscribe to pane and metadata updates")?;
    stdin
        .flush()
        .context("failed to flush tmux control commands")?;

    let stdout = child
        .stdout
        .take()
        .context("tmux control-mode client did not provide stdout")?;
    let reader = BufReader::new(stdout);

    for line in reader.lines() {
        let line = line.context("failed to read tmux control-mode output")?;
        if should_refresh_from_notification(&line) {
            let mut snapshot = snapshot_from_tmux()?;
            snapshot.source.kind = SourceKind::Daemon;
            write_snapshot_to_cache(&snapshot)?;
        }

        if line.starts_with("%exit") {
            break;
        }
    }

    let status = child
        .wait()
        .context("failed while waiting for tmux control-mode client to exit")?;
    if !status.success() {
        bail!("tmux control-mode client exited with status {status}");
    }

    Ok(())
}

fn default_session_target() -> Result<String> {
    if env::var_os("TMUX").is_some() {
        let output = Command::new("tmux")
            .args(["display-message", "-p", "#{session_id}"])
            .output()
            .context("failed to query current tmux session")?;
        if output.status.success() {
            let stdout =
                String::from_utf8(output.stdout).context("current session was not UTF-8")?;
            let session = stdout.trim();
            if !session.is_empty() {
                return Ok(session.to_string());
            }
        }
    }

    let output = Command::new("tmux")
        .args(["list-sessions", "-F", "#{session_id}"])
        .output()
        .context("failed to list tmux sessions")?;
    if !output.status.success() {
        bail!("tmux list-sessions failed with status {}", output.status);
    }

    let stdout = String::from_utf8(output.stdout).context("tmux sessions output was not UTF-8")?;
    let session = stdout
        .lines()
        .find(|line| !line.trim().is_empty())
        .context("no tmux sessions available for daemon attach")?;
    Ok(session.trim().to_string())
}

fn current_pane_id() -> Result<Option<String>> {
    if env::var_os("TMUX").is_none() {
        return Ok(None);
    }

    let output = Command::new("tmux")
        .args(["display-message", "-p", "#{pane_id}"])
        .output()
        .context("failed to query current tmux pane id")?;
    if !output.status.success() {
        return Ok(None);
    }

    let stdout =
        String::from_utf8(output.stdout).context("current pane id output was not UTF-8")?;
    let pane_id = stdout.trim();
    if pane_id.is_empty() {
        Ok(None)
    } else {
        Ok(Some(pane_id.to_string()))
    }
}

fn resolve_tmux_target_pane(pane_id: Option<&str>, command_name: &str) -> Result<String> {
    match pane_id {
        Some(pane_id) if !pane_id.trim().is_empty() => Ok(pane_id.trim().to_string()),
        _ => current_pane_id()?
            .with_context(|| format!("`tmux {command_name}` requires --pane-id outside tmux")),
    }
}

fn current_client_tty() -> Result<Option<String>> {
    if env::var_os("TMUX").is_none() {
        return Ok(None);
    }

    let output = Command::new("tmux")
        .args(["display-message", "-p", "#{client_tty}"])
        .output()
        .context("failed to query current tmux client tty")?;
    if !output.status.success() {
        return Ok(None);
    }

    let stdout =
        String::from_utf8(output.stdout).context("current client tty output was not UTF-8")?;
    let tty = stdout.trim();
    if tty.is_empty() {
        Ok(None)
    } else {
        Ok(Some(tty.to_string()))
    }
}

fn attached_client_tty() -> Result<Option<String>> {
    let output = Command::new("tmux")
        .args(["list-clients", "-F", "#{client_tty}\x1f#{client_activity}"])
        .output()
        .context("failed to list tmux clients")?;
    if !output.status.success() {
        return Ok(None);
    }

    let stdout = String::from_utf8(output.stdout).context("tmux client output was not UTF-8")?;
    let clients = parse_tmux_client_rows(&stdout)?;
    Ok(select_best_client_tty(&clients))
}

fn parse_tmux_client_rows(input: &str) -> Result<Vec<TmuxClientRow>> {
    let mut clients = Vec::new();

    for (line_number, line) in input.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }

        let fields: Vec<_> = line.split(PANE_DELIM).collect();
        if fields.len() != 2 {
            bail!(
                "unexpected tmux client field count on line {}: expected 2, got {}",
                line_number + 1,
                fields.len()
            );
        }

        let client_tty = fields[0].trim();
        if client_tty.is_empty() {
            continue;
        }

        clients.push(TmuxClientRow {
            client_tty: client_tty.to_string(),
            client_activity: fields[1].trim().parse::<i64>().with_context(|| {
                format!(
                    "failed to parse client_activity as i64 on tmux output line {}",
                    line_number + 1
                )
            })?,
        });
    }

    Ok(clients)
}

fn select_best_client_tty(clients: &[TmuxClientRow]) -> Option<String> {
    clients
        .iter()
        .max_by_key(|client| client.client_activity)
        .map(|client| client.client_tty.clone())
}

fn default_focus_client_tty() -> Result<Option<String>> {
    if let Some(client_tty) = current_client_tty()? {
        return Ok(Some(client_tty));
    }

    attached_client_tty()
}

fn focus_tmux_pane(pane_id: &str, client_tty: Option<&str>) -> Result<()> {
    let client_tty = match client_tty {
        Some(tty) if !tty.trim().is_empty() => Some(tty.trim().to_string()),
        _ => default_focus_client_tty()?,
    };

    let status = if let Some(client_tty) = client_tty.as_deref() {
        let status = Command::new("tmux")
            .args(["switch-client", "-Z", "-c", client_tty, "-t", pane_id])
            .status()
            .context("failed to execute tmux switch-client with client tty")?;
        if status.success() {
            status
        } else {
            Command::new("tmux")
                .args(["switch-client", "-c", client_tty, "-t", pane_id])
                .status()
                .context("failed to execute tmux switch-client fallback with client tty")?
        }
    } else {
        let status = Command::new("tmux")
            .args(["switch-client", "-Z", "-t", pane_id])
            .status()
            .context("failed to execute tmux switch-client")?;
        if status.success() {
            status
        } else {
            Command::new("tmux")
                .args(["switch-client", "-t", pane_id])
                .status()
                .context("failed to execute tmux switch-client fallback")?
        }
    };

    if !status.success() {
        bail!("tmux switch-client failed with status {status}");
    }

    Ok(())
}

fn should_refresh_from_notification(line: &str) -> bool {
    matches!(
        notification_name(line),
        Some(
            "%subscription-changed"
                | "%sessions-changed"
                | "%session-changed"
                | "%session-renamed"
                | "%session-window-changed"
                | "%layout-change"
                | "%window-add"
                | "%window-close"
                | "%window-pane-changed"
                | "%window-renamed"
        )
    )
}

fn notification_name(line: &str) -> Option<&str> {
    line.split_whitespace()
        .next()
        .filter(|token| token.starts_with('%'))
}

fn cache_path() -> Result<PathBuf> {
    if let Some(path) = env::var_os(CACHE_ENV_VAR) {
        return Ok(PathBuf::from(path));
    }

    if let Some(cache_home) = env::var_os("XDG_CACHE_HOME") {
        return Ok(PathBuf::from(cache_home).join(CACHE_RELATIVE_PATH));
    }

    let home = env::var_os("HOME").context("HOME is not set and no cache override was provided")?;
    Ok(Path::new(&home).join(".cache").join(CACHE_RELATIVE_PATH))
}

fn status_kind_name(status: StatusKind) -> &'static str {
    match status {
        StatusKind::Idle => "idle",
        StatusKind::Busy => "busy",
        StatusKind::Unknown => "unknown",
    }
}

fn daemon_cache_status_name(status: DaemonCacheStatus) -> &'static str {
    match status {
        DaemonCacheStatus::Healthy => "healthy",
        DaemonCacheStatus::Stale => "stale",
        DaemonCacheStatus::Unavailable => "unavailable",
    }
}

fn tmux_metadata_updates(args: &TmuxSetMetadataArgs) -> Vec<(&'static str, String)> {
    let mut updates = Vec::new();

    if let Some(provider) = args.provider {
        updates.push(("@agent.provider", provider.to_string()));
    }
    if let Some(label) = args
        .label
        .as_deref()
        .map(str::trim)
        .filter(|label| !label.is_empty())
    {
        updates.push(("@agent.label", label.to_string()));
    }
    if let Some(cwd) = args
        .cwd
        .as_deref()
        .map(str::trim)
        .filter(|cwd| !cwd.is_empty())
    {
        updates.push(("@agent.cwd", cwd.to_string()));
    }
    if let Some(state) = args.state {
        updates.push(("@agent.state", status_kind_name(state).to_string()));
    }
    if let Some(session_id) = args
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|session_id| !session_id.is_empty())
    {
        updates.push(("@agent.session_id", session_id.to_string()));
    }

    updates
}

fn tmux_metadata_fields_to_clear(fields: &[TmuxMetadataField]) -> Vec<&'static str> {
    if fields.is_empty() {
        return vec![
            "@agent.provider",
            "@agent.label",
            "@agent.cwd",
            "@agent.state",
            "@agent.session_id",
        ];
    }

    fields
        .iter()
        .map(|field| match field {
            TmuxMetadataField::Provider => "@agent.provider",
            TmuxMetadataField::Label => "@agent.label",
            TmuxMetadataField::Cwd => "@agent.cwd",
            TmuxMetadataField::State => "@agent.state",
            TmuxMetadataField::SessionId => "@agent.session_id",
        })
        .collect()
}

fn set_tmux_pane_option(pane_id: &str, option_name: &str, value: &str) -> Result<()> {
    let status = Command::new("tmux")
        .args(["set-option", "-p", "-t", pane_id, option_name, value])
        .status()
        .with_context(|| format!("failed to set tmux option {option_name} on {pane_id}"))?;
    if !status.success() {
        bail!("tmux set-option failed for {option_name} on {pane_id}");
    }

    Ok(())
}

fn unset_tmux_pane_option(pane_id: &str, option_name: &str) -> Result<()> {
    let status = Command::new("tmux")
        .args(["set-option", "-p", "-u", "-t", pane_id, option_name])
        .status()
        .with_context(|| format!("failed to clear tmux option {option_name} on {pane_id}"))?;
    if !status.success() {
        bail!("tmux set-option -u failed for {option_name} on {pane_id}");
    }

    Ok(())
}

fn format_provider_counts(counts: &[(Provider, usize)]) -> String {
    if counts.is_empty() {
        return "none".to_string();
    }

    counts
        .iter()
        .map(|(provider, count)| format!("{provider}={count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_status_counts(counts: &[(StatusKind, usize)]) -> String {
    if counts.is_empty() {
        return "none".to_string();
    }

    counts
        .iter()
        .map(|(status, count)| format!("{}={count}", status_kind_name(*status)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn tsv_escape(value: &str) -> String {
    value.replace(['\t', '\n', '\r'], " ")
}

fn default_if_empty<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.trim().is_empty() {
        fallback
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use anyhow::Context;
    use proptest::{prelude::*, string::string_regex};

    const TMUX_SNAPSHOT_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/tmux_snapshot_titles.txt"
    ));
    const CACHE_SNAPSHOT_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/cache_snapshot_v1.json"
    ));
    const TMUX_METADATA_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/tmux_snapshot_with_metadata.txt"
    ));

    #[allow(unused_imports)]
    use super::{
        CACHE_RELATIVE_PATH, CACHE_SCHEMA_VERSION, CLAUDE_SPINNER_GLYPHS, ClassificationMatchKind,
        Cli, DAEMON_SUBSCRIPTION_FORMAT, IDLE_GLYPHS, PaneRecord, Provider, SnapshotEnvelope,
        SourceKind, StatusKind, TmuxMetadataField, classify_provider, daemon_cache_status,
        daemon_cache_status_name, infer_status, infer_title_status, looks_like_codex_title,
        normalize_title_for_display, notification_name, pane_from_row, parse_pane_rows,
        parse_tmux_client_rows, popup_entries, select_best_client_tty,
        should_refresh_from_notification, status_kind_name, strip_known_status_glyph,
        summarize_snapshot, tmux_metadata_fields_to_clear, tmux_metadata_updates, tsv_escape,
        validate_snapshot,
    };

    #[test]
    fn classifies_from_command() {
        let matched = classify_provider(None, "codex", "").expect("should match codex");
        assert_eq!(matched.provider, Provider::Codex);
        assert_eq!(
            matched.matched_by,
            ClassificationMatchKind::PaneCurrentCommand
        );
    }

    #[test]
    fn classifies_from_title_before_command() {
        let matched =
            classify_provider(None, "zsh", "Claude Code | Working").expect("should match claude");
        assert_eq!(matched.provider, Provider::Claude);
        assert_eq!(matched.matched_by, ClassificationMatchKind::PaneTitle);
    }

    #[test]
    fn classifies_from_pane_metadata_before_title_and_command() {
        let matched = classify_provider(Some("codex"), "zsh", "Claude Code | Working")
            .expect("pane metadata should match codex");
        assert_eq!(matched.provider, Provider::Codex);
        assert_eq!(matched.matched_by, ClassificationMatchKind::PaneMetadata);
    }

    #[test]
    fn parses_tmux_output_into_rows() {
        let input = concat!(
            "dotfiles\x1f1\x1f1\x1f%50\x1f438455\x1fcodex\x1f(bront) .dotfiles: codex\x1f/dev/pts/55\x1f/home/auro/.dotfiles\x1feditor\n",
            "notes\x1f4\x1f1\x1f%41\x1f324026\x1fclaude\x1fClaude Code\x1f/dev/pts/44\x1f/home/auro/notes\x1fquery\n"
        );

        let rows = parse_pane_rows(input).expect("tmux output should parse");

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].pane_id, "%50");
        assert_eq!(rows[1].pane_title_raw, "Claude Code");
    }

    #[test]
    fn parses_tmux_client_rows_and_selects_most_recent_tty() {
        let input = concat!(
            "/dev/pts/5\x1f1711671000\n",
            "/dev/pts/7\x1f1711672000\n",
            "\x1f1711673000\n"
        );

        let clients = parse_tmux_client_rows(input).expect("tmux client output should parse");
        assert_eq!(clients.len(), 2);
        assert_eq!(clients[0].client_tty, "/dev/pts/5");
        assert_eq!(
            select_best_client_tty(&clients),
            Some("/dev/pts/7".to_string())
        );
    }

    #[test]
    fn pane_record_uses_canonical_shape() {
        let pane = pane_from_row(super::TmuxPaneRow {
            session_name: "notes".to_string(),
            window_index: 4,
            pane_index: 1,
            pane_id: "%41".to_string(),
            pane_pid: 324026,
            pane_current_command: "claude".to_string(),
            pane_title_raw: "Claude Code | Query".to_string(),
            pane_tty: "/dev/pts/44".to_string(),
            pane_current_path: "/home/auro/notes".to_string(),
            window_name: "ai".to_string(),
            agent_provider: None,
            agent_label: None,
            agent_cwd: None,
            agent_state: None,
            agent_session_id: None,
        });

        assert_eq!(pane.provider, Some(Provider::Claude));
        assert_eq!(pane.location.session_name, "notes");
        assert_eq!(pane.display.label, "Query");
    }

    #[test]
    fn daemon_notifications_trigger_refresh() {
        assert!(should_refresh_from_notification("%window-add @1"));
        assert!(should_refresh_from_notification(
            "%subscription-changed agentscan %1 : value"
        ));
        assert!(!should_refresh_from_notification("%begin 1 1 0"));
    }

    #[test]
    fn daemon_subscription_format_includes_wrapper_metadata_fields() {
        assert!(DAEMON_SUBSCRIPTION_FORMAT.contains("#{{pane_title}}"));
        assert!(DAEMON_SUBSCRIPTION_FORMAT.contains("#{{@agent.provider}}"));
        assert!(DAEMON_SUBSCRIPTION_FORMAT.contains("#{{@agent.state}}"));
        assert!(DAEMON_SUBSCRIPTION_FORMAT.contains("#{{@agent.session_id}}"));
    }

    #[test]
    fn detects_notification_names() {
        assert_eq!(
            notification_name("%window-renamed @1 editor"),
            Some("%window-renamed")
        );
        assert_eq!(notification_name("plain output"), None);
    }

    #[test]
    fn infers_status_from_title_only() {
        let status = infer_title_status(Some(Provider::Gemini), "Working");
        assert_eq!(status.kind, StatusKind::Busy);
    }

    #[test]
    fn codex_status_uses_title_only() {
        let busy = infer_title_status(Some(Provider::Codex), "⠹ agentscan | Working");
        let idle = infer_title_status(Some(Provider::Codex), "Ready");

        assert_eq!(busy.kind, StatusKind::Busy);
        assert_eq!(idle.kind, StatusKind::Idle);
    }

    #[test]
    fn claude_status_distinguishes_spinner_and_idle_marker() {
        let busy = infer_title_status(Some(Provider::Claude), "⠏ Building summary");
        let idle = infer_title_status(Some(Provider::Claude), "✳ Review and summarize todo list");

        assert_eq!(busy.kind, StatusKind::Busy);
        assert_eq!(idle.kind, StatusKind::Idle);
    }

    #[test]
    fn claude_status_uses_textual_titles_without_spinner_glyphs() {
        let busy = infer_title_status(Some(Provider::Claude), "Claude Code | Working");
        let idle = infer_title_status(Some(Provider::Claude), "Claude Code | Ready");
        let unknown = infer_title_status(Some(Provider::Claude), "Claude Code | Query");

        assert_eq!(busy.kind, StatusKind::Busy);
        assert_eq!(idle.kind, StatusKind::Idle);
        assert_eq!(unknown.kind, StatusKind::Unknown);
    }

    #[test]
    fn opencode_status_uses_title_prefix_when_present() {
        let busy = infer_title_status(Some(Provider::Opencode), "OC | Working");
        let idle = infer_title_status(Some(Provider::Opencode), "OC | Ready");
        let unknown = infer_title_status(Some(Provider::Opencode), "OC | Query planner");

        assert_eq!(busy.kind, StatusKind::Busy);
        assert_eq!(idle.kind, StatusKind::Idle);
        assert_eq!(unknown.kind, StatusKind::Unknown);
    }

    #[test]
    fn metadata_state_fills_unknown_status_without_overriding_title_signal() {
        let unknown_from_title = infer_title_status(Some(Provider::Codex), "(bront) repo: codex");
        let busy_from_metadata = infer_status(unknown_from_title, Some("busy"));
        assert_eq!(busy_from_metadata.kind, StatusKind::Busy);

        let idle_from_title = infer_title_status(Some(Provider::Codex), "Ready");
        let still_idle = infer_status(idle_from_title, Some("busy"));
        assert_eq!(still_idle.kind, StatusKind::Idle);
    }

    #[test]
    fn tmux_metadata_updates_emit_expected_option_values() {
        let args = super::TmuxSetMetadataArgs {
            pane_id: Some("%41".to_string()),
            provider: Some(Provider::Claude),
            label: Some("Review notes".to_string()),
            cwd: Some("/tmp/notes".to_string()),
            state: Some(StatusKind::Busy),
            session_id: Some("sess-123".to_string()),
        };

        let updates = tmux_metadata_updates(&args);
        assert_eq!(
            updates,
            vec![
                ("@agent.provider", "claude".to_string()),
                ("@agent.label", "Review notes".to_string()),
                ("@agent.cwd", "/tmp/notes".to_string()),
                ("@agent.state", "busy".to_string()),
                ("@agent.session_id", "sess-123".to_string()),
            ]
        );
    }

    #[test]
    fn tmux_metadata_fields_to_clear_defaults_to_all_fields() {
        assert_eq!(
            tmux_metadata_fields_to_clear(&[]),
            vec![
                "@agent.provider",
                "@agent.label",
                "@agent.cwd",
                "@agent.state",
                "@agent.session_id",
            ]
        );
    }

    #[test]
    fn tmux_metadata_fields_to_clear_maps_selected_fields() {
        assert_eq!(
            tmux_metadata_fields_to_clear(&[
                TmuxMetadataField::Provider,
                TmuxMetadataField::State,
                TmuxMetadataField::SessionId,
            ]),
            vec!["@agent.provider", "@agent.state", "@agent.session_id"]
        );
    }

    #[test]
    fn detects_codex_titles() {
        assert!(looks_like_codex_title("(repo) task: codex"));
        assert!(looks_like_codex_title(
            "(repo) task: /home/auro/.zshrc.d/scripts/lgpt.sh"
        ));
        assert!(!looks_like_codex_title("(repo) task: shell"));
    }

    #[test]
    fn cache_path_uses_override_when_present() {
        let actual = cache_path_for_test(Some("/tmp/agentscan-cache.json"), None, None)
            .expect("override path should work");
        assert_eq!(actual, PathBuf::from("/tmp/agentscan-cache.json"));
    }

    #[test]
    fn cache_path_defaults_to_xdg_location() {
        let actual = cache_path_for_test(None, Some("/tmp/cache"), Some("/tmp/home"))
            .expect("xdg cache path should work");
        assert_eq!(
            actual,
            PathBuf::from("/tmp/cache").join(CACHE_RELATIVE_PATH)
        );
    }

    #[test]
    fn source_kind_supports_daemon() {
        assert_eq!(
            serde_json::to_string(&SourceKind::Daemon).unwrap(),
            "\"daemon\""
        );
    }

    #[test]
    fn daemon_cache_status_reports_health_states() {
        let mut snapshot: SnapshotEnvelope =
            serde_json::from_str(CACHE_SNAPSHOT_FIXTURE).expect("cache fixture should parse");

        assert_eq!(
            daemon_cache_status(&snapshot, 10, Some(60)),
            super::DaemonCacheStatus::Healthy
        );
        assert_eq!(
            daemon_cache_status_name(super::DaemonCacheStatus::Healthy),
            "healthy"
        );

        snapshot.source.kind = SourceKind::Snapshot;
        assert_eq!(
            daemon_cache_status(&snapshot, 10, Some(60)),
            super::DaemonCacheStatus::Unavailable
        );

        snapshot.source.kind = SourceKind::Daemon;
        assert_eq!(
            daemon_cache_status(&snapshot, 120, Some(60)),
            super::DaemonCacheStatus::Stale
        );
    }

    #[test]
    fn popup_entries_include_location_and_status() {
        let pane = pane_from_row(super::TmuxPaneRow {
            session_name: "notes".to_string(),
            window_index: 4,
            pane_index: 1,
            pane_id: "%41".to_string(),
            pane_pid: 324026,
            pane_current_command: "claude".to_string(),
            pane_title_raw: "Working".to_string(),
            pane_tty: "/dev/pts/44".to_string(),
            pane_current_path: "/home/auro/notes".to_string(),
            window_name: "ai".to_string(),
            agent_provider: None,
            agent_label: None,
            agent_cwd: None,
            agent_state: None,
            agent_session_id: None,
        });

        let entries = popup_entries(&[pane]);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_name, "notes");
    }

    #[test]
    fn fixture_snapshot_parses_expected_provider_cases() {
        let rows = parse_pane_rows(TMUX_SNAPSHOT_FIXTURE).expect("fixture snapshot should parse");
        let panes: Vec<_> = rows.into_iter().map(pane_from_row).collect();

        let codex_working = pane_by_id(&panes, "%191");
        assert_eq!(codex_working.provider, Some(Provider::Codex));
        assert_eq!(codex_working.status.kind, StatusKind::Busy);
        assert_eq!(codex_working.display.label, "agentscan | Working");

        let codex_ready = pane_by_id(&panes, "%67");
        assert_eq!(codex_ready.status.kind, StatusKind::Idle);

        let codex_waiting = pane_by_id(&panes, "%194");
        assert_eq!(codex_waiting.provider, Some(Provider::Codex));
        assert_eq!(codex_waiting.status.kind, StatusKind::Idle);
        assert_eq!(codex_waiting.display.label, "agentscan | Waiting");

        let claude_idle = pane_by_id(&panes, "%41");
        assert_eq!(claude_idle.provider, Some(Provider::Claude));
        assert_eq!(claude_idle.status.kind, StatusKind::Idle);
        assert_eq!(claude_idle.display.label, "Review and summarize todo list");

        let claude_busy = pane_by_id(&panes, "%223");
        assert_eq!(claude_busy.status.kind, StatusKind::Busy);

        let claude_title_busy = pane_by_id(&panes, "%224");
        assert_eq!(claude_title_busy.provider, Some(Provider::Claude));
        assert_eq!(claude_title_busy.status.kind, StatusKind::Busy);
        assert_eq!(claude_title_busy.display.label, "Working");

        let claude_title_idle = pane_by_id(&panes, "%225");
        assert_eq!(claude_title_idle.provider, Some(Provider::Claude));
        assert_eq!(claude_title_idle.status.kind, StatusKind::Idle);
        assert_eq!(claude_title_idle.display.label, "Ready");

        let opencode = pane_by_id(&panes, "%301");
        assert_eq!(opencode.provider, Some(Provider::Opencode));
        assert_eq!(opencode.display.label, "Query planner");
    }

    #[test]
    fn fixture_snapshot_preserves_wrapper_prefixes() {
        let rows = parse_pane_rows(TMUX_SNAPSHOT_FIXTURE).expect("fixture snapshot should parse");
        let panes: Vec<_> = rows.into_iter().map(pane_from_row).collect();

        let wrapped_codex = pane_by_id(&panes, "%89");
        assert_eq!(wrapped_codex.provider, Some(Provider::Codex));
        assert_eq!(wrapped_codex.display.label, "(bront) parallel-n64: codex");
        assert_eq!(wrapped_codex.status.kind, StatusKind::Unknown);
    }

    #[test]
    fn pane_metadata_overrides_display_provider_and_status_when_title_is_ambiguous() {
        let pane = pane_from_row(super::TmuxPaneRow {
            session_name: "wrapper".to_string(),
            window_index: 1,
            pane_index: 1,
            pane_id: "%500".to_string(),
            pane_pid: 500,
            pane_current_command: "zsh".to_string(),
            pane_title_raw: "(bront) ~/code/wrapper".to_string(),
            pane_tty: "/dev/pts/500".to_string(),
            pane_current_path: "/home/auro/code/wrapper".to_string(),
            window_name: "ai".to_string(),
            agent_provider: Some("claude".to_string()),
            agent_label: Some("Wrapper Claude Task".to_string()),
            agent_cwd: Some("/tmp/wrapper".to_string()),
            agent_state: Some("idle".to_string()),
            agent_session_id: Some("sess-123".to_string()),
        });

        assert_eq!(pane.provider, Some(Provider::Claude));
        assert_eq!(pane.display.label, "Wrapper Claude Task");
        assert_eq!(pane.status.kind, StatusKind::Idle);
        assert_eq!(pane.status.source, super::StatusSource::PaneMetadata);
        assert_eq!(
            pane.classification.matched_by,
            Some(ClassificationMatchKind::PaneMetadata)
        );
        assert_eq!(pane.agent_metadata.provider.as_deref(), Some("claude"));
        assert_eq!(pane.agent_metadata.session_id.as_deref(), Some("sess-123"));
    }

    #[test]
    fn fixture_snapshot_with_metadata_parses_wrapper_fields() {
        let rows = parse_pane_rows(TMUX_METADATA_FIXTURE).expect("metadata fixture should parse");
        let panes: Vec<_> = rows.into_iter().map(pane_from_row).collect();

        let pane = pane_by_id(&panes, "%400");
        assert_eq!(pane.provider, Some(Provider::Claude));
        assert_eq!(pane.display.label, "Wrapped Claude Task");
        assert_eq!(pane.status.kind, StatusKind::Busy);
        assert_eq!(pane.status.source, super::StatusSource::PaneMetadata);
        assert_eq!(
            pane.agent_metadata.cwd.as_deref(),
            Some("/tmp/wrapper-meta")
        );
        assert_eq!(pane.agent_metadata.session_id.as_deref(), Some("sess-123"));
    }

    #[test]
    fn cache_fixture_deserializes_into_current_schema() {
        let snapshot: SnapshotEnvelope =
            serde_json::from_str(CACHE_SNAPSHOT_FIXTURE).expect("cache fixture should parse");

        assert_eq!(snapshot.schema_version, CACHE_SCHEMA_VERSION);
        assert_eq!(snapshot.source.kind, SourceKind::Daemon);
        assert_eq!(snapshot.panes.len(), 1);
        assert_eq!(snapshot.panes[0].pane_id, "%67");
        assert_eq!(snapshot.panes[0].status.kind, StatusKind::Idle);
    }

    #[test]
    fn cache_summary_counts_fixture_contents() {
        let snapshot: SnapshotEnvelope =
            serde_json::from_str(CACHE_SNAPSHOT_FIXTURE).expect("cache fixture should parse");

        let summary = summarize_snapshot(&snapshot).expect("cache fixture should summarize");
        assert_eq!(summary.pane_count, 1);
        assert_eq!(summary.agent_pane_count, 1);
        assert_eq!(summary.provider_counts, vec![(Provider::Codex, 1)]);
        assert_eq!(summary.status_counts, vec![(StatusKind::Idle, 1)]);
    }

    #[test]
    fn validate_snapshot_rejects_unsupported_schema_version() {
        let mut snapshot: SnapshotEnvelope =
            serde_json::from_str(CACHE_SNAPSHOT_FIXTURE).expect("cache fixture should parse");
        snapshot.schema_version = CACHE_SCHEMA_VERSION + 1;

        let error = validate_snapshot(&snapshot, None).expect_err("schema mismatch should fail");
        assert!(
            error
                .to_string()
                .contains("unsupported cache schema version"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn validate_snapshot_rejects_stale_cache_when_max_age_is_exceeded() {
        let mut snapshot: SnapshotEnvelope =
            serde_json::from_str(CACHE_SNAPSHOT_FIXTURE).expect("cache fixture should parse");
        snapshot.generated_at = "2020-01-01T00:00:00Z".to_string();

        let error = validate_snapshot(&snapshot, Some(1)).expect_err("stale cache should fail");
        assert!(
            error.to_string().contains("cache is stale"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn tsv_escape_removes_control_whitespace() {
        assert_eq!(tsv_escape("a\tb\nc\rd"), "a b c d");
    }

    #[test]
    fn status_names_match_serialized_values() {
        assert_eq!(status_kind_name(StatusKind::Busy), "busy");
        assert_eq!(status_kind_name(StatusKind::Idle), "idle");
        assert_eq!(status_kind_name(StatusKind::Unknown), "unknown");
    }

    #[test]
    fn known_status_glyph_stripping_preserves_normal_prefixes() {
        assert_eq!(
            strip_known_status_glyph("(bront) parallel-n64: codex"),
            "(bront) parallel-n64: codex"
        );
        assert_eq!(
            strip_known_status_glyph("✳ Review and summarize todo list"),
            "Review and summarize todo list"
        );
    }

    #[test]
    fn title_normalization_strips_claude_and_opencode_prefixes() {
        assert_eq!(normalize_title_for_display("Claude Code | Query"), "Query");
        assert_eq!(normalize_title_for_display("Claude | Ready"), "Ready");
        assert_eq!(
            normalize_title_for_display("OC | Query planner"),
            "Query planner"
        );
    }

    #[test]
    fn cli_refresh_flag_is_global() {
        let cli = <Cli as clap::Parser>::parse_from(["agentscan", "-f", "list"]);
        assert!(cli.refresh);

        let cli = <Cli as clap::Parser>::parse_from(["agentscan", "list", "-f"]);
        assert!(cli.refresh);

        let cli = <Cli as clap::Parser>::parse_from(["agentscan", "-f"]);
        assert!(cli.refresh);
    }

    fn cache_path_for_test(
        override_path: Option<&str>,
        xdg_cache_home: Option<&str>,
        home: Option<&str>,
    ) -> Result<PathBuf, anyhow::Error> {
        if let Some(path) = override_path {
            return Ok(PathBuf::from(path));
        }

        if let Some(cache_home) = xdg_cache_home {
            return Ok(PathBuf::from(cache_home).join(CACHE_RELATIVE_PATH));
        }

        let home = home.context("missing home")?;
        Ok(Path::new(home).join(".cache").join(CACHE_RELATIVE_PATH))
    }

    proptest! {
        #[test]
        fn parse_pane_rows_roundtrips_generated_rows(
            session_name in safe_tmux_field(),
            window_index in 0_u32..1000,
            pane_index in 0_u32..1000,
            pane_pid in 1_u32..u32::MAX,
            pane_current_command in safe_tmux_field(),
            pane_title_raw in safe_tmux_field(),
            pane_tty in safe_tmux_field(),
            pane_current_path in safe_tmux_field(),
            window_name in safe_tmux_field(),
        ) {
            let pane_id = format!("%{pane_pid}");
            let line = format!(
                "{session_name}\u{1f}{window_index}\u{1f}{pane_index}\u{1f}{pane_id}\u{1f}{pane_pid}\u{1f}{pane_current_command}\u{1f}{pane_title_raw}\u{1f}{pane_tty}\u{1f}{pane_current_path}\u{1f}{window_name}"
            );

            let rows = parse_pane_rows(&line).expect("generated tmux row should parse");
            prop_assert_eq!(rows.len(), 1);

            let row = &rows[0];
            prop_assert_eq!(&row.session_name, &session_name);
            prop_assert_eq!(row.window_index, window_index);
            prop_assert_eq!(row.pane_index, pane_index);
            prop_assert_eq!(&row.pane_id, &pane_id);
            prop_assert_eq!(row.pane_pid, pane_pid);
            prop_assert_eq!(&row.pane_current_command, &pane_current_command);
            prop_assert_eq!(&row.pane_title_raw, &pane_title_raw);
            prop_assert_eq!(&row.pane_tty, &pane_tty);
            prop_assert_eq!(&row.pane_current_path, &pane_current_path);
            prop_assert_eq!(&row.window_name, &window_name);
        }

        #[test]
        fn tsv_escape_is_idempotent_and_removes_control_whitespace(value in any::<String>()) {
            let escaped = tsv_escape(&value);

            prop_assert!(!escaped.contains('\t'));
            prop_assert!(!escaped.contains('\n'));
            prop_assert!(!escaped.contains('\r'));
            prop_assert_eq!(tsv_escape(&escaped), escaped);
        }

        #[test]
        fn known_status_glyphs_strip_to_trimmed_tail(
            glyph in known_status_glyph(),
            padding in 0_usize..4,
            tail in any::<String>(),
        ) {
            let input = format!("{glyph}{}{tail}", " ".repeat(padding));
            prop_assert_eq!(strip_known_status_glyph(&input), tail.trim_start());
        }
    }

    fn safe_tmux_field() -> impl Strategy<Value = String> {
        string_regex(r"[A-Za-z0-9_./()|: -]{0,32}").expect("safe tmux field regex should compile")
    }

    fn known_status_glyph() -> impl Strategy<Value = char> {
        prop::sample::select(
            CLAUDE_SPINNER_GLYPHS
                .iter()
                .copied()
                .chain(IDLE_GLYPHS.iter().copied())
                .collect::<Vec<_>>(),
        )
    }

    fn pane_by_id<'a>(panes: &'a [PaneRecord], pane_id: &str) -> &'a PaneRecord {
        panes
            .iter()
            .find(|pane| pane.pane_id == pane_id)
            .unwrap_or_else(|| panic!("missing pane fixture entry {pane_id}"))
    }
}
