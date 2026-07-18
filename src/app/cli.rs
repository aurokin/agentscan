use clap::{Args, Parser, Subcommand, ValueEnum};

use super::{IconMode, Provider, StatusKind};

#[derive(Parser, Debug)]
#[command(author, version, about = "Scan tmux panes for agent sessions")]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Commands>,

    #[command(flatten)]
    pub(crate) list_args: ListArgs,
}

#[derive(Subcommand, Debug)]
pub(crate) enum Commands {
    /// Take a direct snapshot from tmux.
    Scan(ScanArgs),
    /// List panes using the best available state source.
    List(ListArgs),
    /// Print the current raw snapshot envelope.
    Snapshot(SnapshotArgs),
    /// Stream live daemon subscription events as JSON Lines.
    Subscribe(SubscribeArgs),
    /// Print supported coding agent providers and display markers.
    Providers(ProvidersArgs),
    /// Print current picker hotkeys and their target panes.
    Hotkeys(HotkeysArgs),
    /// Focus the pane assigned to a current picker hotkey.
    Hotkey(HotkeyArgs),
    /// Open the interactive TUI. `tui` is interactive-only; use `list --format json` for automation.
    Tui(TuiArgs),
    /// Inspect one pane by pane id.
    Inspect(InspectArgs),
    /// Focus a pane by pane id.
    Focus(FocusArgs),
    /// Run daemon-related commands.
    Daemon(DaemonArgs),
    /// tmux-facing helper commands.
    Tmux(TmuxArgs),
    /// Diagnose environment, tmux, daemon, and discovery health.
    Doctor(DoctorArgs),
    /// Generate shell completions for bash, zsh, or fish.
    Completions(CompletionsArgs),
}

#[derive(Args, Clone, Copy, Debug)]
pub(crate) struct CompletionsArgs {
    /// Shell to generate completions for.
    #[arg(value_enum)]
    pub(crate) shell: clap_complete::Shell,
}

#[derive(Args, Clone, Copy, Debug)]
pub(crate) struct ListArgs {
    #[command(flatten)]
    pub(crate) refresh: RefreshArgs,

    #[command(flatten)]
    pub(crate) auto_start: AutoStartArgs,

    /// Include all tmux panes, not only likely agent panes.
    #[arg(long)]
    pub(crate) all: bool,

    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,

    /// Icon rendering mode for human-facing output.
    #[arg(long, value_enum)]
    pub(crate) icons: Option<IconMode>,
}

#[derive(Args, Clone, Copy, Debug)]
pub(crate) struct ScanArgs {
    #[command(flatten)]
    pub(crate) refresh: RefreshArgs,

    /// Include all tmux panes, not only likely agent panes.
    #[arg(long)]
    pub(crate) all: bool,

    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,

    /// Icon rendering mode for human-facing output.
    #[arg(long, value_enum)]
    pub(crate) icons: Option<IconMode>,
}

#[derive(Args, Clone, Copy, Debug)]
pub(crate) struct SnapshotArgs {
    #[command(flatten)]
    pub(crate) refresh: RefreshArgs,

    #[command(flatten)]
    pub(crate) auto_start: AutoStartArgs,

    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
}

#[derive(Args, Clone, Copy, Debug)]
pub(crate) struct SubscribeArgs {
    #[command(flatten)]
    pub(crate) auto_start: AutoStartArgs,

    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
}

#[derive(Args, Clone, Copy, Debug)]
pub(crate) struct ProvidersArgs {
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,

    /// Icon rendering mode for human-facing output.
    #[arg(long, value_enum)]
    pub(crate) icons: Option<IconMode>,
}

#[derive(Args, Clone, Copy, Debug)]
pub(crate) struct HotkeysArgs {
    #[command(flatten)]
    pub(crate) refresh: RefreshArgs,

    #[command(flatten)]
    pub(crate) auto_start: AutoStartArgs,

    /// Include all tmux panes, not only likely agent panes, in the picker model.
    #[arg(long)]
    pub(crate) all: bool,

    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
}

#[derive(Args, Clone, Debug)]
pub(crate) struct HotkeyArgs {
    /// The picker hotkey to activate, for example `q`.
    pub(crate) key: String,

    #[command(flatten)]
    pub(crate) refresh: RefreshArgs,

    #[command(flatten)]
    pub(crate) auto_start: AutoStartArgs,

    /// Include all tmux panes, not only likely agent panes, in the picker model.
    #[arg(long)]
    pub(crate) all: bool,

    /// The tmux client tty to target when switching panes.
    #[arg(long)]
    pub(crate) client_tty: Option<String>,
}

#[derive(Args, Debug)]
pub(crate) struct InspectArgs {
    /// The tmux pane id, for example `%42`.
    pub(crate) pane_id: String,

    #[command(flatten)]
    pub(crate) refresh: RefreshArgs,

    #[command(flatten)]
    pub(crate) auto_start: AutoStartArgs,

    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
}

#[derive(Args, Debug)]
pub(crate) struct FocusArgs {
    /// The tmux pane id, for example `%42`.
    pub(crate) pane_id: String,

