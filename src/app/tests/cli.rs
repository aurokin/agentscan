#[test]
fn root_list_args_parse_for_default_list_flow() {
    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "-f", "--all", "--format", "json"]);
    assert!(cli.list_args.refresh.refresh);
    assert!(!cli.list_args.auto_start.no_auto_start);
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
            assert!(!args.auto_start.no_auto_start);
            assert!(args.all);
            assert_eq!(args.format, OutputFormat::Json);
        }
        other => panic!("expected list command, got {other:?}"),
    }

    let cli =
        <Cli as clap::Parser>::parse_from(["agentscan", "--all", "--format", "json", "scan", "-f"]);
    match cli.command {
        Some(super::Commands::Scan(mut args)) => {
            super::commands::merge_scan_args(&mut args, &cli.list_args);
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
            assert!(!args.auto_start.no_auto_start);
            assert_eq!(args.format, OutputFormat::Json);
        }
        other => panic!("expected inspect command, got {other:?}"),
    }

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "-f", "focus", "%1"]);
    match cli.command {
        Some(super::Commands::Focus(mut args)) => {
            super::commands::merge_focus_args(&mut args, &cli.list_args).unwrap();
            assert!(args.refresh.refresh);
            assert!(!args.auto_start.no_auto_start);
        }
        other => panic!("expected focus command, got {other:?}"),
    }

    let cli =
        <Cli as clap::Parser>::parse_from(["agentscan", "--all", "--format", "json", "hotkeys"]);
    match cli.command {
        Some(super::Commands::Hotkeys(mut args)) => {
            super::commands::merge_hotkeys_args(&mut args, &cli.list_args).unwrap();
            assert!(!args.refresh.refresh);
            assert!(!args.auto_start.no_auto_start);
            assert!(args.all);
            assert_eq!(args.format, OutputFormat::Json);
        }
        other => panic!("expected hotkeys command, got {other:?}"),
    }

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "-f", "--all", "hotkey", "q"]);
    match cli.command {
        Some(super::Commands::Hotkey(mut args)) => {
            super::commands::merge_hotkey_args(&mut args, &cli.list_args).unwrap();
            assert!(args.refresh.refresh);
            assert!(!args.auto_start.no_auto_start);
            assert!(args.all);
            assert_eq!(args.key, "q");
        }
        other => panic!("expected hotkey command, got {other:?}"),
    }

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "--all", "tui"]);
    match cli.command {
        Some(super::Commands::Tui(mut args)) => {
            super::commands::merge_tui_args(&mut args, &cli.list_args).unwrap();
            assert!(!args.auto_start.no_auto_start);
            assert!(args.all);
        }
        other => panic!("expected tui command, got {other:?}"),
    }

    let cli = <Cli as clap::Parser>::parse_from([
        "agentscan",
        "--format",
        "json",
        "--no-auto-start",
        "snapshot",
        "-f",
    ]);
    match cli.command {
        Some(super::Commands::Snapshot(mut args)) => {
            super::commands::merge_snapshot_args(&mut args, &cli.list_args).unwrap();
            assert!(args.refresh.refresh);
            assert!(args.auto_start.no_auto_start);
            assert_eq!(args.format, OutputFormat::Json);
        }
        other => panic!("expected snapshot command, got {other:?}"),
    }

}

#[test]
fn root_list_args_merge_into_tmux_hotkey_command() {
    let cli = <Cli as clap::Parser>::parse_from([
        "agentscan",
        "-f",
        "--all",
        "tmux",
        "hotkey",
        "q",
        "--client-tty",
        "/dev/pts/1",
    ]);
    match cli.command {
        Some(super::Commands::Tmux(super::TmuxArgs {
            command: super::TmuxCommands::Hotkey(mut args),
        })) => {
            super::commands::merge_hotkey_args(&mut args, &cli.list_args).unwrap();
            assert!(args.refresh.refresh);
            assert!(!args.auto_start.no_auto_start);
            assert!(args.all);
            assert_eq!(args.key, "q");
            assert_eq!(args.client_tty.as_deref(), Some("/dev/pts/1"));
        }
        other => panic!("expected tmux hotkey command, got {other:?}"),
    }
}

