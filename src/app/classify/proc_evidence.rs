use super::*;

pub(super) fn provider_match_from_proc_evidence(
    process: &proc::ProcessEvidence,
    source_reason_prefix: &str,
) -> Option<ProviderMatch> {
    if let Some(provider_match) =
        provider_match_from_proc_command(&process.command, source_reason_prefix)
    {
        return Some(provider_match);
    }

    if let Some(provider_match) = provider_match_from_proc_argv0(process, source_reason_prefix) {
        return Some(provider_match);
    }

    if process.argv.first().is_some_and(|arg| {
        claude_argv0_has_binary_shape(arg)
            || command_basename(arg).is_some_and(|command| command.eq_ignore_ascii_case("claude"))
    }) || process
        .argv
        .iter()
        .any(|arg| claude_arg_has_known_package_path(arg))
    {
        return Some(ProviderMatch::single_reason(
            Provider::Claude,
            ClassificationMatchKind::ProcProcessTree,
            ClassificationConfidence::High,
            format!("{source_reason_prefix}_argv={}", proc_arg_reason(process)),
        ));
    }

    if process_has_claude_teammate_shape(process) {
        return Some(ProviderMatch::single_reason(
            Provider::Claude,
            ClassificationMatchKind::ProcProcessTree,
            ClassificationConfidence::High,
            format!("{source_reason_prefix}_argv=claude teammate flags"),
        ));
    }

    if let Some(arg) = process
        .argv
        .iter()
        .find(|arg| gemini_arg_has_known_package_path(arg))
    {
        return Some(proc_provider_arg_match(
            Provider::Gemini,
            source_reason_prefix,
            arg,
        ));
    }

    if let Some(arg) = process
        .argv
        .iter()
        .find(|arg| opencode_arg_has_known_package_path(arg))
    {
        return Some(proc_provider_arg_match(
            Provider::Opencode,
            source_reason_prefix,
            arg,
        ));
    }

    if process_has_opencode_env(process) {
        return Some(proc_provider_env_match(
            Provider::Opencode,
            source_reason_prefix,
            "OPENCODE",
        ));
    }

    if let Some(arg) = process
        .argv
        .iter()
        .find(|arg| copilot_arg_has_known_package_path(arg))
    {
        return Some(proc_provider_arg_match(
            Provider::Copilot,
            source_reason_prefix,
            arg,
        ));
    }

    if let Some(arg) = process
        .argv
        .iter()
        .find(|arg| pi_arg_has_known_package_path(arg))
    {
        return Some(proc_provider_arg_match(
            Provider::Pi,
            source_reason_prefix,
            arg,
        ));
    }

    if process_has_pi_env(process) {
        return Some(proc_provider_env_match(
            Provider::Pi,
            source_reason_prefix,
            "PI_CODING_AGENT",
        ));
    }

    None
}

fn provider_match_from_proc_command(
    command: &str,
    source_reason_prefix: &str,
) -> Option<ProviderMatch> {
    let command = command.trim();
    let (provider, exact) = provider_from_command(command)?;
    Some(ProviderMatch::single_reason(
        provider,
        ClassificationMatchKind::ProcProcessTree,
        if exact {
            ClassificationConfidence::High
        } else {
            ClassificationConfidence::Medium
        },
        format!("{source_reason_prefix}_command={command}"),
    ))
}

fn provider_match_from_proc_argv0(
    process: &proc::ProcessEvidence,
    source_reason_prefix: &str,
) -> Option<ProviderMatch> {
    let argv0 = process.argv.first()?;
    let command = command_basename(argv0)?;
    provider_match_from_proc_command(&command, source_reason_prefix)
}

fn proc_provider_arg_match(
    provider: Provider,
    source_reason_prefix: &str,
    arg: &str,
) -> ProviderMatch {
    ProviderMatch::single_reason(
        provider,
        ClassificationMatchKind::ProcProcessTree,
        ClassificationConfidence::High,
        format!("{source_reason_prefix}_argv={arg}"),
    )
}

fn proc_provider_env_match(
    provider: Provider,
    source_reason_prefix: &str,
    env_key: &str,
) -> ProviderMatch {
    ProviderMatch::single_reason(
        provider,
        ClassificationMatchKind::ProcProcessTree,
        ClassificationConfidence::High,
        format!("{source_reason_prefix}_env={env_key}"),
    )
}

