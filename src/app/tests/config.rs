#[test]
fn config_path_uses_xdg_config_home_before_home() {
    let source = config::ConfigSource {
        xdg_config_home: Some(Path::new("/tmp/xdg").to_path_buf()),
        home: Some(Path::new("/tmp/home").to_path_buf()),
        ..config::ConfigSource::default()
    };

    assert_eq!(
        config::config_path(&source).as_deref(),
        Some(Path::new("/tmp/xdg/agentscan/config.toml"))
    );
}

#[test]
fn config_path_treats_empty_xdg_config_home_as_unset() {
    let source = config::ConfigSource {
        xdg_config_home: Some(Path::new("").to_path_buf()),
        home: Some(Path::new("/tmp/home").to_path_buf()),
        ..config::ConfigSource::default()
    };

    assert_eq!(
        config::config_path(&source).as_deref(),
        Some(Path::new("/tmp/home/.config/agentscan/config.toml"))
    );
}

#[test]
fn config_path_treats_relative_xdg_config_home_as_unset() {
    let source = config::ConfigSource {
        xdg_config_home: Some(Path::new("relative-config").to_path_buf()),
        home: Some(Path::new("/tmp/home").to_path_buf()),
        ..config::ConfigSource::default()
    };

    assert_eq!(
        config::config_path(&source).as_deref(),
        Some(Path::new("/tmp/home/.config/agentscan/config.toml"))
    );
}

#[test]
fn config_path_falls_back_to_home_config() {
    let source = config::ConfigSource {
        home: Some(Path::new("/tmp/home").to_path_buf()),
        ..config::ConfigSource::default()
    };

    assert_eq!(
        config::config_path(&source).as_deref(),
        Some(Path::new("/tmp/home/.config/agentscan/config.toml"))
    );
}

#[test]
fn config_path_treats_empty_home_as_unset() {
    let source = config::ConfigSource {
        home: Some(Path::new("").to_path_buf()),
        ..config::ConfigSource::default()
    };

    assert_eq!(config::config_path(&source), None);
}

#[test]
fn config_path_treats_relative_home_as_unset() {
    let source = config::ConfigSource {
        home: Some(Path::new("relative-home").to_path_buf()),
        ..config::ConfigSource::default()
    };

    assert_eq!(config::config_path(&source), None);
}

#[test]
fn config_defaults_to_emoji_when_file_is_missing() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let source = config::ConfigSource {
        xdg_config_home: Some(tempdir.path().to_path_buf()),
        ..config::ConfigSource::default()
    };

    let resolved =
        config::resolve_config_from_source(&source).expect("missing config should be accepted");

    assert_eq!(resolved.icons, IconMode::Emoji);
    assert_eq!(
        resolved.config_path.as_deref(),
        Some(tempdir.path().join("agentscan/config.toml").as_path())
    );
}

#[test]
fn config_reads_icon_mode_from_file() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let config_dir = tempdir.path().join("agentscan");
    std::fs::create_dir_all(&config_dir).expect("config dir should be created");
    std::fs::write(config_dir.join("config.toml"), "icons = \"nerd-font\"\n")
        .expect("config file should be written");
    let source = config::ConfigSource {
        xdg_config_home: Some(tempdir.path().to_path_buf()),
        ..config::ConfigSource::default()
    };

    let resolved =
        config::resolve_config_from_source(&source).expect("config file should be parsed");

    assert_eq!(resolved.icons, IconMode::NerdFont);
}

#[test]
fn runtime_options_default_to_enabled_safety_paths_when_file_is_missing() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let source = config::ConfigSource {
        xdg_config_home: Some(tempdir.path().to_path_buf()),
        ..config::ConfigSource::default()
    };

    let resolved = config::resolve_runtime_options_from_source(&source)
        .expect("missing config should be accepted");

    assert!(!resolved.disable_reconcile);
    assert!(!resolved.disable_proc_fallback);
    assert_eq!(
        resolved.config_path.as_deref(),
        Some(tempdir.path().join("agentscan/config.toml").as_path())
    );
}

#[test]
fn runtime_options_read_diagnostic_toggles_from_file() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let config_dir = tempdir.path().join("agentscan");
    std::fs::create_dir_all(&config_dir).expect("config dir should be created");
    std::fs::write(
        config_dir.join("config.toml"),
        "disable_reconcile = true\ndisable_proc_fallback = true\n",
    )
    .expect("config file should be written");
    let source = config::ConfigSource {
        xdg_config_home: Some(tempdir.path().to_path_buf()),
        ..config::ConfigSource::default()
    };

    let resolved =
        config::resolve_runtime_options_from_source(&source).expect("config should be parsed");

    assert!(resolved.disable_reconcile);
    assert!(resolved.disable_proc_fallback);
}

