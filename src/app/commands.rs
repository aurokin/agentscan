use super::*;

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let root_list_args = cli.list_args;

    match cli.command {
        Some(Commands::Scan(mut args)) => {
            reject_root_auto_start(&root_list_args, "scan")?;
            merge_scan_args(&mut args, &root_list_args);
            command_scan(&args)
        }
        Some(Commands::List(mut args)) => {
            merge_list_args(&mut args, &root_list_args);
            command_list(&args)
        }
        Some(Commands::Snapshot(mut args)) => {
            merge_snapshot_args(&mut args, &root_list_args)?;
            command_snapshot(&args)
        }
        Some(Commands::Subscribe(mut args)) => {
            merge_subscribe_args(&mut args, &root_list_args)?;
            command_subscribe(&args)
        }
        Some(Commands::Providers(mut args)) => {
            merge_providers_args(&mut args, &root_list_args)?;
            command_providers(&args)
        }
        Some(Commands::Hotkeys(mut args)) => {
            merge_hotkeys_args(&mut args, &root_list_args)?;
            command_hotkeys(&args)
        }
        Some(Commands::Hotkey(mut args)) => {
            merge_hotkey_args(&mut args, &root_list_args)?;
            command_hotkey(&args)
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
        None => command_list(&root_list_args),
    }
}

pub(super) fn merge_list_args(args: &mut ListArgs, root_list_args: &ListArgs) {
    args.refresh.refresh |= root_list_args.refresh.refresh;
    args.auto_start.no_auto_start |= root_list_args.auto_start.no_auto_start;
    args.all |= root_list_args.all;
    if args.format == OutputFormat::Text {
        args.format = root_list_args.format;
    }
    args.icons = args.icons.or(root_list_args.icons);
}

pub(super) fn merge_scan_args(args: &mut ScanArgs, root_list_args: &ListArgs) {
    args.refresh.refresh |= root_list_args.refresh.refresh;
    args.all |= root_list_args.all;
    if args.format == OutputFormat::Text {
        args.format = root_list_args.format;
    }
    args.icons = args.icons.or(root_list_args.icons);
}

pub(super) fn merge_snapshot_args(
    args: &mut SnapshotArgs,
    root_list_args: &ListArgs,
) -> Result<()> {
    reject_root_all(root_list_args, "snapshot")?;
    reject_root_icons(root_list_args, "snapshot")?;
    args.refresh.refresh |= root_list_args.refresh.refresh;
    args.auto_start.no_auto_start |= root_list_args.auto_start.no_auto_start;
    if args.format == OutputFormat::Text {
        args.format = root_list_args.format;
    }

    Ok(())
}

pub(super) fn merge_subscribe_args(
    args: &mut SubscribeArgs,
    root_list_args: &ListArgs,
) -> Result<()> {
    reject_root_refresh(root_list_args, "subscribe")?;
    reject_root_all(root_list_args, "subscribe")?;
    reject_root_format(root_list_args, "subscribe")?;
    reject_root_icons(root_list_args, "subscribe")?;
    args.auto_start.no_auto_start |= root_list_args.auto_start.no_auto_start;

    Ok(())
}

pub(super) fn merge_providers_args(
    args: &mut ProvidersArgs,
    root_list_args: &ListArgs,
) -> Result<()> {
    reject_root_refresh(root_list_args, "providers")?;
    reject_root_all(root_list_args, "providers")?;
    reject_root_auto_start(root_list_args, "providers")?;
    if args.format == OutputFormat::Text {
        args.format = root_list_args.format;
    }
    args.icons = args.icons.or(root_list_args.icons);

    Ok(())
}

pub(super) fn merge_inspect_args(args: &mut InspectArgs, root_list_args: &ListArgs) -> Result<()> {
    reject_root_all(root_list_args, "inspect")?;
    reject_root_icons(root_list_args, "inspect")?;
    args.auto_start.no_auto_start |= root_list_args.auto_start.no_auto_start;
    args.refresh.refresh |= root_list_args.refresh.refresh;
    if args.format == OutputFormat::Text {
        args.format = root_list_args.format;
    }

    Ok(())
}

pub(super) fn merge_focus_args(args: &mut FocusArgs, root_list_args: &ListArgs) -> Result<()> {
    reject_root_all(root_list_args, "focus")?;
    reject_root_format(root_list_args, "focus")?;
    reject_root_icons(root_list_args, "focus")?;
    args.auto_start.no_auto_start |= root_list_args.auto_start.no_auto_start;
    args.refresh.refresh |= root_list_args.refresh.refresh;

    Ok(())
}

pub(super) fn merge_hotkeys_args(args: &mut HotkeysArgs, root_list_args: &ListArgs) -> Result<()> {
    reject_root_icons(root_list_args, "hotkeys")?;
    args.refresh.refresh |= root_list_args.refresh.refresh;
    args.auto_start.no_auto_start |= root_list_args.auto_start.no_auto_start;
    args.all |= root_list_args.all;
    if args.format == OutputFormat::Text {
        args.format = root_list_args.format;
    }

    Ok(())
}

