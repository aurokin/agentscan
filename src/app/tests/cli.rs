#[test]
fn root_list_args_parse_for_default_list_flow() {
    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "-f", "--all", "--format", "json"]);
    assert!(cli.list_args.refresh.refresh);
    assert!(cli.list_args.all);
    assert_eq!(cli.list_args.format, OutputFormat::Json);
}

#[test]
fn root_list_args_merge_into_list_like_commands() {
    let cli =
        <Cli as clap::Parser>::parse_from(["agentscan", "--all", "--format", "json", "list", "-f"]);
    match cli.command {
        Some(super::Commands::List(mut args)) => {
            super::commands::merge_list_args(&mut args, &cli.list_args);
            assert!(args.refresh.refresh);
            assert!(args.all);
            assert_eq!(args.format, OutputFormat::Json);
        }
        other => panic!("expected list command, got {other:?}"),
    }

    let cli =
        <Cli as clap::Parser>::parse_from(["agentscan", "--all", "--format", "json", "scan", "-f"]);
    match cli.command {
        Some(super::Commands::Scan(mut args)) => {
            super::commands::merge_list_args(&mut args, &cli.list_args);
            assert!(args.refresh.refresh);
            assert!(args.all);
            assert_eq!(args.format, OutputFormat::Json);
        }
        other => panic!("expected scan command, got {other:?}"),
    }
}

#[test]
fn root_list_args_merge_into_other_refresh_capable_commands() {
    let cli =
        <Cli as clap::Parser>::parse_from(["agentscan", "--format", "json", "inspect", "%1", "-f"]);
    match cli.command {
        Some(super::Commands::Inspect(mut args)) => {
            super::commands::merge_inspect_args(&mut args, &cli.list_args).unwrap();
            assert!(args.refresh.refresh);
            assert_eq!(args.format, OutputFormat::Json);
        }
        other => panic!("expected inspect command, got {other:?}"),
    }

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "-f", "focus", "%1"]);
    match cli.command {
        Some(super::Commands::Focus(mut args)) => {
            super::commands::merge_focus_args(&mut args, &cli.list_args).unwrap();
            assert!(args.refresh.refresh);
        }
        other => panic!("expected focus command, got {other:?}"),
    }

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "--all", "popup", "-f"]);
    match cli.command {
        Some(super::Commands::Popup(mut args)) => {
            super::commands::merge_popup_args(&mut args, &cli.list_args).unwrap();
            assert!(args.refresh.refresh);
            assert!(args.all);
        }
        other => panic!("expected popup command, got {other:?}"),
    }

    let cli =
        <Cli as clap::Parser>::parse_from(["agentscan", "--format", "json", "cache", "show", "-f"]);
    match cli.command {
        Some(super::Commands::Cache(args)) => match args.command {
            super::CacheCommands::Show(show_args) => {
                assert!(show_args.refresh.refresh);
                assert_eq!(cli.list_args.format, OutputFormat::Json);
            }
            other => panic!("expected cache show command, got {other:?}"),
        },
        other => panic!("expected cache command, got {other:?}"),
    }
}

#[test]
fn unsupported_root_list_args_are_rejected_for_other_commands() {
    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "--all", "daemon", "status"]);
    assert!(cli.list_args.all);
    assert!(super::commands::reject_root_all(&cli.list_args, "daemon").is_err());

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "--format", "json", "cache", "path"]);
    assert_eq!(cli.list_args.format, OutputFormat::Json);
    assert!(super::commands::reject_root_format(&cli.list_args, "cache path").is_err());

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "--format", "json", "popup"]);
    assert_eq!(cli.list_args.format, OutputFormat::Json);
    match cli.command {
        Some(super::Commands::Popup(mut args)) => {
            let error = super::commands::merge_popup_args(&mut args, &cli.list_args).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("`agentscan popup` is interactive-only"),
                "expected popup guidance, got {error:#}"
            );
            assert!(
                error.to_string().contains("`agentscan list --format json`"),
                "expected list-json migration guidance, got {error:#}"
            );
        }
        other => panic!("expected popup command, got {other:?}"),
    }

    let error = <Cli as clap::Parser>::try_parse_from(["agentscan", "popup", "--format", "json"])
        .expect_err("popup should reject local --format during clap parsing");
    assert!(
        error.to_string().contains("unexpected argument '--format'"),
        "expected clap parse error for popup-local --format, got {error:#}"
    );

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "-f", "tmux", "set-metadata"]);
    assert!(cli.list_args.refresh.refresh);
    assert!(super::commands::reject_root_refresh(&cli.list_args, "tmux set-metadata").is_err());
}

#[test]
fn tmux_set_metadata_accepts_provider_aliases() {
    for (value, expected) in [
        ("cursor_cli", Provider::CursorCli),
        ("cursor-cli", Provider::CursorCli),
        ("cursor-agent", Provider::CursorCli),
        ("copilot", Provider::Copilot),
        ("github-copilot", Provider::Copilot),
        ("pi", Provider::Pi),
        ("pi-coding-agent", Provider::Pi),
    ] {
        let cli = <Cli as clap::Parser>::parse_from([
            "agentscan",
            "tmux",
            "set-metadata",
            "--provider",
            value,
        ]);
        match cli.command {
            Some(super::Commands::Tmux(args)) => match args.command {
                super::TmuxCommands::SetMetadata(set_args) => {
                    assert_eq!(set_args.provider, Some(expected), "value: {value}");
                }
                other => panic!("expected tmux set-metadata command, got {other:?}"),
            },
            other => panic!("expected tmux command, got {other:?}"),
        }
    }
}