    #[command(flatten)]
    pub(crate) refresh: RefreshArgs,

    #[command(flatten)]
    pub(crate) auto_start: AutoStartArgs,

    /// The tmux client tty to target when switching panes.
    #[arg(long)]
    pub(crate) client_tty: Option<String>,
}

#[derive(Args, Debug)]
pub(crate) struct TuiArgs {
    #[command(flatten)]
    pub(crate) auto_start: AutoStartArgs,

    /// Include all tmux panes, not only likely agent panes, in the interactive picker.
    #[arg(long)]
    pub(crate) all: bool,

    /// Icon rendering mode for human-facing output.
    #[arg(long, value_enum)]
    pub(crate) icons: Option<IconMode>,
}

#[derive(Args, Debug)]
pub(crate) struct DaemonArgs {
    #[command(subcommand)]
    pub(crate) command: DaemonCommands,
}

#[derive(Args, Debug)]
pub(crate) struct TmuxArgs {
    #[command(subcommand)]
    pub(crate) command: TmuxCommands,
}

#[derive(Subcommand, Debug)]
pub(crate) enum DaemonCommands {
    /// Start the daemon in the background.
    Start,
    /// Run the long-lived daemon loop.
    Run,
    /// Report daemon lifecycle status.
    Status(DaemonStatusArgs),
    /// Stop the daemon if it is running.
    Stop,
    /// Restart the daemon.
    Restart,
}

#[derive(Args, Clone, Copy, Debug)]
pub(crate) struct DaemonStatusArgs {
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,

    /// Include the bounded recent daemon observability event ring.
    #[arg(long)]
    pub(crate) events: bool,
}

#[derive(Args, Clone, Copy, Debug)]
pub(crate) struct DoctorArgs {
    /// Also take a direct tmux snapshot and compare it against daemon state.
    #[command(flatten)]
    pub(crate) refresh: RefreshArgs,

    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,

    /// Include the bounded recent daemon observability event ring in daemon health.
    #[arg(long)]
    pub(crate) events: bool,
}

#[derive(Subcommand, Debug)]
pub(crate) enum TmuxCommands {
    /// Focus a picker hotkey from a tmux bind and report failures with display-message.
    Hotkey(HotkeyArgs),
    /// Publish explicit pane metadata for wrappers.
    SetMetadata(TmuxSetMetadataArgs),
    /// Clear explicit pane metadata.
    ClearMetadata(TmuxClearMetadataArgs),
}

#[derive(Args, Debug)]
pub(crate) struct TmuxSetMetadataArgs {
    /// The tmux pane id to target. Defaults to the current pane when inside tmux.
    #[arg(long)]
    pub(crate) pane_id: Option<String>,

    /// Explicit provider published by the wrapper.
    #[arg(long, value_enum)]
    pub(crate) provider: Option<Provider>,

    /// User-facing short label published by the wrapper.
    #[arg(long)]
    pub(crate) label: Option<String>,

    /// Explicit working directory published by the wrapper.
    #[arg(long)]
    pub(crate) cwd: Option<String>,

    /// Optional explicit state published by the wrapper.
    #[arg(long, value_enum)]
    pub(crate) state: Option<StatusKind>,

    /// Optional provider-specific session identifier.
    #[arg(long)]
    pub(crate) session_id: Option<String>,

    /// Publishing process id used to reject stale metadata. The scanner drops
    /// the whole metadata block when this is not a live pane descendant, so a
    /// non-numeric or zero value would silently untrust every field — reject
    /// it here instead of writing it.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    pub(crate) pid: Option<u32>,

    /// Metadata contract version.
    #[arg(long = "v", value_parser = clap::value_parser!(u32))]
    pub(crate) contract_version: Option<u32>,

    /// Agent model identifier.
    #[arg(long)]
    pub(crate) model: Option<String>,
}

#[derive(Args, Debug)]
pub(crate) struct TmuxClearMetadataArgs {
    /// The tmux pane id to target. Defaults to the current pane when inside tmux.
    #[arg(long)]
    pub(crate) pane_id: Option<String>,

    /// Clear only specific metadata fields. Defaults to all fields.
    #[arg(long, value_enum)]
    pub(crate) field: Vec<TmuxMetadataField>,
}

#[derive(Args, Clone, Copy, Debug, Default)]
pub(crate) struct RefreshArgs {
    /// Bypass daemon-backed state and read a fresh tmux snapshot.
    #[arg(short = 'f', long = "refresh")]
    pub(crate) refresh: bool,
}

#[derive(Args, Clone, Copy, Debug, Default)]
pub(crate) struct AutoStartArgs {
    /// Disable daemon auto-start when this command uses daemon-backed state.
    #[arg(long = "no-auto-start")]
    pub(crate) no_auto_start: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum OutputFormat {
    Text,
    Json,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum TmuxMetadataField {
    Provider,
    Label,
    Cwd,
    State,
    SessionId,
    Pid,
    V,
    Model,
}