fn claude_argv0_has_binary_shape(arg: &str) -> bool {
    let lower = normalize_proc_arg(arg);
    lower.ends_with("/claude")
        || lower.ends_with("/claude-code")
        || lower.ends_with("/node_modules/.bin/claude")
        || claude_arg_has_known_package_path(&lower)
}

fn claude_arg_has_known_package_path(arg: &str) -> bool {
    let lower = normalize_proc_arg(arg);
    lower.contains("/node_modules/@anthropic-ai/claude-code/")
}

fn gemini_arg_has_known_package_path(arg: &str) -> bool {
    let lower = normalize_proc_arg(arg);

    lower.contains("/node_modules/@google/gemini-cli/dist/index.js")
        || lower.contains("/node_modules/@google/gemini-cli/bundle/gemini.js")
        || lower.ends_with("/node_modules/@google/gemini-cli")
        || gemini_arg_has_known_bin_shim_path(&lower)
        || lower.ends_with("/gemini-cli/packages/cli/index.ts")
        || lower.ends_with("/gemini-cli/packages/cli/dist/index.js")
        || lower.ends_with("/gemini-cli/bundle/gemini.js")
        || lower.ends_with("/gemini-cli/sea/sea-launch.cjs")
}

fn gemini_arg_has_known_bin_shim_path(lower: &str) -> bool {
    arg_has_known_bin_shim_path(
        lower,
        "gemini",
        &[
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/.volta/bin",
        ],
        true,
    )
}

fn pi_arg_has_known_package_path(arg: &str) -> bool {
    let lower = normalize_proc_arg(arg);

    lower.contains("/node_modules/@mariozechner/pi-coding-agent/")
        || lower.ends_with("/pi-mono/packages/coding-agent/dist/cli.js")
        || lower.ends_with("/pi-mono/packages/coding-agent/dist/pi")
        || pi_arg_has_known_bin_shim_path(&lower)
}

fn pi_arg_has_known_bin_shim_path(lower: &str) -> bool {
    arg_has_known_bin_shim_path(lower, "pi", &["/opt/homebrew/bin", "/usr/local/bin"], false)
}

fn opencode_arg_has_known_package_path(arg: &str) -> bool {
    let lower = normalize_proc_arg(arg);

    lower.ends_with("/node_modules/opencode/bin/opencode")
        || lower.ends_with("/node_modules/opencode-ai/bin/opencode")
        || opencode_arg_has_platform_package_path(&lower)
        || lower.ends_with("/opencode/packages/opencode/bin/opencode")
        || lower.ends_with("/opencode/packages/opencode/src/index.ts")
        || opencode_arg_has_known_bin_shim_path(&lower)
}

fn opencode_arg_has_platform_package_path(lower: &str) -> bool {
    const PACKAGES: &[&str] = &[
        "opencode-darwin-arm64",
        "opencode-darwin-x64",
        "opencode-darwin-x64-baseline",
        "opencode-linux-arm64",
        "opencode-linux-arm64-musl",
        "opencode-linux-x64",
        "opencode-linux-x64-baseline",
        "opencode-linux-x64-musl",
        "opencode-linux-x64-baseline-musl",
        "opencode-windows-arm64",
        "opencode-windows-x64",
        "opencode-windows-x64-baseline",
    ];

    PACKAGES
        .iter()
        .any(|package| lower.ends_with(&format!("/node_modules/{package}/bin/opencode")))
}

fn copilot_arg_has_known_package_path(arg: &str) -> bool {
    let lower = normalize_proc_arg(arg);

    lower.ends_with("/node_modules/@github/copilot/npm-loader.js")
        || lower.ends_with("/node_modules/@github/copilot/index.js")
        || lower.ends_with("/node_modules/@github/copilot/app.js")
        || copilot_arg_has_platform_package_path(&lower)
        || copilot_arg_has_known_bin_shim_path(&lower)
}

fn copilot_arg_has_platform_package_path(lower: &str) -> bool {
    const PACKAGES: &[&str] = &[
        "copilot-darwin-arm64",
        "copilot-darwin-x64",
        "copilot-linux-arm64",
        "copilot-linux-x64",
        "copilot-win32-arm64",
        "copilot-win32-x64",
    ];

    PACKAGES
        .iter()
        .any(|package| lower.ends_with(&format!("/node_modules/@github/{package}/copilot")))
}

fn copilot_arg_has_known_bin_shim_path(lower: &str) -> bool {
    arg_has_known_bin_shim_path(
        lower,
        "copilot",
        &[
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/.local/bin",
        ],
        true,
    )
}

