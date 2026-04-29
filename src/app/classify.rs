use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TitleHintStrength {
    Weak,
    Strong,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TitleProviderHintKind {
    Explicit,
    Fuzzy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TitleProviderHint {
    provider: Provider,
    strength: TitleHintStrength,
    kind: TitleProviderHintKind,
}

struct TitleAnalysis<'a> {
    raw: &'a str,
    stripped: &'a str,
    has_spinner_glyph: bool,
    has_idle_glyph: bool,
    claude_label: Option<&'a str>,
    opencode_label: Option<&'a str>,
    copilot_label: Option<&'a str>,
    cursor_label: Option<&'a str>,
    pi_label: Option<&'a str>,
    cursor_title_shaped: bool,
    provider_hint: Option<TitleProviderHint>,
    codex_status_title: String,
    codex_normalized_label: String,
    gemini_title: Option<GeminiTitle>,
}

#[derive(Clone, Debug)]
struct GeminiTitle {
    status: Option<StatusKind>,
    label: Option<String>,
    activity_label: Option<String>,
    strong_provider_signal: bool,
}

impl<'a> TitleAnalysis<'a> {
    fn classifyable_provider(&self) -> Option<Provider> {
        self.provider_hint
            .filter(|hint| hint.strength == TitleHintStrength::Strong)
            .map(|hint| hint.provider)
    }

    fn conflicts_with_resolved_provider(
        &self,
        provider: Option<Provider>,
        provider_match_kind: Option<ClassificationMatchKind>,
    ) -> bool {
        if provider_match_kind == Some(ClassificationMatchKind::PaneTitle) {
            return false;
        }

        self.provider_hint.is_some_and(|hint| {
            hint.kind == TitleProviderHintKind::Explicit
                && hint.strength == TitleHintStrength::Strong
                && !matches!(provider, Some(resolved_provider) if resolved_provider == hint.provider)
        })
    }

    fn normalized_label(&self, provider: Option<Provider>) -> Option<String> {
        if self.stripped.is_empty() {
            return None;
        }

        if let Some(stripped) = self.claude_label {
            return Some(stripped.to_string());
        }
        if let Some(stripped) = self.opencode_label {
            return Some(stripped.to_string());
        }
        if matches!(provider, Some(Provider::Copilot))
            && let Some(stripped) = self.copilot_label
        {
            return Some(stripped.to_string());
        }
        if matches!(provider, Some(Provider::CursorCli))
            && let Some(stripped) = self.cursor_label
        {
            return Some(stripped.to_string());
        }
        if matches!(provider, Some(Provider::Pi))
            && let Some(stripped) = self.pi_label
        {
            return Some(stripped.to_string());
        }

        if matches!(provider, Some(Provider::Codex)) {
            return Some(self.codex_normalized_label.clone());
        }
        if matches!(provider, Some(Provider::Gemini))
            && let Some(label) = self
                .gemini_title
                .as_ref()
                .and_then(|title| title.label.clone())
        {
            return Some(label);
        }

        Some(self.stripped.to_string())
    }
}

fn analyze_title(raw_title: &str) -> TitleAnalysis<'_> {
    let raw = raw_title.trim();
    let stripped = strip_known_status_glyph(raw).trim();
    let has_spinner_glyph = has_spinner_glyph(raw);
    let has_idle_glyph = has_idle_glyph(raw);
    let claude_label = strip_claude_title_prefix(stripped);
    let opencode_label = strip_opencode_title_prefix(stripped);
    let copilot_label = strip_copilot_title_prefix(stripped);
    let cursor_label = strip_cursor_cli_title_prefix(stripped);
    let cursor_title_shaped = cursor_label.is_some()
        || stripped.eq_ignore_ascii_case("Cursor Agent")
        || stripped.eq_ignore_ascii_case("Cursor CLI")
        || stripped.eq_ignore_ascii_case("Cursor");
    let pi_label = looks_like_pi_title(stripped)
        .then_some(())
        .and_then(|_| strip_pi_title_prefix(stripped));
    let gemini_title = parse_gemini_terminal_title(stripped);

    let provider_hint = if claude_label.is_some() || stripped == "Claude Code" {
        Some(TitleProviderHint {
            provider: Provider::Claude,
            strength: TitleHintStrength::Strong,
            kind: TitleProviderHintKind::Explicit,
        })
    } else if opencode_label.is_some() || stripped == "OpenCode" {
        Some(TitleProviderHint {
            provider: Provider::Opencode,
            strength: TitleHintStrength::Strong,
            kind: TitleProviderHintKind::Explicit,
        })
    } else if looks_like_codex_title(stripped) {
        Some(TitleProviderHint {
            provider: Provider::Codex,
            strength: TitleHintStrength::Strong,
            kind: TitleProviderHintKind::Explicit,
        })
    } else if copilot_label.is_some() || stripped.eq_ignore_ascii_case("GitHub Copilot") {
        Some(TitleProviderHint {
            provider: Provider::Copilot,
            strength: TitleHintStrength::Strong,
            kind: TitleProviderHintKind::Explicit,
        })
    } else if cursor_title_shaped {
        Some(TitleProviderHint {
            provider: Provider::CursorCli,
            strength: TitleHintStrength::Strong,
            kind: TitleProviderHintKind::Explicit,
        })
    } else if pi_label.is_some() {
        Some(TitleProviderHint {
            provider: Provider::Pi,
            strength: if stripped.starts_with("π - ") || has_spinner_glyph {
                TitleHintStrength::Strong
            } else {
                TitleHintStrength::Weak
            },
            kind: TitleProviderHintKind::Explicit,
        })
    } else if gemini_title
        .as_ref()
        .is_some_and(|title| title.strong_provider_signal)
    {
        Some(TitleProviderHint {
            provider: Provider::Gemini,
            strength: TitleHintStrength::Strong,
            kind: TitleProviderHintKind::Explicit,
        })
    } else if stripped.to_ascii_lowercase().contains("gemini") {
        Some(TitleProviderHint {
            provider: Provider::Gemini,
            strength: TitleHintStrength::Weak,
            kind: TitleProviderHintKind::Fuzzy,
        })
    } else {
        None
    };

    let codex_status_title = normalize_codex_title_before_status(stripped);
    let codex_normalized_label = normalize_codex_terminal_title_label(&codex_status_title);

    TitleAnalysis {
        raw,
        stripped,
        has_spinner_glyph,
        has_idle_glyph,
        claude_label,
        opencode_label,
        copilot_label,
        cursor_label,
        pi_label,
        cursor_title_shaped,
        provider_hint,
        codex_status_title,
        codex_normalized_label,
        gemini_title,
    }
}

