use super::*;

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let root_list_args = cli.list_args;

    match cli.command {
        Some(Commands::Scan(mut args)) => {
            merge_list_args(&mut args, &root_list_args);
            command_scan(&args)
        }
        Some(Commands::List(mut args)) => {
            merge_list_args(&mut args, &root_list_args);
            command_list(&args)
        }
        Some(Commands::Tui(mut args)) => {
            merge_tui_args(&mut args, &root_list_args)?;
            command_tui(&args)
        }
        Some(Commands::Inspect(mut args)) => {
            merge_inspect_args(&mut args, &root_list_args)?;
            command_inspect(&args)
        }
        Some(Commands::Focus(mut args)) => {
            merge_focus_args(&mut args, &root_list_args)?;
            command_focus(&args)
        }
        Some(Commands::Daemon(args)) => {
            reject_root_list_args(&root_list_args, "daemon")?;
            command_daemon(&args)
        }
        Some(Commands::Tmux(args)) => command_tmux(&args, &root_list_args),
        Some(Commands::Cache(args)) => command_cache(&args, &root_list_args),
        None => command_list(&root_list_args),
    }
}

pub(super) fn merge_list_args(args: &mut ListArgs, root_list_args: &ListArgs) {
    args.refresh.refresh |= root_list_args.refresh.refresh;
    args.all |= root_list_args.all;
    if args.format == OutputFormat::Text {
        args.format = root_list_args.format;
    }
}

pub(super) fn merge_inspect_args(args: &mut InspectArgs, root_list_args: &ListArgs) -> Result<()> {
    reject_root_all(root_list_args, "inspect")?;
    args.refresh.refresh |= root_list_args.refresh.refresh;
    if args.format == OutputFormat::Text {
        args.format = root_list_args.format;
    }

    Ok(())
}

pub(super) fn merge_focus_args(args: &mut FocusArgs, root_list_args: &ListArgs) -> Result<()> {
    reject_root_all(root_list_args, "focus")?;
    reject_root_format(root_list_args, "focus")?;
    args.refresh.refresh |= root_list_args.refresh.refresh;

    Ok(())
}

pub(super) fn merge_tui_args(args: &mut TuiArgs, root_list_args: &ListArgs) -> Result<()> {
    reject_tui_format(root_list_args)?;
    args.refresh.refresh |= root_list_args.refresh.refresh;
    args.all |= root_list_args.all;

    Ok(())
}

pub(super) fn reject_root_refresh(root_list_args: &ListArgs, command_name: &str) -> Result<()> {
    if root_list_args.refresh.refresh {
        bail!(
            "`--refresh` is not supported before `{command_name}`; place it on a refresh-capable subcommand or omit it"
        );
    }

    Ok(())
}

pub(super) fn reject_root_all(root_list_args: &ListArgs, command_name: &str) -> Result<()> {
    if root_list_args.all {
        bail!(
            "`--all` is not supported before `{command_name}`; place it on a list-like subcommand or omit it"
        );
    }

    Ok(())
}

pub(super) fn reject_root_format(root_list_args: &ListArgs, command_name: &str) -> Result<()> {
    if root_list_args.format != OutputFormat::Text {
        bail!(
            "`--format` is not supported before `{command_name}`; place it on a format-capable subcommand or omit it"
        );
    }

    Ok(())
}

pub(super) fn reject_tui_format(root_list_args: &ListArgs) -> Result<()> {
    if root_list_args.format != OutputFormat::Text {
        bail!(
            "`agentscan tui` is interactive-only and does not support `--format`; use `agentscan list --format json` for supported machine-readable output or `agentscan cache show --format json` for the raw cached snapshot"
        );
    }

    Ok(())
}

fn reject_root_list_args(root_list_args: &ListArgs, command_name: &str) -> Result<()> {
    reject_root_refresh(root_list_args, command_name)?;
    reject_root_all(root_list_args, command_name)?;
    reject_root_format(root_list_args, command_name)
}

fn command_scan(args: &ListArgs) -> Result<()> {
    emit_filtered_snapshot(
        snapshot_for_scan(args.refresh.refresh)?,
        args.all,
        args.format,
    )
}

fn snapshot_for_scan(refresh: bool) -> Result<SnapshotEnvelope> {
    if refresh {
        cache::refresh_cache_from_tmux()
    } else {
        scanner::snapshot_from_tmux()
    }
}

fn command_list(args: &ListArgs) -> Result<()> {
    let snapshot = cache::load_snapshot(args.refresh.refresh)?;
    emit_filtered_snapshot(snapshot, args.all, args.format)
}

fn emit_filtered_snapshot(
    mut snapshot: SnapshotEnvelope,
    include_all: bool,
    format: OutputFormat,
) -> Result<()> {
    cache::filter_snapshot(&mut snapshot, include_all);
    output::emit_snapshot(&snapshot, format)
}

fn command_tui(args: &TuiArgs) -> Result<()> {
    tui::run(args)
}

fn command_inspect(args: &InspectArgs) -> Result<()> {
    let snapshot = cache::load_snapshot(args.refresh.refresh)?;
    let snapshot_name = if args.refresh.refresh {
        "fresh tmux snapshot"
    } else {
        "cached snapshot"
    };
    let pane = snapshot
        .panes
        .into_iter()
        .find(|pane| pane.pane_id == args.pane_id)
        .with_context(|| format!("pane {} not found in {snapshot_name}", args.pane_id))?;

    match args.format {
        OutputFormat::Text => output::print_inspect_text(&pane),
        OutputFormat::Json => output::print_json(&pane)?,
    }

    Ok(())
}