#[test]
fn subscribe_args_parse_and_merge_root_options() {
    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "subscribe", "--format", "json"]);
    match cli.command {
        Some(super::Commands::Subscribe(mut args)) => {
            super::commands::merge_subscribe_args(&mut args, &cli.list_args).unwrap();
            assert!(!args.auto_start.no_auto_start);
            assert_eq!(args.format, OutputFormat::Json);
        }
        other => panic!("expected subscribe command, got {other:?}"),
    }

    let cli = <Cli as clap::Parser>::parse_from([
        "agentscan",
        "--no-auto-start",
        "subscribe",
        "--format",
        "json",
    ]);
    match cli.command {
        Some(super::Commands::Subscribe(mut args)) => {
            super::commands::merge_subscribe_args(&mut args, &cli.list_args).unwrap();
            assert!(args.auto_start.no_auto_start);
            assert_eq!(args.format, OutputFormat::Json);
        }
        other => panic!("expected subscribe command, got {other:?}"),
    }

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "subscribe", "--format", "text"]);
    match cli.command {
        Some(super::Commands::Subscribe(mut args)) => {
            super::commands::merge_subscribe_args(&mut args, &cli.list_args).unwrap();
            assert_eq!(args.format, OutputFormat::Text);
        }
        other => panic!("expected subscribe command, got {other:?}"),
    }

    let cli =
        <Cli as clap::Parser>::parse_from(["agentscan", "--format", "json", "subscribe"]);
    match cli.command {
        Some(super::Commands::Subscribe(mut args)) => {
            let error = super::commands::merge_subscribe_args(&mut args, &cli.list_args)
                .expect_err("root --format should be rejected for subscribe");
            assert!(
                error
                    .to_string()
                    .contains("`--format` is not supported before `subscribe`"),
                "expected root format rejection, got {error:#}"
            );
        }
        other => panic!("expected subscribe command, got {other:?}"),
    }
}

#[test]
fn providers_accepts_local_and_root_format() {
    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "providers", "--format", "json"]);
    match cli.command {
        Some(super::Commands::Providers(args)) => {
            assert_eq!(args.format, OutputFormat::Json);
        }
        other => panic!("expected providers command, got {other:?}"),
    }

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "--format", "json", "providers"]);
    match cli.command {
        Some(super::Commands::Providers(mut args)) => {
            super::commands::merge_providers_args(&mut args, &cli.list_args).unwrap();
            assert_eq!(args.format, OutputFormat::Json);
        }
        other => panic!("expected providers command, got {other:?}"),
    }
}

#[test]
fn icon_mode_merges_into_human_facing_commands() {
    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "--icons", "nerd-font"]);
    assert_eq!(cli.list_args.icons, Some(IconMode::NerdFont));

    let cli = <Cli as clap::Parser>::parse_from([
        "agentscan",
        "--icons",
        "emoji",
        "list",
        "--icons",
        "nerd-font",
    ]);
    match cli.command {
        Some(super::Commands::List(mut args)) => {
            super::commands::merge_list_args(&mut args, &cli.list_args);
            assert_eq!(args.icons, Some(IconMode::NerdFont));
        }
        other => panic!("expected list command, got {other:?}"),
    }

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "--icons", "nerd-font", "scan"]);
    match cli.command {
        Some(super::Commands::Scan(mut args)) => {
            super::commands::merge_scan_args(&mut args, &cli.list_args);
            assert_eq!(args.icons, Some(IconMode::NerdFont));
        }
        other => panic!("expected scan command, got {other:?}"),
    }

    let cli =
        <Cli as clap::Parser>::parse_from(["agentscan", "--icons", "nerd-font", "providers"]);
    match cli.command {
        Some(super::Commands::Providers(mut args)) => {
            super::commands::merge_providers_args(&mut args, &cli.list_args).unwrap();
            assert_eq!(args.icons, Some(IconMode::NerdFont));
        }
        other => panic!("expected providers command, got {other:?}"),
    }

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "--icons", "nerd-font", "tui"]);
    match cli.command {
        Some(super::Commands::Tui(mut args)) => {
            super::commands::merge_tui_args(&mut args, &cli.list_args).unwrap();
            assert_eq!(args.icons, Some(IconMode::NerdFont));
        }
        other => panic!("expected tui command, got {other:?}"),
    }
}

