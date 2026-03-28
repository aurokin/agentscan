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
    "#{window_name}"
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
}

#[derive(Subcommand, Debug)]
enum DaemonCommands {
    /// Run the long-lived daemon loop.
    Run,
}

#[derive(Subcommand, Debug)]
enum TmuxCommands {
    /// Emit popup-oriented pane output.
    Popup(TmuxPopupArgs),
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum StatusKind {
    Idle,
    Busy,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum StatusSource {
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ClassificationMatchKind {
    PaneCurrentCommand,
    PaneTitle,
}

#[derive(Debug, Deserialize, Serialize)]
struct SnapshotEnvelope {
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
struct PaneRecord {
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
}

#[derive(Debug, Deserialize, Serialize)]
struct PaneDiagnostics {
    cache_origin: String,
}

#[derive(Debug, Serialize)]
struct PopupEntry {
    pane_id: String,
    provider: Option<Provider>,
    status: StatusKind,
    session_name: String,
    window_index: u32,
    pane_index: u32,
    display_label: String,
}

#[derive(Debug)]
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
}

#[derive(Debug)]
struct ProviderMatch {
    provider: Provider,
    matched_by: ClassificationMatchKind,
    confidence: ClassificationConfidence,
    reasons: Vec<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Scan(args)) => command_scan(&args),
        Some(Commands::List(args)) => command_list(&args),
        Some(Commands::Inspect(args)) => command_inspect(&args),
        Some(Commands::Focus(args)) => command_focus(&args),
        Some(Commands::Daemon(args)) => command_daemon(&args),
        Some(Commands::Tmux(args)) => command_tmux(&args),
        Some(Commands::Cache(args)) => command_cache(&args),
        None => command_list(&cli.list_args),
    }
}

fn command_scan(args: &ListArgs) -> Result<()> {
    let mut snapshot = snapshot_from_tmux()?;
    filter_snapshot(&mut snapshot, args.all);
    emit_snapshot(&snapshot, args.format)
}

fn command_list(args: &ListArgs) -> Result<()> {
    let mut snapshot = read_snapshot_from_cache()?;
    filter_snapshot(&mut snapshot, args.all);
    emit_snapshot(&snapshot, args.format)
}

fn command_inspect(args: &InspectArgs) -> Result<()> {
    let snapshot = read_snapshot_from_cache()?;
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

fn command_focus(args: &FocusArgs) -> Result<()> {
    focus_tmux_pane(&args.pane_id, args.client_tty.as_deref())
}

fn command_daemon(args: &DaemonArgs) -> Result<()> {
    match args.command {
        DaemonCommands::Run => daemon_run(),
    }
}

fn command_cache(args: &CacheArgs) -> Result<()> {
    match args.command {
        CacheCommands::Path => {
            println!("{}", cache_path()?.display());
        }
    }

    Ok(())
}

fn command_tmux(args: &TmuxArgs) -> Result<()> {
    match &args.command {
        TmuxCommands::Popup(args) => command_tmux_popup(args),
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

fn command_tmux_popup(args: &TmuxPopupArgs) -> Result<()> {
    let mut snapshot = read_snapshot_from_cache()?;
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

fn snapshot_from_tmux() -> Result<SnapshotEnvelope> {
    let rows = tmux_list_panes()?;
    let panes = rows.into_iter().map(pane_from_row).collect();

    Ok(SnapshotEnvelope {
        schema_version: 1,
        generated_at: now_rfc3339()?,
        source: SnapshotSource {
            kind: SourceKind::Snapshot,
            tmux_version: tmux_version(),
        },
        panes,
    })
}

fn read_snapshot_from_cache() -> Result<SnapshotEnvelope> {
    let path = cache_path()?;
    let contents = fs::read_to_string(&path).with_context(|| {
        format!(
            "failed to read cache at {}. Run `agentscan daemon run` first",
            path.display()
        )
    })?;

    serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse cache at {}", path.display()))
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

fn popup_entries(panes: &[PaneRecord]) -> Vec<PopupEntry> {
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

fn pane_from_row(row: TmuxPaneRow) -> PaneRecord {
    let provider_match = classify_provider(&row.pane_current_command, &row.pane_title_raw);
    let provider = provider_match.as_ref().map(|matched| matched.provider);

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
                &row.pane_title_raw,
                &row.pane_current_command,
                &row.window_name,
            ),
            activity_label: None,
        },
        provider,
        status: infer_status(provider, &row.pane_title_raw),
        classification: PaneClassification {
            matched_by: provider_match.as_ref().map(|matched| matched.matched_by),
            confidence: provider_match.as_ref().map(|matched| matched.confidence),
            reasons: provider_match
                .map(|matched| matched.reasons)
                .unwrap_or_default(),
        },
        agent_metadata: AgentMetadata::default(),
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

fn parse_pane_rows(input: &str) -> Result<Vec<TmuxPaneRow>> {
    let mut panes = Vec::new();

    for (line_number, line) in input.lines().enumerate() {
        if line.is_empty() {
            continue;
        }

        let fields: Vec<_> = line.split(PANE_DELIM).collect();
        if fields.len() != 10 {
            bail!(
                "unexpected tmux pane field count on line {}: expected 10, got {}",
                line_number + 1,
                fields.len()
            );
        }

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
        });
    }

    Ok(panes)
}

fn parse_u32(value: &str, field_name: &str, line_number: usize) -> Result<u32> {
    value.parse::<u32>().with_context(|| {
        format!("failed to parse {field_name} as u32 on tmux output line {line_number}")
    })
}

fn classify_provider(command: &str, title: &str) -> Option<ProviderMatch> {
    let title = title.trim();
    let command = command.trim();

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

fn infer_status(provider: Option<Provider>, title: &str) -> PaneStatus {
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

    PaneStatus {
        kind: StatusKind::Unknown,
        source: StatusSource::NotChecked,
    }
}

fn display_label(raw_title: &str, current_command: &str, window_name: &str) -> String {
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
    let codex_normalized = normalize_codex_invocation_in_title(stripped);
    strip_codex_args_from_title(&codex_normalized)
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
    writeln!(
        stdin,
        "refresh-client -B agentscan:%*:#{{pane_id}}:#{{pane_title}}"
    )
    .context("failed to subscribe to pane title updates")?;
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

fn focus_tmux_pane(pane_id: &str, client_tty: Option<&str>) -> Result<()> {
    let client_tty = match client_tty {
        Some(tty) if !tty.trim().is_empty() => Some(tty.trim().to_string()),
        _ => current_client_tty()?,
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

    const TMUX_SNAPSHOT_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/tmux_snapshot_titles.txt"
    ));
    const CACHE_SNAPSHOT_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/cache_snapshot_v1.json"
    ));

    use super::{
        CACHE_RELATIVE_PATH, ClassificationMatchKind, PaneRecord, Provider, SnapshotEnvelope,
        SourceKind, StatusKind, classify_provider, infer_status, looks_like_codex_title,
        notification_name, pane_from_row, parse_pane_rows, popup_entries,
        should_refresh_from_notification, status_kind_name, strip_known_status_glyph, tsv_escape,
    };

    #[test]
    fn classifies_from_command() {
        let matched = classify_provider("codex", "").expect("should match codex");
        assert_eq!(matched.provider, Provider::Codex);
        assert_eq!(
            matched.matched_by,
            ClassificationMatchKind::PaneCurrentCommand
        );
    }

    #[test]
    fn classifies_from_title_before_command() {
        let matched =
            classify_provider("zsh", "Claude Code | Working").expect("should match claude");
        assert_eq!(matched.provider, Provider::Claude);
        assert_eq!(matched.matched_by, ClassificationMatchKind::PaneTitle);
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
        });

        assert_eq!(pane.provider, Some(Provider::Claude));
        assert_eq!(pane.location.session_name, "notes");
        assert_eq!(pane.display.label, "Claude Code | Query");
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
    fn detects_notification_names() {
        assert_eq!(
            notification_name("%window-renamed @1 editor"),
            Some("%window-renamed")
        );
        assert_eq!(notification_name("plain output"), None);
    }

    #[test]
    fn infers_status_from_title_only() {
        let status = infer_status(Some(Provider::Gemini), "Working");
        assert_eq!(status.kind, StatusKind::Busy);
    }

    #[test]
    fn codex_status_uses_title_only() {
        let busy = infer_status(Some(Provider::Codex), "⠹ agentscan | Working");
        let idle = infer_status(Some(Provider::Codex), "Ready");

        assert_eq!(busy.kind, StatusKind::Busy);
        assert_eq!(idle.kind, StatusKind::Idle);
    }

    #[test]
    fn claude_status_distinguishes_spinner_and_idle_marker() {
        let busy = infer_status(Some(Provider::Claude), "⠏ Building summary");
        let idle = infer_status(Some(Provider::Claude), "✳ Review and summarize todo list");

        assert_eq!(busy.kind, StatusKind::Busy);
        assert_eq!(idle.kind, StatusKind::Idle);
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

        let claude_idle = pane_by_id(&panes, "%41");
        assert_eq!(claude_idle.provider, Some(Provider::Claude));
        assert_eq!(claude_idle.status.kind, StatusKind::Idle);
        assert_eq!(claude_idle.display.label, "Review and summarize todo list");

        let claude_busy = pane_by_id(&panes, "%223");
        assert_eq!(claude_busy.status.kind, StatusKind::Busy);

        let opencode = pane_by_id(&panes, "%301");
        assert_eq!(opencode.provider, Some(Provider::Opencode));
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
    fn cache_fixture_deserializes_into_current_schema() {
        let snapshot: SnapshotEnvelope =
            serde_json::from_str(CACHE_SNAPSHOT_FIXTURE).expect("cache fixture should parse");

        assert_eq!(snapshot.schema_version, 1);
        assert_eq!(snapshot.source.kind, SourceKind::Daemon);
        assert_eq!(snapshot.panes.len(), 1);
        assert_eq!(snapshot.panes[0].pane_id, "%67");
        assert_eq!(snapshot.panes[0].status.kind, StatusKind::Idle);
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

    fn pane_by_id<'a>(panes: &'a [PaneRecord], pane_id: &str) -> &'a PaneRecord {
        panes
            .iter()
            .find(|pane| pane.pane_id == pane_id)
            .unwrap_or_else(|| panic!("missing pane fixture entry {pane_id}"))
    }
}