fn opencode_arg_has_known_bin_shim_path(lower: &str) -> bool {
    arg_has_known_bin_shim_path(
        lower,
        "opencode",
        &[
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/.volta/bin",
        ],
        true,
    )
}

fn normalize_proc_arg(arg: &str) -> String {
    arg.replace('\\', "/").trim().to_ascii_lowercase()
}

fn arg_has_known_bin_shim_path(
    lower: &str,
    binary: &str,
    direct_bin_dirs: &[&str],
    allow_node_manager_bin: bool,
) -> bool {
    lower.ends_with(&format!("/node_modules/.bin/{binary}"))
        || direct_bin_dirs
            .iter()
            .any(|dir| lower.ends_with(&format!("{dir}/{binary}")))
        || (allow_node_manager_bin
            && lower.ends_with(&format!("/bin/{binary}"))
            && arg_has_node_manager_prefix(lower))
}

fn arg_has_node_manager_prefix(lower: &str) -> bool {
    const NODE_MANAGER_PATHS: &[&str] = &[
        "/.nvm/versions/node/",
        "/.nodenv/versions/",
        "/.asdf/installs/nodejs/",
        "/.local/share/mise/installs/node/",
    ];

    NODE_MANAGER_PATHS
        .iter()
        .any(|node_manager_path| lower.contains(node_manager_path))
}

fn claude_arg_for_reason(process: &proc::ProcessEvidence) -> Option<String> {
    process
        .argv
        .first()
        .filter(|arg| claude_argv0_has_binary_shape(arg))
        .cloned()
        .or_else(|| {
            process
                .argv
                .iter()
                .find(|arg| claude_arg_has_known_package_path(arg))
                .cloned()
        })
}

fn process_has_claude_teammate_shape(process: &proc::ProcessEvidence) -> bool {
    let has_teammate_flags = argv_has_flag(&process.argv, "--agent-id")
        && argv_has_flag(&process.argv, "--agent-name")
        && argv_has_flag(&process.argv, "--team-name");
    let has_claudecode_env = process.has_env("CLAUDECODE", "1")
        || process.has_env("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS", "1")
        || argv_has_env_assignment(&process.argv, "CLAUDECODE", "1")
        || argv_has_env_assignment(&process.argv, "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS", "1");

    has_teammate_flags && has_claudecode_env
}

fn process_has_pi_env(process: &proc::ProcessEvidence) -> bool {
    process_env_is_truthy(process, "PI_CODING_AGENT")
}

fn process_has_opencode_env(process: &proc::ProcessEvidence) -> bool {
    if !process_env_is_truthy(process, "OPENCODE") {
        return false;
    }

    process_env_value(process, "OPENCODE_PID")
        .is_some_and(|pid| pid.trim() == process.pid.to_string())
        || process_env_value(process, "OPENCODE_PROCESS_ROLE")
            .is_some_and(|role| matches!(role.trim(), "main" | "worker"))
        || process_env_value(process, "OPENCODE_RUN_ID")
            .is_some_and(|run_id| !run_id.trim().is_empty())
}

fn process_env_value<'a>(process: &'a proc::ProcessEvidence, key: &str) -> Option<&'a str> {
    process
        .env
        .iter()
        .find(|(env_key, _)| env_key == key)
        .map(|(_, value)| value.as_str())
}

fn process_env_is_truthy(process: &proc::ProcessEvidence, key: &str) -> bool {
    process.env.iter().any(|(env_key, value)| {
        env_key == key && matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true")
    })
}

fn argv_has_flag(argv: &[String], flag: &str) -> bool {
    argv_has_token(argv, |token| {
        token == flag
            || token
                .strip_prefix(flag)
                .is_some_and(|rest| rest.starts_with('='))
    })
}

fn argv_has_env_assignment(argv: &[String], key: &str, expected: &str) -> bool {
    argv_has_token(argv, |token| {
        token
            .split_once('=')
            .is_some_and(|(env_key, value)| env_key == key && value == expected)
    })
}

fn argv_has_token(argv: &[String], predicate: impl Fn(&str) -> bool) -> bool {
    argv.iter()
        .any(|arg| predicate(arg) || arg.split_whitespace().any(&predicate))
}

fn proc_arg_reason(process: &proc::ProcessEvidence) -> String {
    claude_arg_for_reason(process)
        .or_else(|| process.argv.first().cloned())
        .unwrap_or_else(|| process.command.clone())
}
