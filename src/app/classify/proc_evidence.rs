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

    if process_has_aider_module_invocation(process) {
        return Some(ProviderMatch::single_reason(
            Provider::Aider,
            ClassificationMatchKind::ProcProcessTree,
            ClassificationConfidence::High,
            format!("{source_reason_prefix}_argv=python -m aider"),
        ));
    }

    if let Some(arg) = process_aider_console_script_arg(process) {
        return Some(proc_provider_arg_match(
            Provider::Aider,
            source_reason_prefix,
            arg,
        ));
    }

    // Normalize every argv entry exactly once; the arg-pattern checks below (aider
    // package path, the Claude package path, and the provider table) all match against
    // these normalized entries instead of re-normalizing the argv per provider.
    let normalized_args = normalized_proc_args(process);

    if let Some(arg) = find_provider_arg(process, &normalized_args, &AIDER_PACKAGE_ARG_PATTERNS) {
        return Some(proc_provider_arg_match(
            Provider::Aider,
            source_reason_prefix,
            arg,
        ));
    }

    if process_has_claude_proc_shape(process, &normalized_args) {
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

    // Ordered arg-path providers. Order is significant (it decides the winner when a
    // process's argv matches more than one pattern set), and it stays after the Claude
    // checks above so Claude keeps priority over Gemini. Adding a provider here is one
    // row plus its `*_ARG_PATTERNS` const.
    for &(provider, patterns) in PROVIDER_ARG_PATTERN_TABLE {
        if let Some(arg) = find_provider_arg(process, &normalized_args, patterns) {
            return Some(proc_provider_arg_match(provider, source_reason_prefix, arg));
        }
    }

    if process_has_opencode_env(process) {
        return Some(proc_provider_env_match(
            Provider::Opencode,
            source_reason_prefix,
            "OPENCODE",
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

/// Ordered table of providers identified purely by an argv path pattern. The order is a
/// classification contract: the first matching row wins, so it mirrors the historic
/// hand-ordered if-chain. Providers with extra signals (Aider's Python entrypoints, Claude's
/// binary shape and reason handling, and the OpenCode/Pi env checks) stay as explicit checks
/// around this loop rather than joining the table.
const PROVIDER_ARG_PATTERN_TABLE: &[(Provider, &ProcArgPathPatterns)] = &[
    (Provider::Gemini, &GEMINI_ARG_PATTERNS),
    (Provider::Opencode, &OPENCODE_ARG_PATTERNS),
    (Provider::Copilot, &COPILOT_ARG_PATTERNS),
    (Provider::Pi, &PI_ARG_PATTERNS),
    (Provider::Hermes, &HERMES_ARG_PATTERNS),
];

fn normalized_proc_args(process: &proc::ProcessEvidence) -> Vec<String> {
    process
        .argv
        .iter()
        .map(|arg| normalize_proc_arg(arg))
        .collect()
}

/// Returns the first argv entry whose normalized form matches `patterns`, reported as the
/// original (un-normalized) argv string so diagnostics keep the process's real path.
fn find_provider_arg<'a>(
    process: &'a proc::ProcessEvidence,
    normalized_args: &[String],
    patterns: &ProcArgPathPatterns,
) -> Option<&'a str> {
    normalized_args
        .iter()
        .zip(process.argv.iter())
        .find(|(normalized, _raw)| arg_matches_patterns(normalized, patterns))
        .map(|(_normalized, raw)| raw.as_str())
}

fn process_has_claude_proc_shape(
    process: &proc::ProcessEvidence,
    normalized_args: &[String],
) -> bool {
    process.argv.first().is_some_and(|arg| {
        claude_argv0_has_binary_shape(arg)
            || command_basename(arg).is_some_and(|command| command.eq_ignore_ascii_case("claude"))
    }) || normalized_args
        .iter()
        .any(|arg| arg_matches_patterns(arg, &CLAUDE_ARG_PATTERNS))
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

struct ProcArgPathPatterns {
    contains: &'static [&'static str],
    suffixes: &'static [&'static str],
    node_module_files: &'static [NodeModuleFilePattern],
    bin_shims: &'static [BinShimPattern],
}

struct NodeModuleFilePattern {
    package: &'static str,
    relative_path: &'static str,
}

struct BinShimPattern {
    binary: &'static str,
    direct_bin_dirs: &'static [&'static str],
    allow_node_manager_bin: bool,
}

