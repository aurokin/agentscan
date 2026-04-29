use super::*;

pub(crate) fn apply_proc_fallback(pane: &mut PaneRecord, inspector: &impl proc::ProcessInspector) {
    if !is_proc_fallback_candidate(pane) {
        pane.diagnostics.proc_fallback = ProcFallbackDiagnostics {
            outcome: ProcFallbackOutcome::Skipped,
            reason: proc_fallback_skip_reason(pane),
            commands: Vec::new(),
        };
        return;
    }

    let evidence = match proc_fallback_evidence(pane, inspector) {
        Ok(evidence) => evidence,
        Err(error) => {
            pane.diagnostics.proc_fallback = ProcFallbackDiagnostics {
                outcome: ProcFallbackOutcome::Error,
                reason: error,
                commands: Vec::new(),
            };
            return;
        }
    };
    let commands: Vec<String> = evidence
        .iter()
        .map(|evidence| evidence.process.command_for_diagnostics())
        .collect();

    let Some(provider_match) = evidence
        .iter()
        .find_map(|evidence| provider_match_from_proc_evidence(&evidence.process, evidence.source))
    else {
        pane.diagnostics.proc_fallback = ProcFallbackDiagnostics {
            outcome: ProcFallbackOutcome::NoMatch,
            reason: proc_fallback_no_match_reason(&evidence),
            commands,
        };
        return;
    };

    apply_provider_match(pane, provider_match);
    pane.diagnostics.proc_fallback = ProcFallbackDiagnostics {
        outcome: ProcFallbackOutcome::Resolved,
        reason: "resolved provider from process evidence".to_string(),
        commands,
    };
}

struct ProcFallbackEvidence {
    source: ProcEvidenceSource,
    process: proc::ProcessEvidence,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ProcEvidenceSource {
    Foreground,
    Descendant,
}

impl ProcEvidenceSource {
    fn reason_prefix(self) -> &'static str {
        match self {
            Self::Foreground => "proc_foreground",
            Self::Descendant => "proc_descendant",
        }
    }
}

fn proc_fallback_evidence(
    pane: &PaneRecord,
    inspector: &impl proc::ProcessInspector,
) -> Result<Vec<ProcFallbackEvidence>, String> {
    let mut foreground = Vec::new();
    let mut descendants = Vec::new();
    let mut errors = Vec::new();

    if proc_fallback_uses_foreground(pane) {
        match inspector.foreground_processes(&pane.tmux.pane_tty) {
            Ok(processes) => foreground = processes,
            Err(error) => errors.push(format!("failed to inspect foreground process: {error}")),
        }
    }

    if proc_fallback_uses_descendants(pane) {
        match inspector.descendant_processes(pane.tmux.pane_pid) {
            Ok(processes) => descendants = processes,
            Err(error) => errors.push(format!("failed to inspect descendants: {error}")),
        }
    }

    if foreground.is_empty() && descendants.is_empty() && !errors.is_empty() {
        return Err(errors.join("; "));
    }

    let evidence = foreground
        .into_iter()
        .map(|process| ProcFallbackEvidence {
            source: ProcEvidenceSource::Foreground,
            process,
        })
        .chain(descendants.into_iter().map(|process| ProcFallbackEvidence {
            source: ProcEvidenceSource::Descendant,
            process,
        }))
        .collect::<Vec<_>>();

    Ok(evidence)
}

fn proc_fallback_no_match_reason(evidence: &[ProcFallbackEvidence]) -> String {
    let has_foreground = evidence
        .iter()
        .any(|evidence| evidence.source == ProcEvidenceSource::Foreground);
    let has_descendant = evidence
        .iter()
        .any(|evidence| evidence.source == ProcEvidenceSource::Descendant);

    match (has_foreground, has_descendant) {
        (true, true) => {
            "no known provider evidence found in foreground process or descendants".to_string()
        }
        (true, false) => "no known provider evidence found in foreground process".to_string(),
        (false, true) => "no known provider evidence found in descendants".to_string(),
        (false, false) => "no process evidence found".to_string(),
    }
}

fn provider_match_from_proc_command(
    command: &str,
    source: ProcEvidenceSource,
) -> Option<ProviderMatch> {
    let command = command.trim();
    let (provider, exact) = provider_from_command(command)?;
    Some(ProviderMatch {
        provider,
        matched_by: ClassificationMatchKind::ProcProcessTree,
        confidence: if exact {
            ClassificationConfidence::High
        } else {
            ClassificationConfidence::Medium
        },
        reasons: vec![format!("{}_command={command}", source.reason_prefix())],
    })
}