pub(crate) fn pane_from_row(row: TmuxPaneRow) -> PaneRecord {
    let agent_metadata = AgentMetadata {
        provider: row.agent_provider.clone(),
        label: row.agent_label.clone(),
        cwd: row.agent_cwd.clone(),
        state: row.agent_state.clone(),
        session_id: row.agent_session_id.clone(),
    };
    let title_analysis = analyze_title(&row.pane_title_raw);
    let current_command = current_command_for_analysis(&row.pane_current_command);
    let provider_match = classify_provider_from_analysis(
        agent_metadata.provider.as_deref(),
        current_command,
        &title_analysis,
    );
    let provider = provider_match.as_ref().map(|matched| matched.provider);
    let title_status = infer_title_status_from_analysis(
        provider,
        provider_match.as_ref().map(|matched| matched.matched_by),
        &title_analysis,
    );
    let status = infer_status(title_status, agent_metadata.state.as_deref());

    PaneRecord {
        pane_id: row.pane_id,
        location: PaneLocation {
            session_name: row.session_name,
            window_index: row.window_index,
            pane_index: row.pane_index,
            window_name: row.window_name.clone(),
        },
        tmux: TmuxPaneMetadata {
            pane_pid: row.pane_pid,
            pane_tty: row.pane_tty,
            pane_current_path: row.pane_current_path,
            pane_current_command: row.pane_current_command.clone(),
            pane_title_raw: row.pane_title_raw.clone(),
            session_id: row.session_id.clone(),
            window_id: row.window_id.clone(),
        },
        display: display_metadata_from_analysis(
            &title_analysis,
            provider,
            provider_match.as_ref().map(|matched| matched.matched_by),
            agent_metadata.label.as_deref(),
            current_command,
            &row.window_name,
        ),
        provider,
        status,
        classification: PaneClassification {
            matched_by: provider_match.as_ref().map(|matched| matched.matched_by),
            confidence: provider_match.as_ref().map(|matched| matched.confidence),
            reasons: provider_match
                .map(|matched| matched.reasons)
                .unwrap_or_default(),
        },
        agent_metadata,
        diagnostics: PaneDiagnostics {
            cache_origin: "direct_snapshot".to_string(),
            proc_fallback: ProcFallbackDiagnostics::default(),
        },
    }
}

pub(crate) fn panes_from_rows_with_proc_fallback(
    rows: Vec<TmuxPaneRow>,
    inspector: &impl proc::ProcessInspector,
) -> Vec<PaneRecord> {
    rows.into_iter()
        .map(|row| {
            let mut pane = pane_from_row(row);
            apply_proc_fallback(&mut pane, inspector);
            pane
        })
        .collect()
}

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