#[test]
fn runtime_options_env_overrides_file_toggles() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let config_dir = tempdir.path().join("agentscan");
    std::fs::create_dir_all(&config_dir).expect("config dir should be created");
    std::fs::write(
        config_dir.join("config.toml"),
        "disable_reconcile = true\ndisable_proc_fallback = true\n",
    )
    .expect("config file should be written");
    let source = config::ConfigSource {
        env_disable_reconcile: Some("false".to_string()),
        env_disable_proc_fallback: Some("0".to_string()),
        xdg_config_home: Some(tempdir.path().to_path_buf()),
        ..config::ConfigSource::default()
    };

    let resolved =
        config::resolve_runtime_options_from_source(&source).expect("config should be parsed");

    assert!(!resolved.disable_reconcile);
    assert!(!resolved.disable_proc_fallback);
}

#[test]
fn icon_mode_precedence_is_cli_env_config_default() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let config_dir = tempdir.path().join("agentscan");
    std::fs::create_dir_all(&config_dir).expect("config dir should be created");
    std::fs::write(config_dir.join("config.toml"), "icons = \"nerd-font-patched\"\n")
        .expect("config file should be written");

    let source = config::ConfigSource {
        cli: config::CliConfigOverrides {
            icons: Some(IconMode::Emoji),
        },
        env_icons: Some("nerd-font".to_string()),
        xdg_config_home: Some(tempdir.path().to_path_buf()),
        ..config::ConfigSource::default()
    };

    let resolved =
        config::resolve_config_from_source(&source).expect("config should resolve from cli");

    assert_eq!(resolved.icons, IconMode::Emoji);

    let source_without_cli = config::ConfigSource {
        cli: config::CliConfigOverrides::default(),
        ..source
    };

    let resolved = config::resolve_config_from_source(&source_without_cli)
        .expect("config should resolve from env");

    assert_eq!(resolved.icons, IconMode::NerdFont);

    let source_without_env = config::ConfigSource {
        env_icons: None,
        ..source_without_cli
    };

    let resolved = config::resolve_config_from_source(&source_without_env)
        .expect("config should resolve from file");

    assert_eq!(resolved.icons, IconMode::NerdFontPatched);
}

#[test]
fn cli_icon_override_bypasses_invalid_config_file() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let config_dir = tempdir.path().join("agentscan");
    std::fs::create_dir_all(&config_dir).expect("config dir should be created");
    std::fs::write(config_dir.join("config.toml"), "icons = \"symbols\"\n")
        .expect("config file should be written");
    let source = config::ConfigSource {
        cli: config::CliConfigOverrides {
            icons: Some(IconMode::Emoji),
        },
        xdg_config_home: Some(tempdir.path().to_path_buf()),
        ..config::ConfigSource::default()
    };

    let resolved =
        config::resolve_config_from_source(&source).expect("cli override should bypass config");

    assert_eq!(resolved.icons, IconMode::Emoji);
}

#[test]
fn env_icon_override_bypasses_invalid_config_file() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let config_dir = tempdir.path().join("agentscan");
    std::fs::create_dir_all(&config_dir).expect("config dir should be created");
    std::fs::write(config_dir.join("config.toml"), "icons = \"symbols\"\n")
        .expect("config file should be written");
    let source = config::ConfigSource {
        env_icons: Some("nerd-font".to_string()),
        xdg_config_home: Some(tempdir.path().to_path_buf()),
        ..config::ConfigSource::default()
    };

    let resolved =
        config::resolve_config_from_source(&source).expect("env override should bypass config");

    assert_eq!(resolved.icons, IconMode::NerdFont);
}

#[test]
fn empty_icon_env_is_ignored() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let config_dir = tempdir.path().join("agentscan");
    std::fs::create_dir_all(&config_dir).expect("config dir should be created");
    std::fs::write(config_dir.join("config.toml"), "icons = \"nerd-font\"\n")
        .expect("config file should be written");
    let source = config::ConfigSource {
        env_icons: Some("  ".to_string()),
        xdg_config_home: Some(tempdir.path().to_path_buf()),
        ..config::ConfigSource::default()
    };

    let resolved =
        config::resolve_config_from_source(&source).expect("empty env should be ignored");

    assert_eq!(resolved.icons, IconMode::NerdFont);
}

#[test]
fn invalid_icon_env_reports_allowed_values() {
    let source = config::ConfigSource {
        env_icons: Some("symbols".to_string()),
        ..config::ConfigSource::default()
    };

    let error =
        config::resolve_config_from_source(&source).expect_err("invalid env should be rejected");

    assert!(error.to_string().contains(ICONS_ENV_VAR));
    assert!(error.to_string().contains("nerd-font-patched"));
}

#[test]
fn invalid_config_file_reports_path() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let config_dir = tempdir.path().join("agentscan");
    let config_path = config_dir.join("config.toml");
    std::fs::create_dir_all(&config_dir).expect("config dir should be created");
    std::fs::write(&config_path, "icons = \"symbols\"\n").expect("config file should be written");
    let source = config::ConfigSource {
        xdg_config_home: Some(tempdir.path().to_path_buf()),
        ..config::ConfigSource::default()
    };

    let error =
        config::resolve_config_from_source(&source).expect_err("invalid config should be rejected");
    let message = format!("{error:#}");

    assert!(message.contains(config_path.to_str().expect("path should be UTF-8")));
    assert!(message.contains("unknown variant"));
}