fn command_focus(args: &FocusArgs) -> Result<()> {
    if args.refresh.refresh {
        let snapshot = cache::refresh_cache_from_tmux()?;
        let pane_exists = snapshot
            .panes
            .iter()
            .any(|pane| pane.pane_id == args.pane_id);
        if !pane_exists {
            bail!("pane {} not found in fresh tmux snapshot", args.pane_id);
        }
    }
    match tmux::focus_tmux_pane(&args.pane_id, args.client_tty.as_deref())? {
        tmux::FocusTmuxPaneResult::Focused => Ok(()),
        tmux::FocusTmuxPaneResult::Missing => {
            bail!("pane {} is no longer available", args.pane_id)
        }
    }
}

fn command_daemon(args: &DaemonArgs) -> Result<()> {
    match args.command {
        DaemonCommands::Run => daemon::daemon_run(),
        DaemonCommands::Status(ref args) => command_daemon_status(args),
    }
}

fn command_daemon_status(args: &DaemonStatusArgs) -> Result<()> {
    let path = cache::cache_path()?;
    let snapshot = cache::read_snapshot_from_cache()?;
    let summary = cache::summarize_snapshot(&snapshot)?;
    let diagnostics = cache::cache_diagnostics(&snapshot, args.max_age_seconds)?;

    output::print_daemon_status_text(
        &path,
        &snapshot,
        &summary,
        &diagnostics,
        args.max_age_seconds,
    );

    match diagnostics.daemon_cache_status {
        DaemonCacheStatus::Healthy => Ok(()),
        DaemonCacheStatus::Stale => bail!("daemon cache is stale"),
        DaemonCacheStatus::SnapshotOnly => {
            bail!("daemon cache is snapshot-only; run `agentscan daemon run` for normal cached use")
        }
        DaemonCacheStatus::Unavailable => {
            bail!(
                "daemon cache is unavailable because the cache does not include a usable daemon refresh timestamp"
            )
        }
    }
}

fn command_cache(args: &CacheArgs, root_list_args: &ListArgs) -> Result<()> {
    match args.command {
        CacheCommands::Path => {
            reject_root_list_args(root_list_args, "cache path")?;
            println!("{}", cache::cache_path()?.display());
        }
        CacheCommands::Show(ref args) => {
            reject_root_all(root_list_args, "cache show")?;
            let snapshot =
                cache::load_snapshot(args.refresh.refresh || root_list_args.refresh.refresh)?;
            match merged_output_format(args.format, root_list_args.format) {
                OutputFormat::Text => output::print_cache_summary_text(&snapshot)?,
                OutputFormat::Json => output::print_json(&snapshot)?,
            }
        }
        CacheCommands::Validate(ref args) => {
            reject_root_all(root_list_args, "cache validate")?;
            reject_root_format(root_list_args, "cache validate")?;
            let path = cache::cache_path()?;
            let snapshot =
                cache::load_snapshot(args.refresh.refresh || root_list_args.refresh.refresh)?;
            let summary = cache::validate_snapshot(&snapshot, args.max_age_seconds)?;
            let diagnostics = cache::cache_diagnostics(&snapshot, args.max_age_seconds)?;
            output::print_cache_validate_text(
                &path,
                &snapshot,
                &summary,
                &diagnostics,
                args.max_age_seconds,
            );
        }
    }

    Ok(())
}

fn merged_output_format(command_format: OutputFormat, root_format: OutputFormat) -> OutputFormat {
    if command_format == OutputFormat::Text {
        root_format
    } else {
        command_format
    }
}

fn command_tmux(args: &TmuxArgs, root_list_args: &ListArgs) -> Result<()> {
    match &args.command {
        TmuxCommands::SetMetadata(args) => {
            reject_root_list_args(root_list_args, "tmux set-metadata")?;
            command_tmux_set_metadata(args)
        }
        TmuxCommands::ClearMetadata(args) => {
            reject_root_list_args(root_list_args, "tmux clear-metadata")?;
            command_tmux_clear_metadata(args)
        }
    }
}

fn command_tmux_set_metadata(args: &TmuxSetMetadataArgs) -> Result<()> {
    let pane_id = tmux::resolve_tmux_target_pane(args.pane_id.as_deref(), "set-metadata")?;

    let updates = tmux::tmux_metadata_updates(args);
    if updates.is_empty() {
        bail!("no metadata fields were provided");
    }

    for (option_name, value) in updates {
        tmux::set_tmux_pane_option(&pane_id, option_name, &value)?;
    }
    cache::refresh_existing_cache_from_tmux()?;

    println!("updated pane metadata for {pane_id}");
    Ok(())
}

fn command_tmux_clear_metadata(args: &TmuxClearMetadataArgs) -> Result<()> {
    let pane_id = tmux::resolve_tmux_target_pane(args.pane_id.as_deref(), "clear-metadata")?;
    let fields = tmux::tmux_metadata_fields_to_clear(&args.field);

    for option_name in fields {
        tmux::unset_tmux_pane_option(&pane_id, option_name)?;
    }
    cache::refresh_existing_cache_from_tmux()?;

    println!("cleared pane metadata for {pane_id}");
    Ok(())
}