pub(crate) fn pane_output_status_fallback_candidate(pane: &PaneRecord) -> bool {
    matches!(pane.provider, Some(Provider::Copilot))
        && pane.status.kind == StatusKind::Unknown
        && pane.status.source == StatusSource::NotChecked
}

pub(crate) fn apply_pane_output_status_fallback(pane: &mut PaneRecord, output: &str) {
    if !pane_output_status_fallback_candidate(pane) {
        return;
    }

    if matches!(pane.provider, Some(Provider::Copilot))
        && copilot_pane_output_indicates_busy(output)
    {
        pane.status = PaneStatus {
            kind: StatusKind::Busy,
            source: StatusSource::PaneOutput,
        };
    }
}

fn copilot_pane_output_indicates_busy(output: &str) -> bool {
    copilot_current_status_line(output).is_some_and(|line| line.contains("Thinking (Esc to cancel"))
        || copilot_current_trust_prompt_visible(output)
}

fn copilot_current_status_line(output: &str) -> Option<&str> {
    let lines: Vec<&str> = output.lines().collect();
    let prompt_index = lines.iter().rposition(|line| line.trim() == "❯")?;
    let context_index = lines[..prompt_index]
        .iter()
        .rposition(|line| copilot_prompt_context_line(line))?;

    let status_line = lines[..context_index].last()?.trim();
    (!status_line.is_empty()).then_some(status_line)
}

fn copilot_prompt_context_line(line: &str) -> bool {
    let line = line.trim();
    (line.starts_with('/') || line.starts_with("~/")) && line.contains('[') && line.contains(']')
}

fn copilot_current_trust_prompt_visible(output: &str) -> bool {
    let lines: Vec<&str> = output.lines().collect();
    let Some(modal_index) = lines
        .iter()
        .rposition(|line| line.contains("Confirm folder trust"))
    else {
        return false;
    };

    let modal_lines = &lines[modal_index..];
    let normal_prompt_after_modal = modal_lines.iter().any(|line| line.trim() == "❯");
    !normal_prompt_after_modal
        && modal_lines
            .iter()
            .any(|line| line.contains("Do you trust the files in this folder?"))
}

#[derive(Clone)]
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

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn classify_provider(
    published_provider: Option<&str>,
    command: &str,
    title: &str,
) -> Option<ProviderMatch> {
    let title_analysis = analyze_title(title);
    classify_provider_from_analysis(published_provider, command, &title_analysis)
}

fn classify_provider_from_analysis(
    published_provider: Option<&str>,
    command: &str,
    title_analysis: &TitleAnalysis<'_>,
) -> Option<ProviderMatch> {
    let command = current_command_for_analysis(command);

    if let Some(provider) = provider_from_metadata(published_provider) {
        return Some(ProviderMatch {
            provider,
            matched_by: ClassificationMatchKind::PaneMetadata,
            confidence: ClassificationConfidence::High,
            reasons: vec![format!(
                "agent.provider={}",
                published_provider.unwrap_or_default().trim()
            )],
        });
    }

    if let Some((provider, exact)) = provider_from_command(command) {
        return Some(ProviderMatch {
            provider,
            matched_by: ClassificationMatchKind::PaneCurrentCommand,
            confidence: if exact {
                ClassificationConfidence::High
            } else {
                ClassificationConfidence::Medium
            },
            reasons: vec![format!("pane_current_command={command}")],
        });
    }

    if let Some(provider) = title_analysis.classifyable_provider() {
        return Some(ProviderMatch {
            provider,
            matched_by: ClassificationMatchKind::PaneTitle,
            confidence: ClassificationConfidence::High,
            reasons: vec![format!("pane_title={}", title_analysis.raw)],
        });
    }

    if command.eq_ignore_ascii_case("pi") && title_analysis.pi_label.is_some() {
        return Some(ProviderMatch {
            provider: Provider::Pi,
            matched_by: ClassificationMatchKind::PaneCurrentCommand,
            confidence: ClassificationConfidence::Medium,
            reasons: vec![
                format!("pane_current_command={command}"),
                format!("pane_title={}", title_analysis.raw),
            ],
        });
    }

    None
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

fn provider_from_metadata(provider: Option<&str>) -> Option<Provider> {
    let normalized = provider?.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "codex" => Some(Provider::Codex),
        "claude" => Some(Provider::Claude),
        "gemini" => Some(Provider::Gemini),
        "opencode" => Some(Provider::Opencode),
        "copilot" | "github-copilot" | "github copilot" => Some(Provider::Copilot),
        "cursor_cli" | "cursor-cli" | "cursor cli" | "cursor-agent" => Some(Provider::CursorCli),
        "pi" | "pi-coding-agent" | "pi coding agent" => Some(Provider::Pi),
        _ => None,
    }
}