pub(super) fn merge_hotkey_args(args: &mut HotkeyArgs, root_list_args: &ListArgs) -> Result<()> {
    reject_root_format(root_list_args, "hotkey")?;
    reject_root_icons(root_list_args, "hotkey")?;
    args.refresh.refresh |= root_list_args.refresh.refresh;
    args.auto_start.no_auto_start |= root_list_args.auto_start.no_auto_start;
    args.all |= root_list_args.all;

    Ok(())
}

pub(super) fn merge_tui_args(args: &mut TuiArgs, root_list_args: &ListArgs) -> Result<()> {
    reject_tui_format(root_list_args)?;
    reject_root_refresh(root_list_args, "tui")?;
    args.auto_start.no_auto_start |= root_list_args.auto_start.no_auto_start;
    args.all |= root_list_args.all;
    args.icons = args.icons.or(root_list_args.icons);

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

pub(super) fn reject_root_auto_start(root_list_args: &ListArgs, command_name: &str) -> Result<()> {
    if root_list_args.auto_start.no_auto_start {
        bail!(
            "`--no-auto-start` is not supported before `{command_name}`; place it on a daemon-backed consumer command or omit it"
        );
    }

    Ok(())
}

pub(super) fn reject_root_icons(root_list_args: &ListArgs, command_name: &str) -> Result<()> {
    if root_list_args.icons.is_some() {
        bail!(
            "`--icons` is not supported before `{command_name}`; place it on a human-facing command that renders provider icons or omit it"
        );
    }

    Ok(())
}

pub(super) fn reject_tui_format(root_list_args: &ListArgs) -> Result<()> {
    if root_list_args.format != OutputFormat::Text {
        bail!(
            "`agentscan tui` is interactive-only and does not support `--format`; use `agentscan list --format json` for supported machine-readable output or `agentscan snapshot --format json` for the raw snapshot envelope"
        );
    }

    Ok(())
}

fn reject_root_list_args(root_list_args: &ListArgs, command_name: &str) -> Result<()> {
    reject_root_refresh(root_list_args, command_name)?;
    reject_root_all(root_list_args, command_name)?;
    reject_root_format(root_list_args, command_name)?;
    reject_root_auto_start(root_list_args, command_name)?;
    reject_root_icons(root_list_args, command_name)
}

fn resolve_command_config(icons: Option<IconMode>) -> Result<ResolvedConfig> {
    config::resolve_config(config::CliConfigOverrides { icons })
}

fn resolve_text_icon_mode(format: OutputFormat, icons: Option<IconMode>) -> Result<IconMode> {
    match format {
        OutputFormat::Text => Ok(resolve_command_config(icons)?.icons),
        OutputFormat::Json => Ok(IconMode::default()),
    }
}

fn command_scan(args: &ScanArgs) -> Result<()> {
    let icon_mode = resolve_text_icon_mode(args.format, args.icons)?;
    emit_filtered_snapshot(
        snapshot_from_direct_tmux_for_recovery()?,
        args.all,
        args.format,
        icon_mode,
    )
}

fn snapshot_from_direct_tmux_for_recovery() -> Result<SnapshotEnvelope> {
    scanner::snapshot_from_tmux()
}

fn snapshot_for_consumer(
    refresh: RefreshArgs,
    auto_start: AutoStartArgs,
) -> Result<SnapshotEnvelope> {
    if refresh.refresh {
        return snapshot_from_direct_tmux_for_recovery();
    }

    daemon::snapshot_via_socket(daemon::AutoStartPolicy::from_args(auto_start))
        .map_err(daemon::DaemonSnapshotError::into_anyhow)
}

fn command_list(args: &ListArgs) -> Result<()> {
    let icon_mode = resolve_text_icon_mode(args.format, args.icons)?;
    let snapshot = snapshot_for_consumer(args.refresh, args.auto_start)?;
    emit_filtered_snapshot(snapshot, args.all, args.format, icon_mode)
}

fn emit_filtered_snapshot(
    mut snapshot: SnapshotEnvelope,
    include_all: bool,
    format: OutputFormat,
    icon_mode: IconMode,
) -> Result<()> {
    snapshot::filter_snapshot(&mut snapshot, include_all);
    output::emit_snapshot(&snapshot, format, icon_mode)
}

fn command_providers(args: &ProvidersArgs) -> Result<()> {
    let config = resolve_command_config(args.icons)?;
    output::emit_providers(&provider_summaries(config.icons), args.format, config.icons)
}

fn command_hotkeys(args: &HotkeysArgs) -> Result<()> {
    let mut snapshot = snapshot_for_consumer(args.refresh, args.auto_start)?;
    snapshot::filter_snapshot(&mut snapshot, args.all);
    // Best-effort: resolve the focused pane and attached-client count live from
    // tmux so clients can highlight the pane the user is in and warn about
    // multiple clients. Any tmux error degrades to "no focus" rather than failing.
    let focus = tmux::tmux_focus_state().unwrap_or_default();
    output::emit_picker_rows(
        &picker::picker_rows(
            &snapshot.panes,
            focus.focused_session.as_deref(),
            u32::try_from(focus.attached_client_count).unwrap_or(u32::MAX),
        ),
        args.format,
    )
}

fn command_hotkey(args: &HotkeyArgs) -> Result<()> {
    let selected_key = picker::normalize_picker_key(&args.key)?;
    let mut snapshot = snapshot_for_consumer(args.refresh, args.auto_start)?;
    snapshot::filter_snapshot(&mut snapshot, args.all);
    // Focus highlight and client count are irrelevant when resolving a key to a
    // pane to switch to.
    let rows = picker::picker_rows(&snapshot.panes, None, 0);
    let row = rows
        .iter()
        .find(|row| row.key == selected_key)
        .with_context(|| format!("hotkey {selected_key} is not assigned in the current picker"))?;

    focus_pane_from_snapshot(
        &snapshot,
        &row.pane_id,
        args.client_tty.as_deref(),
        snapshot_name(args.refresh),
    )
}

fn command_tui(args: &TuiArgs) -> Result<()> {
    let config = resolve_command_config(args.icons)?;
    tui::run(args, config.icons)
}

fn command_snapshot(args: &SnapshotArgs) -> Result<()> {
    let snapshot = snapshot_for_consumer(args.refresh, args.auto_start)?;
    match args.format {
        OutputFormat::Text => output::print_snapshot_summary_text(&snapshot)?,
        OutputFormat::Json => output::print_json(&snapshot)?,
    }
    Ok(())
}

fn command_subscribe(args: &SubscribeArgs) -> Result<()> {
    if args.format != OutputFormat::Json {
        bail!("`agentscan subscribe` only supports `--format json`");
    }

    daemon::stream_subscription_events_json(daemon::AutoStartPolicy::from_args(args.auto_start))
        .map_err(daemon::DaemonSnapshotError::into_anyhow)
}

fn command_inspect(args: &InspectArgs) -> Result<()> {
    let snapshot = snapshot_for_consumer(args.refresh, args.auto_start)?;
    let snapshot_name = if args.refresh.refresh {
        "fresh tmux snapshot"
    } else {
        "daemon snapshot"
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
    let snapshot = snapshot_for_consumer(args.refresh, args.auto_start)?;
    focus_pane_from_snapshot(
        &snapshot,
        &args.pane_id,
        args.client_tty.as_deref(),
        snapshot_name(args.refresh),
    )
}

fn focus_pane_from_snapshot(
    snapshot: &SnapshotEnvelope,
    pane_id: &str,
    client_tty: Option<&str>,
    snapshot_name: &str,
) -> Result<()> {
    let pane_exists = snapshot.panes.iter().any(|pane| pane.pane_id == pane_id);
    if !pane_exists {
        bail!("pane {pane_id} not found in {snapshot_name}");
    }
    match tmux::focus_tmux_pane(pane_id, client_tty)? {
        tmux::FocusTmuxPaneResult::Focused => Ok(()),
        tmux::FocusTmuxPaneResult::Missing => {
            bail!("pane {pane_id} is no longer available")
        }
    }
}

fn snapshot_name(refresh: RefreshArgs) -> &'static str {
    if refresh.refresh {
        "fresh tmux snapshot"
    } else {
        "daemon snapshot"
    }
}

fn command_daemon(args: &DaemonArgs) -> Result<()> {
    match &args.command {
        DaemonCommands::Start => daemon::daemon_start(),
        DaemonCommands::Run => daemon::daemon_run(),
        DaemonCommands::Status(args) => daemon::daemon_status(args.format, args.events),
        DaemonCommands::Stop => daemon::daemon_stop(),
        DaemonCommands::Restart => daemon::daemon_restart(),
    }
}

fn command_tmux(args: &TmuxArgs, root_list_args: &ListArgs) -> Result<()> {
    match &args.command {
        TmuxCommands::Hotkey(args) => {
            reject_root_format(root_list_args, "tmux hotkey")?;
            reject_root_icons(root_list_args, "tmux hotkey")?;
            let mut args = args.clone();
            merge_hotkey_args(&mut args, root_list_args)?;
            command_tmux_hotkey(&args)
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

fn command_tmux_hotkey(args: &HotkeyArgs) -> Result<()> {
    if let Err(error) = command_hotkey(args) {
        let message = error.to_string();
        let _ = tmux::display_tmux_message(args.client_tty.as_deref(), &message);
    }

    Ok(())
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

    println!("updated pane metadata for {pane_id}");
    Ok(())
}

fn command_tmux_clear_metadata(args: &TmuxClearMetadataArgs) -> Result<()> {
    let pane_id = tmux::resolve_tmux_target_pane(args.pane_id.as_deref(), "clear-metadata")?;
    let fields = tmux::tmux_metadata_fields_to_clear(&args.field);

    for option_name in fields {
        tmux::unset_tmux_pane_option(&pane_id, option_name)?;
    }

    println!("cleared pane metadata for {pane_id}");
    Ok(())
}
