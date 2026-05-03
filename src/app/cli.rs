use clap::{Args, Parser, Subcommand, ValueEnum};

use super::{Provider, StatusKind};

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
    Status,
    /// Stop the daemon if it is running.
    Stop,
    /// Restart the daemon.
    Restart,
}

#[derive(Subcommand, Debug)]
pub(crate) enum TmuxCommands {
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
}