fn provider_match_from_proc_evidence(
    process: &proc::ProcessEvidence,
    source: ProcEvidenceSource,
) -> Option<ProviderMatch> {
    if let Some(provider_match) = provider_match_from_proc_command(&process.command, source) {
        return Some(provider_match);
    }

    if let Some(provider_match) = provider_match_from_proc_argv0(process, source) {
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
        return Some(ProviderMatch {
            provider: Provider::Claude,
            matched_by: ClassificationMatchKind::ProcProcessTree,
            confidence: ClassificationConfidence::High,
            reasons: vec![format!(
                "{}_argv={}",
                source.reason_prefix(),
                proc_arg_reason(process)
            )],
        });
    }

    if process_has_claude_teammate_shape(process) {
        return Some(ProviderMatch {
            provider: Provider::Claude,
            matched_by: ClassificationMatchKind::ProcProcessTree,
            confidence: ClassificationConfidence::High,
            reasons: vec![format!(
                "{}_argv=claude teammate flags",
                source.reason_prefix()
            )],
        });
    }

    if let Some(arg) = process
        .argv
        .iter()
        .find(|arg| gemini_arg_has_known_package_path(arg))
    {
        return Some(ProviderMatch {
            provider: Provider::Gemini,
            matched_by: ClassificationMatchKind::ProcProcessTree,
            confidence: ClassificationConfidence::High,
            reasons: vec![format!("{}_argv={arg}", source.reason_prefix())],
        });
    }

    if let Some(arg) = process
        .argv
        .iter()
        .find(|arg| opencode_arg_has_known_package_path(arg))
    {
        return Some(ProviderMatch {
            provider: Provider::Opencode,
            matched_by: ClassificationMatchKind::ProcProcessTree,
            confidence: ClassificationConfidence::High,
            reasons: vec![format!("{}_argv={arg}", source.reason_prefix())],
        });
    }

    if process_has_opencode_env(process) {
        return Some(ProviderMatch {
            provider: Provider::Opencode,
            matched_by: ClassificationMatchKind::ProcProcessTree,
            confidence: ClassificationConfidence::High,
            reasons: vec![format!("{}_env=OPENCODE", source.reason_prefix())],
        });
    }

    if let Some(arg) = process
        .argv
        .iter()
        .find(|arg| pi_arg_has_known_package_path(arg))
    {
        return Some(ProviderMatch {
            provider: Provider::Pi,
            matched_by: ClassificationMatchKind::ProcProcessTree,
            confidence: ClassificationConfidence::High,
            reasons: vec![format!("{}_argv={arg}", source.reason_prefix())],
        });
    }

    if process_has_pi_env(process) {
        return Some(ProviderMatch {
            provider: Provider::Pi,
            matched_by: ClassificationMatchKind::ProcProcessTree,
            confidence: ClassificationConfidence::High,
            reasons: vec![format!("{}_env=PI_CODING_AGENT", source.reason_prefix())],
        });
    }

    None
}

fn provider_match_from_proc_argv0(
    process: &proc::ProcessEvidence,
    source: ProcEvidenceSource,
) -> Option<ProviderMatch> {
    let argv0 = process.argv.first()?;
    let command = command_basename(argv0)?;
    provider_match_from_proc_command(&command, source)
}

fn claude_argv0_has_binary_shape(arg: &str) -> bool {
    let normalized = arg.replace('\\', "/");
    let lower = normalized.trim().to_ascii_lowercase();
    lower.ends_with("/claude")
        || lower.ends_with("/claude-code")
        || lower.ends_with("/node_modules/.bin/claude")
        || claude_arg_has_known_package_path(&lower)
}

fn claude_arg_has_known_package_path(arg: &str) -> bool {
    let normalized = arg.replace('\\', "/");
    let lower = normalized.trim().to_ascii_lowercase();
    lower.contains("/node_modules/@anthropic-ai/claude-code/")
}

fn gemini_arg_has_known_package_path(arg: &str) -> bool {
    let normalized = arg.replace('\\', "/");
    let lower = normalized.trim().to_ascii_lowercase();

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
    lower.ends_with("/node_modules/.bin/gemini")
        || lower.ends_with("/opt/homebrew/bin/gemini")
        || lower.ends_with("/usr/local/bin/gemini")
        || lower.ends_with("/usr/bin/gemini")
        || lower.ends_with("/.volta/bin/gemini")
        || (lower.ends_with("/bin/gemini")
            && (lower.contains("/.nvm/versions/node/")
                || lower.contains("/.nodenv/versions/")
                || lower.contains("/.asdf/installs/nodejs/")
                || lower.contains("/.local/share/mise/installs/node/")))
}

fn pi_arg_has_known_package_path(arg: &str) -> bool {
    let normalized = arg.replace('\\', "/");
    let lower = normalized.trim().to_ascii_lowercase();

    lower.contains("/node_modules/@mariozechner/pi-coding-agent/")
        || lower.ends_with("/pi-mono/packages/coding-agent/dist/cli.js")
        || lower.ends_with("/pi-mono/packages/coding-agent/dist/pi")
        || pi_arg_has_known_bin_shim_path(&lower)
}