const HOMEBREW_LOCAL_BIN_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin"];
const VOLTA_SYSTEM_BIN_DIRS: &[&str] = &[
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/usr/bin",
    "/.volta/bin",
];

const CLAUDE_ARGV0_PATTERNS: ProcArgPathPatterns = ProcArgPathPatterns {
    contains: CLAUDE_ARG_CONTAINS,
    suffixes: &["/claude", "/claude-code", "/node_modules/.bin/claude"],
    node_module_files: &[],
    bin_shims: &[],
};

const CLAUDE_ARG_CONTAINS: &[&str] = &["/node_modules/@anthropic-ai/claude-code/"];

const CLAUDE_ARG_PATTERNS: ProcArgPathPatterns = ProcArgPathPatterns {
    contains: CLAUDE_ARG_CONTAINS,
    suffixes: &[],
    node_module_files: &[],
    bin_shims: &[],
};

const GEMINI_ARG_PATTERNS: ProcArgPathPatterns = ProcArgPathPatterns {
    contains: &[
        "/node_modules/@google/gemini-cli/dist/index.js",
        "/node_modules/@google/gemini-cli/bundle/gemini.js",
    ],
    suffixes: &[
        "/node_modules/@google/gemini-cli",
        "/gemini-cli/packages/cli/index.ts",
        "/gemini-cli/packages/cli/dist/index.js",
        "/gemini-cli/bundle/gemini.js",
        "/gemini-cli/sea/sea-launch.cjs",
    ],
    node_module_files: &[],
    bin_shims: &[BinShimPattern {
        binary: "gemini",
        direct_bin_dirs: VOLTA_SYSTEM_BIN_DIRS,
        allow_node_manager_bin: true,
    }],
};

const PI_ARG_PATTERNS: ProcArgPathPatterns = ProcArgPathPatterns {
    contains: &["/node_modules/@mariozechner/pi-coding-agent/"],
    suffixes: &[
        "/pi-mono/packages/coding-agent/dist/cli.js",
        "/pi-mono/packages/coding-agent/dist/pi",
    ],
    node_module_files: &[],
    bin_shims: &[BinShimPattern {
        binary: "pi",
        direct_bin_dirs: HOMEBREW_LOCAL_BIN_DIRS,
        allow_node_manager_bin: false,
    }],
};

const HERMES_ARG_PATTERNS: ProcArgPathPatterns = ProcArgPathPatterns {
    contains: &["/.hermes/hermes-agent/", "/site-packages/hermes_cli/"],
    suffixes: &[
        "/.local/bin/hermes",
        "/.local/bin/hermes-agent",
        "/hermes-agent/hermes",
        "/hermes-agent/run_agent.py",
        "/hermes-agent/hermes_cli/main.py",
    ],
    node_module_files: &[],
    bin_shims: &[
        BinShimPattern {
            binary: "hermes",
            direct_bin_dirs: HOMEBREW_LOCAL_BIN_DIRS,
            allow_node_manager_bin: false,
        },
        BinShimPattern {
            binary: "hermes-agent",
            direct_bin_dirs: HOMEBREW_LOCAL_BIN_DIRS,
            allow_node_manager_bin: false,
        },
    ],
};

const AIDER_PACKAGE_ARG_PATTERNS: ProcArgPathPatterns = ProcArgPathPatterns {
    contains: &[],
    suffixes: &[
        "/site-packages/aider/main.py",
        "/site-packages/aider/__main__.py",
        "/aider-chat/bin/aider",
    ],
    node_module_files: &[],
    bin_shims: &[],
};

const AIDER_CONSOLE_SCRIPT_PATTERNS: ProcArgPathPatterns = ProcArgPathPatterns {
    contains: &[],
    suffixes: &[
        "/.local/bin/aider",
        "/.venv/bin/aider",
        "/venv/bin/aider",
        "/aider-chat/bin/aider",
    ],
    node_module_files: &[],
    bin_shims: &[BinShimPattern {
        binary: "aider",
        direct_bin_dirs: &[
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/.local/bin",
        ],
        allow_node_manager_bin: false,
    }],
};