#[test]
fn root_icon_mode_is_rejected_for_non_icon_commands() {
    for (args, command_name) in [
        (
            ["agentscan", "--icons", "nerd-font", "snapshot"].as_slice(),
            "snapshot",
        ),
        (
            ["agentscan", "--icons", "nerd-font", "subscribe"].as_slice(),
            "subscribe",
        ),
        (
            ["agentscan", "--icons", "nerd-font", "inspect", "%1"].as_slice(),
            "inspect",
        ),
        (
            ["agentscan", "--icons", "nerd-font", "focus", "%1"].as_slice(),
            "focus",
        ),
        (
            ["agentscan", "--icons", "nerd-font", "hotkeys"].as_slice(),
            "hotkeys",
        ),
        (
            ["agentscan", "--icons", "nerd-font", "hotkey", "q"].as_slice(),
            "hotkey",
        ),
        (
            ["agentscan", "--icons", "nerd-font", "daemon", "status"].as_slice(),
            "daemon",
        ),
        (
            [
                "agentscan",
                "--icons",
                "nerd-font",
                "tmux",
                "set-metadata",
            ]
            .as_slice(),
            "tmux",
        ),
    ] {
        let cli = <Cli as clap::Parser>::parse_from(args);
        assert_eq!(cli.list_args.icons, Some(IconMode::NerdFont));

        let error = match cli.command {
            Some(super::Commands::Snapshot(mut args)) => {
                super::commands::merge_snapshot_args(&mut args, &cli.list_args).unwrap_err()
            }
            Some(super::Commands::Subscribe(mut args)) => {
                super::commands::merge_subscribe_args(&mut args, &cli.list_args).unwrap_err()
            }
            Some(super::Commands::Inspect(mut args)) => {
                super::commands::merge_inspect_args(&mut args, &cli.list_args).unwrap_err()
            }
            Some(super::Commands::Focus(mut args)) => {
                super::commands::merge_focus_args(&mut args, &cli.list_args).unwrap_err()
            }
            Some(super::Commands::Hotkeys(mut args)) => {
                super::commands::merge_hotkeys_args(&mut args, &cli.list_args).unwrap_err()
            }
            Some(super::Commands::Hotkey(mut args)) => {
                super::commands::merge_hotkey_args(&mut args, &cli.list_args).unwrap_err()
            }
            Some(super::Commands::Daemon(_)) => {
                super::commands::reject_root_icons(&cli.list_args, command_name).unwrap_err()
            }
            Some(super::Commands::Tmux(_)) => {
                super::commands::reject_root_icons(&cli.list_args, command_name).unwrap_err()
            }
            other => panic!("unexpected command for {command_name}: {other:?}"),
        };

        assert!(
            error.to_string().contains("`--icons` is not supported"),
            "expected root icons rejection, got {error:#}"
        );
    }
}

#[test]
fn daemon_status_accepts_local_format() {
    let cli =
        <Cli as clap::Parser>::parse_from(["agentscan", "daemon", "status", "--format", "json"]);

    match cli.command {
        Some(super::Commands::Daemon(args)) => match args.command {
            super::DaemonCommands::Status(status_args) => {
                assert_eq!(status_args.format, OutputFormat::Json);
            }
            other => panic!("expected daemon status command, got {other:?}"),
        },
        other => panic!("expected daemon command, got {other:?}"),
    }
}

#[test]
fn unsupported_root_list_args_are_rejected_for_other_commands() {
    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "--all", "daemon", "status"]);
    assert!(cli.list_args.all);
    assert!(super::commands::reject_root_all(&cli.list_args, "daemon").is_err());

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "--format", "json", "tui"]);
    assert_eq!(cli.list_args.format, OutputFormat::Json);
    match cli.command {
        Some(super::Commands::Tui(mut args)) => {
            let error = super::commands::merge_tui_args(&mut args, &cli.list_args).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("`agentscan tui` is interactive-only"),
                "expected tui guidance, got {error:#}"
            );
            assert!(
                error.to_string().contains("`agentscan list --format json`"),
                "expected list-json migration guidance, got {error:#}"
            );
        }
        other => panic!("expected tui command, got {other:?}"),
    }

    let error = <Cli as clap::Parser>::try_parse_from(["agentscan", "tui", "--format", "json"])
        .expect_err("tui should reject local --format during clap parsing");
    assert!(
        error.to_string().contains("unexpected argument '--format'"),
        "expected clap parse error for tui-local --format, got {error:#}"
    );

    let error = <Cli as clap::Parser>::try_parse_from(["agentscan", "tui", "-f"])
        .expect_err("tui should reject local refresh during clap parsing");
    assert!(
        error.to_string().contains("unexpected argument '-f'"),
        "expected clap parse error for tui-local refresh, got {error:#}"
    );

    let error = <Cli as clap::Parser>::try_parse_from(["agentscan", "popup"])
        .expect_err("popup should not remain as a compatibility alias");
    assert!(
        error.to_string().contains("unrecognized subcommand 'popup'"),
        "expected clap parse error for removed popup command, got {error:#}"
    );

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "-f", "tmux", "set-metadata"]);
    assert!(cli.list_args.refresh.refresh);
    assert!(super::commands::reject_root_refresh(&cli.list_args, "tmux set-metadata").is_err());

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "-f", "tui"]);
    assert!(cli.list_args.refresh.refresh);
    match cli.command {
        Some(super::Commands::Tui(mut args)) => {
            let error = super::commands::merge_tui_args(&mut args, &cli.list_args).unwrap_err();
            assert!(
                error.to_string().contains("`--refresh` is not supported"),
                "expected root refresh rejection, got {error:#}"
            );
        }
        other => panic!("expected tui command, got {other:?}"),
    }

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "--all", "providers"]);
    match cli.command {
        Some(super::Commands::Providers(mut args)) => {
            let error = super::commands::merge_providers_args(&mut args, &cli.list_args).unwrap_err();
            assert!(
                error.to_string().contains("`--all` is not supported"),
                "expected root all rejection, got {error:#}"
            );
        }
        other => panic!("expected providers command, got {other:?}"),
    }
}