fn provider_from_command(command: &str) -> Option<(Provider, bool)> {
    const CANDIDATES: &[(Provider, &str, bool)] = &[
        (Provider::Codex, "codex", true),
        (Provider::Claude, "claude", true),
        (Provider::Gemini, "gemini", true),
        (Provider::Opencode, "opencode", true),
        (Provider::Copilot, "copilot", false),
        (Provider::Copilot, "github-copilot", false),
        (Provider::CursorCli, "cursor-cli", false),
        (Provider::CursorCli, "cursor-agent", false),
        (Provider::Pi, "pi-coding-agent", false),
    ];

    CANDIDATES
        .iter()
        .find_map(|(provider, name, allow_suffix)| {
            matches_binary(command, name, *allow_suffix).map(|exact| (*provider, exact))
        })
}

fn current_command_for_analysis(command: &str) -> &str {
    let command = command.trim();
    if is_version_like_command(command) {
        ""
    } else {
        command
    }
}

fn is_version_like_command(command: &str) -> bool {
    let command = command.trim();
    let mut parts = command.split('.');
    let Some(first) = parts.next() else {
        return false;
    };
    let Some(second) = parts.next() else {
        return false;
    };
    let Some(third) = parts.next() else {
        return false;
    };

    !first.is_empty()
        && !second.is_empty()
        && !third.is_empty()
        && parts.all(|part| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit()))
        && first.chars().all(|ch| ch.is_ascii_digit())
        && second.chars().all(|ch| ch.is_ascii_digit())
        && third.chars().all(|ch| ch.is_ascii_digit())
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn infer_title_status(
    provider: Option<Provider>,
    provider_match_kind: Option<ClassificationMatchKind>,
    title: &str,
) -> PaneStatus {
    let title_analysis = analyze_title(title);
    infer_title_status_from_analysis(provider, provider_match_kind, &title_analysis)
}

fn infer_title_status_from_analysis(
    provider: Option<Provider>,
    provider_match_kind: Option<ClassificationMatchKind>,
    title_analysis: &TitleAnalysis<'_>,
) -> PaneStatus {
    if title_analysis.conflicts_with_resolved_provider(provider, provider_match_kind) {
        return PaneStatus {
            kind: StatusKind::Unknown,
            source: StatusSource::NotChecked,
        };
    }

    if matches!(provider, Some(Provider::Claude)) {
        if title_analysis.has_spinner_glyph {
            return PaneStatus {
                kind: StatusKind::Busy,
                source: StatusSource::TmuxTitle,
            };
        }
        if title_analysis.has_idle_glyph {
            return PaneStatus {
                kind: StatusKind::Idle,
                source: StatusSource::TmuxTitle,
            };
        }
        if let Some(rest) = title_analysis.claude_label {
            if rest == "Working" || rest.starts_with("Working ") {
                return PaneStatus {
                    kind: StatusKind::Busy,
                    source: StatusSource::TmuxTitle,
                };
            }
            if rest == "Ready" || rest.starts_with("Ready ") {
                return PaneStatus {
                    kind: StatusKind::Idle,
                    source: StatusSource::TmuxTitle,
                };
            }
        }
    }

    if matches!(provider, Some(Provider::Codex)) {
        if title_analysis.has_spinner_glyph {
            return PaneStatus {
                kind: StatusKind::Busy,
                source: StatusSource::TmuxTitle,
            };
        }
        if let Some(status) = codex_run_state_from_title(&title_analysis.codex_status_title) {
            return PaneStatus {
                kind: status,
                source: StatusSource::TmuxTitle,
            };
        }
    }

    if matches!(provider, Some(Provider::Gemini))
        && let Some(status) = title_analysis
            .gemini_title
            .as_ref()
            .and_then(|title| title.status)
    {
        return PaneStatus {
            kind: status,
            source: StatusSource::TmuxTitle,
        };
    }

    if matches!(provider, Some(Provider::Gemini)) {
        match title_analysis.stripped {
            "Ready" => {
                return PaneStatus {
                    kind: StatusKind::Idle,
                    source: StatusSource::TmuxTitle,
                };
            }
            "Working" | "Working…" | "Action Required" => {
                return PaneStatus {
                    kind: StatusKind::Busy,
                    source: StatusSource::TmuxTitle,
                };
            }
            _ => {}
        }
    }

    if matches!(provider, Some(Provider::Copilot))
        && let Some(rest) = title_analysis.copilot_label
    {
        if rest == "Working" || rest.starts_with("Working ") {
            return PaneStatus {
                kind: StatusKind::Busy,
                source: StatusSource::TmuxTitle,
            };
        }
        if rest == "Ready" || rest.starts_with("Ready ") {
            return PaneStatus {
                kind: StatusKind::Idle,
                source: StatusSource::TmuxTitle,
            };
        }
    }

    if matches!(provider, Some(Provider::CursorCli))
        && let Some(rest) = title_analysis.cursor_label
    {
        if rest == "Working" || rest.starts_with("Working ") {
            return PaneStatus {
                kind: StatusKind::Busy,
                source: StatusSource::TmuxTitle,
            };
        }
        if rest == "Ready" || rest.starts_with("Ready ") {
            return PaneStatus {
                kind: StatusKind::Idle,
                source: StatusSource::TmuxTitle,
            };
        }
    }

    if matches!(provider, Some(Provider::Pi))
        && title_analysis.pi_label.is_some()
        && title_analysis.has_spinner_glyph
    {
        return PaneStatus {
            kind: StatusKind::Busy,
            source: StatusSource::TmuxTitle,
        };
    }

    PaneStatus {
        kind: StatusKind::Unknown,
        source: StatusSource::NotChecked,
    }
}