fn pi_arg_has_known_bin_shim_path(lower: &str) -> bool {
    lower.ends_with("/node_modules/.bin/pi")
        || lower.ends_with("/opt/homebrew/bin/pi")
        || lower.ends_with("/usr/local/bin/pi")
}

fn opencode_arg_has_known_package_path(arg: &str) -> bool {
    let normalized = arg.replace('\\', "/");
    let lower = normalized.trim().to_ascii_lowercase();

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

fn opencode_arg_has_known_bin_shim_path(lower: &str) -> bool {
    lower.ends_with("/node_modules/.bin/opencode")
        || lower.ends_with("/opt/homebrew/bin/opencode")
        || lower.ends_with("/usr/local/bin/opencode")
        || lower.ends_with("/usr/bin/opencode")
        || lower.ends_with("/.volta/bin/opencode")
        || (lower.ends_with("/bin/opencode")
            && (lower.contains("/.nvm/versions/node/")
                || lower.contains("/.nodenv/versions/")
                || lower.contains("/.asdf/installs/nodejs/")
                || lower.contains("/.local/share/mise/installs/node/")))
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

fn apply_provider_match(pane: &mut PaneRecord, provider_match: ProviderMatch) {
    let title_analysis = analyze_title(&pane.tmux.pane_title_raw);
    let provider = Some(provider_match.provider);
    let match_kind = Some(provider_match.matched_by);
    let title_status = infer_title_status_from_analysis(provider, match_kind, &title_analysis);

    pane.provider = provider;
    pane.status = infer_status(title_status, pane.agent_metadata.state.as_deref());
    pane.display = display_metadata_from_analysis(
        &title_analysis,
        provider,
        match_kind,
        pane.agent_metadata.label.as_deref(),
        current_command_for_analysis(&pane.tmux.pane_current_command),
        &pane.location.window_name,
    );
    pane.classification = PaneClassification {
        matched_by: match_kind,
        confidence: Some(provider_match.confidence),
        reasons: provider_match.reasons,
    };
}

fn is_proc_fallback_candidate(pane: &PaneRecord) -> bool {
    pane.provider.is_none()
        && pane.classification.matched_by.is_none()
        && pane.agent_metadata.provider.is_none()
        && (proc_fallback_uses_foreground(pane) || proc_fallback_uses_descendants(pane))
}

fn proc_fallback_uses_foreground(pane: &PaneRecord) -> bool {
    let title_analysis = analyze_title(&pane.tmux.pane_title_raw);
    let current_command = current_command_for_analysis(&pane.tmux.pane_current_command);

    !pane.tmux.pane_tty.trim().is_empty()
        && !pane.tmux.pane_tty.trim().eq_ignore_ascii_case("not a tty")
        && (is_proc_fallback_launcher_command(current_command)
            || is_shell_or_wrapper_command(current_command)
            || title_analysis.has_spinner_glyph
            || title_analysis.has_idle_glyph)
}

fn proc_fallback_uses_descendants(pane: &PaneRecord) -> bool {
    let title_analysis = analyze_title(&pane.tmux.pane_title_raw);
    let current_command = current_command_for_analysis(&pane.tmux.pane_current_command);

    is_proc_fallback_launcher_command(current_command)
        || title_analysis.has_spinner_glyph
        || title_analysis.has_idle_glyph
}

fn is_proc_fallback_launcher_command(command: &str) -> bool {
    matches!(command, "node" | "bun" | "python3") || command.eq_ignore_ascii_case("pi")
}

fn is_shell_or_wrapper_command(command: &str) -> bool {
    matches!(
        command,
        "sh" | "bash"
            | "zsh"
            | "fish"
            | "dash"
            | "ksh"
            | "nu"
            | "xonsh"
            | "pwsh"
            | "env"
            | "npx"
            | "pnpm"
            | "npm"
            | "yarn"
            | "bunx"
            | "uv"
    )
}

fn proc_fallback_skip_reason(pane: &PaneRecord) -> String {
    if let Some(match_kind) = pane.classification.matched_by {
        return format!(
            "provider already resolved by {}",
            classification_match_kind_name(match_kind)
        );
    }
    if pane.provider.is_some() {
        return "provider already resolved".to_string();
    }
    if pane.agent_metadata.provider.is_some() {
        return "agent.provider metadata is present".to_string();
    }

    if is_version_like_command(&pane.tmux.pane_current_command) {
        return "pane_current_command is version-shaped and ignored".to_string();
    }

    format!(
        "pane_current_command={} is not a targeted proc fallback launcher",
        default_if_empty(pane.tmux.pane_current_command.trim(), "<empty>")
    )
}
