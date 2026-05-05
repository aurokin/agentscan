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

    let Some(provider_match) = evidence.iter().find_map(|evidence| {
        proc_evidence::provider_match_from_proc_evidence(
            &evidence.process,
            evidence.source.reason_prefix(),
        )
    }) else {
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

fn apply_provider_match(pane: &mut PaneRecord, provider_match: ProviderMatch) {
    let title_analysis = analyze_title(&pane.tmux.pane_title_raw);
    let derived = pane_derived_fields(
        &title_analysis,
        Some(provider_match),
        &pane.agent_metadata,
        current_command_for_analysis(&pane.tmux.pane_current_command),
        &pane.location.window_name,
    );
    pane.provider = derived.provider;
    pane.status = derived.status;
    pane.display = derived.display;
    pane.classification = derived.classification;
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
            || title_analysis.has_idle_glyph
            || is_version_like_command(&pane.tmux.pane_current_command))
}

fn proc_fallback_uses_descendants(pane: &PaneRecord) -> bool {
    let title_analysis = analyze_title(&pane.tmux.pane_title_raw);
    let current_command = current_command_for_analysis(&pane.tmux.pane_current_command);

    is_proc_fallback_launcher_command(current_command)
        || title_analysis.has_spinner_glyph
        || title_analysis.has_idle_glyph
        || is_version_like_command(&pane.tmux.pane_current_command)
}

fn is_proc_fallback_launcher_command(command: &str) -> bool {
    matches!(command, "node" | "bun")
        || is_python_launcher_command(command)
        || command.eq_ignore_ascii_case("pi")
}

fn is_python_launcher_command(command: &str) -> bool {
    let command = command.trim().to_ascii_lowercase();
    if matches!(command.as_str(), "python" | "python3") {
        return true;
    }

    command
        .strip_prefix("python3.")
        .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit()))
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
        return format!("provider already resolved by {}", match_kind.as_str());
    }
    if pane.provider.is_some() {
        return "provider already resolved".to_string();
    }
    if pane.agent_metadata.provider.is_some() {
        return "agent.provider metadata is present".to_string();
    }

    format!(
        "pane_current_command={} is not a targeted proc fallback launcher",
        default_if_empty(pane.tmux.pane_current_command.trim(), "<empty>")
    )
}
