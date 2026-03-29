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

fn reject_root_list_args(root_list_args: &ListArgs, command_name: &str) -> Result<()> {
    reject_root_refresh(root_list_args, command_name)?;
    reject_root_all(root_list_args, command_name)?;
    reject_root_format(root_list_args, command_name)
}

fn command_scan(args: &ListArgs) -> Result<()> {
    let mut snapshot = if args.refresh.refresh {
        cache::refresh_cache_from_tmux()?
    } else {
        cache::snapshot_from_tmux()?
    };
    cache::filter_snapshot(&mut snapshot, args.all);
    output::emit_snapshot(&snapshot, args.format)
}

fn command_list(args: &ListArgs) -> Result<()> {
    let mut snapshot = cache::load_snapshot(args.refresh.refresh)?;
    cache::filter_snapshot(&mut snapshot, args.all);
    output::emit_snapshot(&snapshot, args.format)
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
            let format = if args.format == OutputFormat::Text {
                root_list_args.format
            } else {
                args.format
            };
            match args.format {
                OutputFormat::Text => {
                    if format == OutputFormat::Text {
                        output::print_cache_summary_text(&snapshot)?
                    } else {
                        output::print_json(&snapshot)?
                    }
                }
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
            output::print_cache_validate_text(&path, &snapshot, &summary, args.max_age_seconds);
        }
    }

    Ok(())
}

fn command_tmux(args: &TmuxArgs, root_list_args: &ListArgs) -> Result<()> {
    match &args.command {
        TmuxCommands::Popup(args) => {
            let args = TmuxPopupArgs {
                refresh: RefreshArgs {
                    refresh: args.refresh.refresh || root_list_args.refresh.refresh,
                },
                all: args.all || root_list_args.all,
                format: if args.format == PopupOutputFormat::Tsv
                    && root_list_args.format == OutputFormat::Json
                {
                    PopupOutputFormat::Json
                } else {
                    args.format
                },
            };
            command_tmux_popup(&args)
        }
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

fn command_tmux_popup(args: &TmuxPopupArgs) -> Result<()> {
    let mut snapshot = cache::load_snapshot(args.refresh.refresh)?;
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