#[test]
fn auto_start_opt_out_parses_only_for_future_daemon_backed_consumers() {
    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "--no-auto-start"]);
    assert!(cli.list_args.auto_start.no_auto_start);

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "--no-auto-start", "list"]);
    match cli.command {
        Some(super::Commands::List(mut args)) => {
            super::commands::merge_list_args(&mut args, &cli.list_args);
            assert!(args.auto_start.no_auto_start);
        }
        other => panic!("expected list command, got {other:?}"),
    }

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "list", "--no-auto-start"]);
    match cli.command {
        Some(super::Commands::List(args)) => {
            assert!(args.auto_start.no_auto_start);
        }
        other => panic!("expected list command, got {other:?}"),
    }

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "--no-auto-start", "inspect", "%1"]);
    match cli.command {
        Some(super::Commands::Inspect(mut args)) => {
            super::commands::merge_inspect_args(&mut args, &cli.list_args).unwrap();
            assert!(args.auto_start.no_auto_start);
        }
        other => panic!("expected inspect command, got {other:?}"),
    }

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "focus", "%1", "--no-auto-start"]);
    match cli.command {
        Some(super::Commands::Focus(args)) => {
            assert!(args.auto_start.no_auto_start);
        }
        other => panic!("expected focus command, got {other:?}"),
    }

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "snapshot", "--no-auto-start"]);
    match cli.command {
        Some(super::Commands::Snapshot(args)) => {
            assert!(args.auto_start.no_auto_start);
        }
        other => panic!("expected snapshot command, got {other:?}"),
    }

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "--no-auto-start", "tui"]);
    match cli.command {
        Some(super::Commands::Tui(mut args)) => {
            super::commands::merge_tui_args(&mut args, &cli.list_args).unwrap();
            assert!(args.auto_start.no_auto_start);
        }
        other => panic!("expected tui command, got {other:?}"),
    }

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "tui", "--no-auto-start"]);
    match cli.command {
        Some(super::Commands::Tui(args)) => {
            assert!(args.auto_start.no_auto_start);
        }
        other => panic!("expected tui command, got {other:?}"),
    }
}

#[test]
fn auto_start_opt_out_is_rejected_for_non_daemon_backed_commands() {
    for args in [
        ["agentscan", "scan", "--no-auto-start"].as_slice(),
        ["agentscan", "daemon", "status", "--no-auto-start"].as_slice(),
    ] {
        let error = <Cli as clap::Parser>::try_parse_from(args)
            .expect_err("local no-auto-start should be rejected by clap");
        assert!(
            error.to_string().contains("--no-auto-start"),
            "expected no-auto-start parse error for {args:?}, got {error:#}"
        );
    }

    let cli = <Cli as clap::Parser>::parse_from(["agentscan", "--no-auto-start", "scan"]);
    assert!(cli.list_args.auto_start.no_auto_start);
    match cli.command {
        Some(super::Commands::Scan(_)) => {
            let error = super::commands::reject_root_auto_start(&cli.list_args, "scan").unwrap_err();
            assert!(
                error.to_string().contains("`--no-auto-start` is not supported"),
                "expected root no-auto-start rejection, got {error:#}"
            );
        }
        other => panic!("expected scan command, got {other:?}"),
    }

    for (args, command_name) in [
        (
            ["agentscan", "--no-auto-start", "daemon", "status"].as_slice(),
            "daemon",
        ),
        (
            ["agentscan", "--no-auto-start", "tmux", "set-metadata"].as_slice(),
            "tmux",
        ),
        (
            ["agentscan", "--no-auto-start", "providers"].as_slice(),
            "providers",
        ),
    ] {
        let cli = <Cli as clap::Parser>::parse_from(args);
        assert!(cli.list_args.auto_start.no_auto_start);
        assert!(super::commands::reject_root_auto_start(&cli.list_args, command_name).is_err());
    }
}

#[test]
fn tmux_set_metadata_accepts_provider_aliases() {
    for (value, expected) in [
        ("cursor_cli", Provider::CursorCli),
        ("cursor-cli", Provider::CursorCli),
        ("cursor-agent", Provider::CursorCli),
        ("aider", Provider::Aider),
        ("aider-chat", Provider::Aider),
        ("copilot", Provider::Copilot),
        ("github-copilot", Provider::Copilot),
        ("antigravity", Provider::Antigravity),
        ("agy", Provider::Antigravity),
        ("pi", Provider::Pi),
        ("pi-coding-agent", Provider::Pi),
        ("grok", Provider::Grok),
        ("hermes", Provider::Hermes),
        ("hermes-agent", Provider::Hermes),
        ("droid", Provider::Droid),
        ("factory-droid", Provider::Droid),
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
