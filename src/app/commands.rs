use super::*;

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
    let mut snapshot = cache::snapshot_from_tmux()?;
    if refresh {
        cache::write_snapshot_to_cache(&snapshot)?;
    }
    cache::filter_snapshot(&mut snapshot, args.all);
    output::emit_snapshot(&snapshot, args.format)
}

fn command_list(args: &ListArgs, refresh: bool) -> Result<()> {
    let mut snapshot = cache::load_snapshot(refresh)?;
    cache::filter_snapshot(&mut snapshot, args.all);
    output::emit_snapshot(&snapshot, args.format)
}

fn command_inspect(args: &InspectArgs, refresh: bool) -> Result<()> {
    let snapshot = cache::load_snapshot(refresh)?;
    let snapshot_name = if refresh {
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

fn command_focus(args: &FocusArgs, refresh: bool) -> Result<()> {
    if refresh {
        let snapshot = cache::refresh_cache_from_tmux()?;
        let pane_exists = snapshot
            .panes
            .iter()
            .any(|pane| pane.pane_id == args.pane_id);
        if !pane_exists {
            bail!("pane {} not found in fresh tmux snapshot", args.pane_id);
        }
    }
    tmux::focus_tmux_pane(&args.pane_id, args.client_tty.as_deref())
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
    let cache_age_seconds = cache::cache_age_seconds(summary.generated_at);
    let daemon_age_seconds = cache::daemon_age_seconds(&snapshot)?;
    let status = cache::daemon_cache_status(daemon_age_seconds, args.max_age_seconds);

    println!("daemon_cache_status: {}", daemon_cache_status_name(status));
    println!("path: {}", path.display());
    println!("generated_at: {}", snapshot.generated_at);
    println!("cache_age_seconds: {cache_age_seconds}");
    if let Some(daemon_age_seconds) = daemon_age_seconds {
        println!("daemon_age_seconds: {daemon_age_seconds}");
    }
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
            println!("{}", cache::cache_path()?.display());
        }
        CacheCommands::Show(ref args) => {
            let snapshot = cache::load_snapshot(refresh)?;
            match args.format {
                OutputFormat::Text => output::print_cache_summary_text(&snapshot)?,
                OutputFormat::Json => output::print_json(&snapshot)?,
            }
        }
        CacheCommands::Validate(ref args) => {
            let path = cache::cache_path()?;
            let snapshot = cache::load_snapshot(refresh)?;
            let summary = cache::validate_snapshot(&snapshot, args.max_age_seconds)?;
            output::print_cache_validate_text(&path, &snapshot, &summary, args.max_age_seconds);
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

fn command_tmux_popup(args: &TmuxPopupArgs, refresh: bool) -> Result<()> {
    let mut snapshot = cache::load_snapshot(refresh)?;
    cache::filter_snapshot(&mut snapshot, args.all);
    let entries = cache::popup_entries(&snapshot.panes);

    match args.format {
        PopupOutputFormat::Tsv => {
            output::print_popup_tsv(&entries);
            Ok(())
        }
        PopupOutputFormat::Json => output::print_json(&entries),
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