pub(crate) fn infer_status(title_status: PaneStatus, published_state: Option<&str>) -> PaneStatus {
    if title_status.kind != StatusKind::Unknown {
        return title_status;
    }

    match published_state.map(|value| value.trim().to_ascii_lowercase()) {
        Some(state) if state == "busy" => PaneStatus {
            kind: StatusKind::Busy,
            source: StatusSource::PaneMetadata,
        },
        Some(state) if state == "idle" => PaneStatus {
            kind: StatusKind::Idle,
            source: StatusSource::PaneMetadata,
        },
        Some(state) if state == "unknown" => PaneStatus {
            kind: StatusKind::Unknown,
            source: StatusSource::PaneMetadata,
        },
        _ => title_status,
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn display_metadata(
    provider: Option<Provider>,
    provider_match_kind: Option<ClassificationMatchKind>,
    published_label: Option<&str>,
    title: &str,
    current_command: &str,
    window_name: &str,
) -> DisplayMetadata {
    let title_analysis = analyze_title(title);
    display_metadata_from_analysis(
        &title_analysis,
        provider,
        provider_match_kind,
        published_label,
        current_command,
        window_name,
    )
}

fn display_metadata_from_analysis(
    title_analysis: &TitleAnalysis<'_>,
    provider: Option<Provider>,
    provider_match_kind: Option<ClassificationMatchKind>,
    published_label: Option<&str>,
    current_command: &str,
    window_name: &str,
) -> DisplayMetadata {
    if let Some(label) = published_label
        .map(str::trim)
        .filter(|label| !label.is_empty())
    {
        return DisplayMetadata {
            label: label.to_string(),
            activity_label: infer_activity_label(provider, label),
        };
    }

    if let Some(label) = display_label_from_title(
        provider,
        provider_match_kind,
        title_analysis,
        current_command,
    ) {
        let activity_label = if matches!(provider, Some(Provider::Codex)) {
            codex_activity_from_status_title(&title_analysis.codex_status_title)
                .or_else(|| infer_activity_label(provider, &label))
        } else if matches!(provider, Some(Provider::Gemini)) {
            title_analysis
                .gemini_title
                .as_ref()
                .and_then(|title| title.activity_label.clone())
                .or_else(|| {
                    title_analysis
                        .gemini_title
                        .is_none()
                        .then(|| infer_activity_label(provider, &label))
                        .flatten()
                })
        } else if title_activity_should_stay_empty(provider, title_analysis) {
            None
        } else {
            infer_activity_label(provider, &label)
        };
        return DisplayMetadata {
            activity_label,
            label,
        };
    }
    if !window_name.trim().is_empty() {
        return DisplayMetadata {
            label: window_name.trim().to_string(),
            activity_label: None,
        };
    }

    DisplayMetadata {
        label: current_command.trim().to_string(),
        activity_label: None,
    }
}

fn title_activity_should_stay_empty(
    provider: Option<Provider>,
    title_analysis: &TitleAnalysis<'_>,
) -> bool {
    (matches!(provider, Some(Provider::Pi)) && title_analysis.stripped.starts_with("π - "))
        || (matches!(provider, Some(Provider::Opencode))
            && (title_analysis.opencode_label.is_some() || title_analysis.stripped == "OpenCode"))
}

fn display_label_from_title(
    provider: Option<Provider>,
    provider_match_kind: Option<ClassificationMatchKind>,
    title_analysis: &TitleAnalysis<'_>,
    current_command: &str,
) -> Option<String> {
    if title_analysis.conflicts_with_resolved_provider(provider, provider_match_kind) {
        return None;
    }

    let normalized = title_analysis.normalized_label(provider)?;
    if matches!(provider, Some(Provider::CursorCli))
        && cursor_cli_should_fall_back_to_window_name(
            provider_match_kind,
            title_analysis.cursor_title_shaped,
            &normalized,
            current_command,
        )
    {
        return None;
    }

    Some(normalized)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn normalize_title_for_display(provider: Option<Provider>, title: &str) -> String {
    analyze_title(title)
        .normalized_label(provider)
        .unwrap_or_default()
}

fn strip_claude_title_prefix(title: &str) -> Option<&str> {
    title
        .strip_prefix("Claude Code | ")
        .or_else(|| title.strip_prefix("Claude | "))
}

fn strip_opencode_title_prefix(title: &str) -> Option<&str> {
    title.strip_prefix("OC | ")
}

fn strip_copilot_title_prefix(title: &str) -> Option<&str> {
    title
        .strip_prefix("GitHub Copilot | ")
        .or_else(|| title.strip_prefix("Copilot | "))
}

fn strip_cursor_cli_title_prefix(title: &str) -> Option<&str> {
    title
        .strip_prefix("Cursor CLI | ")
        .or_else(|| title.strip_prefix("Cursor Agent | "))
        .or_else(|| title.strip_prefix("Cursor | "))
}

fn strip_pi_title_prefix(title: &str) -> Option<&str> {
    title
        .strip_prefix("π - ")
        .or_else(|| title.strip_prefix("pi - "))
}

fn cursor_cli_should_fall_back_to_window_name(
    provider_match_kind: Option<ClassificationMatchKind>,
    cursor_title_shaped: bool,
    normalized_title: &str,
    current_command: &str,
) -> bool {
    if provider_match_kind == Some(ClassificationMatchKind::PaneTitle) && cursor_title_shaped {
        return false;
    }

    if is_generic_provider_label(Some(Provider::CursorCli), normalized_title)
        || is_generic_status_label(normalized_title)
    {
        return true;
    }

    if provider_match_kind != Some(ClassificationMatchKind::PaneMetadata) && !cursor_title_shaped {
        return true;
    }

    normalized_title.eq_ignore_ascii_case(current_command.trim())
}

fn infer_activity_label(provider: Option<Provider>, label: &str) -> Option<String> {
    let label = label.trim();
    if label.is_empty() {
        return None;
    }

    if is_generic_provider_label(provider, label) {
        return None;
    }

    if matches!(provider, Some(Provider::Codex))
        && let Some(activity) = codex_activity_from_status_title(label)
    {
        return Some(activity);
    }

    if is_generic_status_label(label) {
        return None;
    }

    match provider {
        Some(Provider::Codex) => Some(label.to_string()),
        Some(Provider::Claude)
        | Some(Provider::Gemini)
        | Some(Provider::Opencode)
        | Some(Provider::Copilot)
        | Some(Provider::CursorCli)
        | Some(Provider::Pi) => Some(label.to_string()),
        _ => None,
    }
}

fn is_generic_provider_label(provider: Option<Provider>, label: &str) -> bool {
    match provider {
        Some(Provider::CursorCli) => {
            label.eq_ignore_ascii_case("Cursor Agent")
                || label.eq_ignore_ascii_case("cursor-agent")
                || label.eq_ignore_ascii_case("Cursor CLI")
                || label.eq_ignore_ascii_case("Cursor")
        }
        Some(Provider::Copilot) => label.eq_ignore_ascii_case("GitHub Copilot"),
        Some(Provider::Opencode) => label.eq_ignore_ascii_case("OpenCode"),
        _ => false,
    }
}

fn is_generic_status_label(label: &str) -> bool {
    matches!(
        label.trim(),
        "Working" | "Waiting" | "Thinking" | "Starting" | "Undoing" | "Ready"
    )
}

fn parse_gemini_terminal_title(title: &str) -> Option<GeminiTitle> {
    let title = title.trim();
    if let Some(context) = legacy_gemini_title_context(title) {
        return Some(GeminiTitle {
            status: None,
            label: context,
            activity_label: None,
            strong_provider_signal: true,
        });
    }

    let mut chars = title.chars();
    let glyph = chars.next()?;
    let after_glyph = chars.as_str();
    let rest = after_glyph.trim_start();
    match glyph {
        '◇' => {
            let label = gemini_label_after_status(rest, "Ready");
            let has_context = gemini_status_title_has_context(rest, "Ready");
            Some(GeminiTitle {
                status: label.as_ref().map(|_| StatusKind::Idle),
                activity_label: None,
                strong_provider_signal: has_context,
                label,
            })
        }
        '✋' => {
            let label = gemini_label_after_status(rest, "Action Required");
            let has_context = gemini_status_title_has_context(rest, "Action Required");
            Some(GeminiTitle {
                status: label.as_ref().map(|_| StatusKind::Busy),
                activity_label: None,
                strong_provider_signal: has_context,
                label,
            })
        }
        '⏲' => {
            let label = gemini_label_after_status(rest, "Working…")
                .or_else(|| gemini_label_after_status(rest, "Working"));
            let has_context = gemini_status_title_has_context(rest, "Working…")
                || gemini_status_title_has_context(rest, "Working");
            Some(GeminiTitle {
                status: label.as_ref().map(|_| StatusKind::Busy),
                activity_label: None,
                strong_provider_signal: has_context,
                label,
            })
        }
        '✦' => {
            let (label, activity_label) = gemini_active_title_parts(rest);
            let has_context = split_gemini_activity_context(rest).1.is_some();
            Some(GeminiTitle {
                status: label.as_ref().map(|_| StatusKind::Busy),
                activity_label,
                strong_provider_signal: has_context,
                label,
            })
        }
        _ => None,
    }
}

fn legacy_gemini_title_context(title: &str) -> Option<Option<String>> {
    if title == "Gemini CLI" {
        return Some(None);
    }

    title
        .strip_prefix("Gemini CLI ")
        .and_then(gemini_title_context)
        .map(Some)
}

fn gemini_label_after_status(rest: &str, status_label: &str) -> Option<String> {
    let rest = rest.trim();
    if rest == status_label {
        return Some(status_label.to_string());
    }

    let context = rest.strip_prefix(status_label)?.trim_start();
    if context.is_empty() {
        return Some(status_label.to_string());
    }
    gemini_title_context(context)
}

fn gemini_status_title_has_context(rest: &str, status_label: &str) -> bool {
    rest.trim()
        .strip_prefix(status_label)
        .is_some_and(|context| gemini_title_context(context.trim_start()).is_some())
}

fn gemini_active_title_parts(rest: &str) -> (Option<String>, Option<String>) {
    let rest = rest.trim();
    if rest.is_empty() {
        return (None, None);
    }

    let (activity, context) = split_gemini_activity_context(rest);
    let activity = activity.trim();
    if matches!(activity, "Working" | "Working…") {
        return (context.or_else(|| Some(activity.to_string())), None);
    }
    let activity = activity.to_string();
    (Some(activity.clone()), Some(activity))
}

fn split_gemini_activity_context(rest: &str) -> (&str, Option<String>) {
    if let Some(open_index) = trailing_gemini_context_open_index(rest)
        && let Some(context) = gemini_title_context(&rest[open_index..])
    {
        return (&rest[..open_index], Some(context));
    }

    (rest, None)
}

fn trailing_gemini_context_open_index(value: &str) -> Option<usize> {
    if !value.ends_with(')') {
        return None;
    }

    let mut depth = 0_u32;
    for (index, character) in value.char_indices().rev() {
        match character {
            ')' => depth += 1,
            '(' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    let prefix = &value[..index];
                    return prefix.ends_with(char::is_whitespace).then_some(index);
                }
            }
            _ => {}
        }
    }

    None
}

fn gemini_title_context(value: &str) -> Option<String> {
    let context = value.strip_prefix('(')?.strip_suffix(')')?.trim();
    (!context.is_empty()).then(|| context.to_string())
}

pub(crate) fn strip_known_status_glyph(title: &str) -> &str {
    let trimmed = title.trim_start();
    let Some(first) = trimmed.chars().next() else {
        return trimmed;
    };

    if !(CLAUDE_SPINNER_GLYPHS.contains(&first) || IDLE_GLYPHS.contains(&first)) {
        return trimmed;
    }

    let rest = &trimmed[first.len_utf8()..];
    rest.trim_start()
}

fn has_spinner_glyph(title: &str) -> bool {
    title
        .trim_start()
        .chars()
        .next()
        .is_some_and(|glyph| CLAUDE_SPINNER_GLYPHS.contains(&glyph))
}

fn has_idle_glyph(title: &str) -> bool {
    title
        .trim_start()
        .chars()
        .next()
        .is_some_and(|glyph| IDLE_GLYPHS.contains(&glyph))
}

fn normalize_codex_wrapper_title(title: &str) -> String {
    if title.contains("lgpt.sh")
        && let Some((prefix, _)) = title.rsplit_once(':')
    {
        let prefix = prefix.trim_end();
        if !prefix.is_empty() {
            return prefix.to_string();
        }
    }

    title.to_string()
}

fn normalize_codex_terminal_title_label(title: &str) -> String {
    codex_activity_from_status_title(title).unwrap_or_else(|| {
        let wrapper_label = normalize_codex_wrapper_title(title);
        let command_label = strip_codex_args_from_title(&wrapper_label);
        strip_codex_provider_suffix(&command_label)
    })
}

fn normalize_codex_title_before_status(title: &str) -> String {
    strip_codex_provider_suffix(title)
}

fn codex_activity_from_status_title(title: &str) -> Option<String> {
    if let Some((activity, status)) = title.rsplit_once(" | ")
        && codex_run_state_label(status).is_some()
    {
        let activity = activity.trim();
        if !activity.is_empty() {
            return Some(normalize_codex_activity_label(activity));
        }
    }

    if let Some((status, activity)) = title.split_once(" | ")
        && codex_run_state_label(status).is_some()
    {
        let activity = activity.trim();
        if !activity.is_empty() {
            return Some(normalize_codex_activity_label(activity));
        }
    }

    None
}

fn normalize_codex_activity_label(activity: &str) -> String {
    if !looks_like_codex_title(activity) {
        return activity.to_string();
    }

    let wrapper_label = normalize_codex_wrapper_title(activity);
    let command_label = strip_codex_args_from_title(&wrapper_label);
    strip_codex_provider_suffix(&command_label)
}

fn codex_run_state_from_title(title: &str) -> Option<StatusKind> {
    if let Some(status) = codex_run_state_label(title) {
        return Some(status);
    }
    if let Some((_activity, status)) = title.rsplit_once(" | ")
        && let Some(status) = codex_run_state_label(status)
    {
        return Some(status);
    }
    if let Some((status, _activity)) = title.split_once(" | ")
        && let Some(status) = codex_run_state_label(status)
    {
        return Some(status);
    }

    None
}

fn codex_run_state_label(label: &str) -> Option<StatusKind> {
    match label.trim() {
        "Working" | "Waiting" | "Thinking" | "Starting" | "Undoing" => Some(StatusKind::Busy),
        "Ready" => Some(StatusKind::Idle),
        _ => None,
    }
}

fn strip_codex_args_from_title(title: &str) -> String {
    if let Some((prefix, _suffix)) = title.split_once(" codex ") {
        return format!("{prefix} codex");
    }

    title.to_string()
}

fn strip_codex_provider_suffix(title: &str) -> String {
    if let Some((prefix, suffix)) = title.rsplit_once(':')
        && matches!(suffix.trim(), "gpt" | "codex")
    {
        let prefix = prefix.trim_end();
        if !prefix.is_empty() {
            return prefix.to_string();
        }
    }

    title.to_string()
}

fn matches_binary(command: &str, provider: &str, allow_suffix: bool) -> Option<bool> {
    if command == provider {
        return Some(true);
    }
    if allow_suffix
        && command
            .strip_prefix(provider)
            .is_some_and(|suffix| suffix.starts_with('-'))
    {
        return Some(false);
    }
    None
}

fn command_basename(raw: &str) -> Option<String> {
    Path::new(raw.trim())
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(str::to_string)
}

pub(crate) fn looks_like_codex_title(title: &str) -> bool {
    if title.contains("lgpt.sh") {
        return true;
    }

    let Some((_, suffix)) = title.rsplit_once(':') else {
        return false;
    };

    let suffix = suffix.trim();
    suffix == "codex"
        || suffix.starts_with("codex ")
        || suffix.ends_with("/codex")
        || suffix.ends_with("/codex.sh")
}

fn looks_like_pi_title(title: &str) -> bool {
    if let Some(rest) = title.strip_prefix("π - ") {
        return pi_title_has_nonempty_segments(rest);
    }

    if let Some(rest) = title.strip_prefix("pi - ") {
        return pi_title_has_multiple_segments(rest);
    }

    false
}

fn pi_title_has_nonempty_segments(rest: &str) -> bool {
    rest.split(" - ")
        .map(str::trim)
        .all(|segment| !segment.is_empty())
}

fn pi_title_has_multiple_segments(rest: &str) -> bool {
    let mut segments = rest.split(" - ").map(str::trim);
    let Some(first) = segments.next() else {
        return false;
    };
    let Some(second) = segments.next() else {
        return false;
    };

    if first.is_empty() || second.is_empty() {
        return false;
    }

    segments.all(|segment| !segment.is_empty())
}