const OPENCODE_ARG_PATTERNS: ProcArgPathPatterns = ProcArgPathPatterns {
    contains: &[],
    suffixes: &[
        "/node_modules/opencode/bin/opencode",
        "/node_modules/opencode-ai/bin/opencode",
        "/opencode/packages/opencode/bin/opencode",
        "/opencode/packages/opencode/src/index.ts",
    ],
    node_module_files: &[
        NodeModuleFilePattern {
            package: "opencode-darwin-arm64",
            relative_path: "bin/opencode",
        },
        NodeModuleFilePattern {
            package: "opencode-darwin-x64",
            relative_path: "bin/opencode",
        },
        NodeModuleFilePattern {
            package: "opencode-darwin-x64-baseline",
            relative_path: "bin/opencode",
        },
        NodeModuleFilePattern {
            package: "opencode-linux-arm64",
            relative_path: "bin/opencode",
        },
        NodeModuleFilePattern {
            package: "opencode-linux-arm64-musl",
            relative_path: "bin/opencode",
        },
        NodeModuleFilePattern {
            package: "opencode-linux-x64",
            relative_path: "bin/opencode",
        },
        NodeModuleFilePattern {
            package: "opencode-linux-x64-baseline",
            relative_path: "bin/opencode",
        },
        NodeModuleFilePattern {
            package: "opencode-linux-x64-musl",
            relative_path: "bin/opencode",
        },
        NodeModuleFilePattern {
            package: "opencode-linux-x64-baseline-musl",
            relative_path: "bin/opencode",
        },
        NodeModuleFilePattern {
            package: "opencode-windows-arm64",
            relative_path: "bin/opencode",
        },
        NodeModuleFilePattern {
            package: "opencode-windows-x64",
            relative_path: "bin/opencode",
        },
        NodeModuleFilePattern {
            package: "opencode-windows-x64-baseline",
            relative_path: "bin/opencode",
        },
    ],
    bin_shims: &[BinShimPattern {
        binary: "opencode",
        direct_bin_dirs: VOLTA_SYSTEM_BIN_DIRS,
        allow_node_manager_bin: true,
    }],
};

const COPILOT_ARG_PATTERNS: ProcArgPathPatterns = ProcArgPathPatterns {
    contains: &[],
    suffixes: &[
        "/node_modules/@github/copilot/npm-loader.js",
        "/node_modules/@github/copilot/index.js",
        "/node_modules/@github/copilot/app.js",
    ],
    node_module_files: &[
        NodeModuleFilePattern {
            package: "@github/copilot-darwin-arm64",
            relative_path: "copilot",
        },
        NodeModuleFilePattern {
            package: "@github/copilot-darwin-x64",
            relative_path: "copilot",
        },
        NodeModuleFilePattern {
            package: "@github/copilot-linux-arm64",
            relative_path: "copilot",
        },
        NodeModuleFilePattern {
            package: "@github/copilot-linux-x64",
            relative_path: "copilot",
        },
        NodeModuleFilePattern {
            package: "@github/copilot-win32-arm64",
            relative_path: "copilot",
        },
        NodeModuleFilePattern {
            package: "@github/copilot-win32-x64",
            relative_path: "copilot",
        },
    ],
    bin_shims: &[BinShimPattern {
        binary: "copilot",
        direct_bin_dirs: &[
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/.local/bin",
        ],
        allow_node_manager_bin: true,
    }],
};

fn claude_argv0_has_binary_shape(arg: &str) -> bool {
    let lower = normalize_proc_arg(arg);
    arg_matches_patterns(&lower, &CLAUDE_ARGV0_PATTERNS)
}

fn claude_arg_has_known_package_path(arg: &str) -> bool {
    let lower = normalize_proc_arg(arg);
    arg_matches_patterns(&lower, &CLAUDE_ARG_PATTERNS)
}

fn aider_arg_has_console_script_path(arg: &str) -> bool {
    let lower = normalize_proc_arg(arg);
    arg_matches_patterns(&lower, &AIDER_CONSOLE_SCRIPT_PATTERNS)
}

fn normalize_proc_arg(arg: &str) -> String {
    arg.replace('\\', "/").trim().to_ascii_lowercase()
}

fn arg_ends_with_any(lower: &str, suffixes: &[&str]) -> bool {
    suffixes.iter().any(|suffix| lower.ends_with(suffix))
}

fn arg_contains_any(lower: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| lower.contains(needle))
}

fn arg_matches_patterns(lower: &str, patterns: &ProcArgPathPatterns) -> bool {
    arg_contains_any(lower, patterns.contains)
        || arg_ends_with_any(lower, patterns.suffixes)
        || patterns
            .node_module_files
            .iter()
            .any(|pattern| arg_has_node_module_file(lower, pattern.package, pattern.relative_path))
        || patterns.bin_shims.iter().any(|pattern| {
            arg_has_known_bin_shim_path(
                lower,
                pattern.binary,
                pattern.direct_bin_dirs,
                pattern.allow_node_manager_bin,
            )
        })
}

fn arg_has_node_module_file(lower: &str, package: &str, relative_path: &str) -> bool {
    lower.ends_with(&format!("/node_modules/{package}/{relative_path}"))
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

fn process_has_aider_module_invocation(process: &proc::ProcessEvidence) -> bool {
    process_command_is_python(process) && argv_has_module_invocation(&process.argv, "aider")
}

fn process_aider_console_script_arg(process: &proc::ProcessEvidence) -> Option<&str> {
    if !process_command_is_python(process) {
        return None;
    }
    let script = argv_python_script_operand(&process.argv)?;
    aider_arg_has_console_script_path(script).then_some(script)
}

fn process_command_is_python(process: &proc::ProcessEvidence) -> bool {
    command_is_python(&process.command)
        || process
            .argv
            .first()
            .and_then(|argv0| command_basename(argv0))
            .is_some_and(|command| command_is_python(&command))
}

pub(super) fn command_is_python(command: &str) -> bool {
    let command = command.trim().to_ascii_lowercase();
    let command = match command_basename(&command) {
        Some(basename) => basename,
        None => command,
    };
    if matches!(command.as_str(), "python" | "python3") {
        return true;
    }

    command
        .strip_prefix("python3.")
        .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit()))
}

fn argv_has_module_invocation(argv: &[String], module: &str) -> bool {
    matches!(python_entrypoint(argv), Some(PythonEntrypoint::Module(candidate)) if candidate == module)
}

fn argv_python_script_operand(argv: &[String]) -> Option<&str> {
    match python_entrypoint(argv) {
        Some(PythonEntrypoint::Script(script)) => Some(script),
        Some(PythonEntrypoint::Module(_)) | None => None,
    }
}

enum PythonEntrypoint<'a> {
    Module(&'a str),
    Script(&'a str),
}

fn python_entrypoint(argv: &[String]) -> Option<PythonEntrypoint<'_>> {
    let mut index = if argv
        .first()
        .and_then(|argv0| command_basename(argv0))
        .is_some_and(|command| command_is_python(&command))
    {
        1
    } else {
        0
    };

    while let Some(arg) = argv.get(index).map(String::as_str) {
        if arg == "--" {
            return argv
                .get(index + 1)
                .map(String::as_str)
                .map(PythonEntrypoint::Script);
        }
        if arg == "-m" {
            return argv
                .get(index + 1)
                .map(String::as_str)
                .map(PythonEntrypoint::Module);
        }
        if let Some(module) = arg.strip_prefix("-m")
            && !module.is_empty()
        {
            return Some(PythonEntrypoint::Module(module));
        }
        if matches!(arg, "-" | "-c") {
            return None;
        }
        if !arg.starts_with('-') {
            return Some(PythonEntrypoint::Script(arg));
        }

        if python_option_consumes_next_arg(arg) {
            index += 1;
        }
        index += 1;
    }

    None
}

fn python_option_consumes_next_arg(arg: &str) -> bool {
    matches!(arg, "-Q" | "-W" | "-X" | "--check-hash-based-pycs")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_arg_normalization_handles_case_whitespace_and_windows_separators() {
        assert_eq!(
            normalize_proc_arg("  C:\\Users\\Auro\\AppData\\Roaming\\npm\\opencode  "),
            "c:/users/auro/appdata/roaming/npm/opencode"
        );
    }

    #[test]
    fn node_module_file_matcher_requires_package_and_relative_path_boundary() {
        assert!(arg_has_node_module_file(
            "/work/app/node_modules/opencode/bin/opencode",
            "opencode",
            "bin/opencode"
        ));
        assert!(!arg_has_node_module_file(
            "/work/app/node_modules/opencode-helper/bin/opencode",
            "opencode",
            "bin/opencode"
        ));
        assert!(!arg_has_node_module_file(
            "/work/app/node_modules/opencode/bin/opencode-helper",
            "opencode",
            "bin/opencode"
        ));
    }

    #[test]
    fn node_module_file_matcher_preserves_package_root_executables() {
        assert!(arg_has_node_module_file(
            "/work/app/node_modules/@github/copilot-darwin-arm64/copilot",
            "@github/copilot-darwin-arm64",
            "copilot"
        ));
        assert!(!arg_has_node_module_file(
            "/work/app/node_modules/@github/copilot-darwin-arm64/bin/copilot",
            "@github/copilot-darwin-arm64",
            "copilot"
        ));
    }
}
