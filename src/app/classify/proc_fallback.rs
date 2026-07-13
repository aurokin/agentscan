use super::*;

#[cfg(test)]
pub(crate) fn apply_proc_fallback(pane: &mut PaneRecord, inspector: &impl proc::ProcessInspector) {
    let snapshot = inspector.snapshot();
    apply_proc_fallback_with_options(pane, &snapshot, false);
}

pub(crate) fn apply_proc_fallback_with_options(
    pane: &mut PaneRecord,
    snapshot: &impl proc::ProcessSnapshot,
    disabled: bool,
) {
    if disabled {
        pane.diagnostics.proc_fallback = ProcFallbackDiagnostics {
            outcome: ProcFallbackOutcome::Skipped,
            reason: "proc fallback disabled by configuration".to_string(),
            commands: Vec::new(),
        };
        return;
    }

    // Analyze the title once and reuse it for the candidate gate, evidence gating,
    // and the resolved-provider derivation below, rather than recomputing (and
    // reallocating) it in each helper.
    let title_analysis = analyze_title(&pane.tmux.pane_title_raw);

    if !is_proc_fallback_candidate(pane, &title_analysis) {
        pane.diagnostics.proc_fallback = ProcFallbackDiagnostics {
            outcome: ProcFallbackOutcome::Skipped,
            reason: proc_fallback_skip_reason(pane),
            commands: Vec::new(),
        };
        return;
    }

    let evidence = match proc_fallback_evidence(pane, snapshot, &title_analysis) {
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

    let derived = provider_match_derived_fields(pane, &title_analysis, provider_match);
    pane.provider = derived.provider;
    pane.status = derived.status;
    pane.display = derived.display;
    pane.classification = derived.classification;
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
    snapshot: &impl proc::ProcessSnapshot,
    title_analysis: &TitleAnalysis<'_>,
) -> Result<Vec<ProcFallbackEvidence>, String> {
    let mut foreground = Vec::new();
    let mut descendants = Vec::new();
    let mut errors = Vec::new();

    if proc_fallback_uses_foreground(pane, title_analysis) {
        match snapshot.foreground_processes(&pane.tmux.pane_tty) {
            Ok(processes) => foreground = processes,
            Err(error) => errors.push(format!("failed to inspect foreground process: {error}")),
        }
    }

    if proc_fallback_uses_descendants(pane, title_analysis) {
        match snapshot.descendant_processes(pane.tmux.pane_pid) {
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

fn provider_match_derived_fields(
    pane: &PaneRecord,
    title_analysis: &TitleAnalysis<'_>,
    provider_match: ProviderMatch,
) -> PaneDerivedFields {
    pane_derived_fields(
        title_analysis,
        Some(provider_match),
        &pane.agent_metadata,
        current_command_for_analysis(&pane.tmux.pane_current_command),
        &pane.location.window_name,
    )
}

fn is_proc_fallback_candidate(pane: &PaneRecord, title_analysis: &TitleAnalysis<'_>) -> bool {
    pane.provider.is_none()
        && pane.classification.matched_by.is_none()
        && pane.agent_metadata.provider.is_none()
        && (proc_fallback_uses_foreground(pane, title_analysis)
            || proc_fallback_uses_descendants(pane, title_analysis))
}

fn proc_fallback_uses_foreground(pane: &PaneRecord, title_analysis: &TitleAnalysis<'_>) -> bool {
    let current_command = current_command_for_analysis(&pane.tmux.pane_current_command);

    !pane.tmux.pane_tty.trim().is_empty()
        && !pane.tmux.pane_tty.trim().eq_ignore_ascii_case("not a tty")
        && (is_proc_fallback_launcher_command(current_command)
            || is_shell_or_wrapper_command(current_command)
            || title_analysis.has_spinner_glyph
            || title_analysis.has_idle_glyph
            || is_version_like_command(&pane.tmux.pane_current_command))
}

fn proc_fallback_uses_descendants(pane: &PaneRecord, title_analysis: &TitleAnalysis<'_>) -> bool {
    let current_command = current_command_for_analysis(&pane.tmux.pane_current_command);

    is_proc_fallback_launcher_command(current_command)
        || title_analysis.has_spinner_glyph
        || title_analysis.has_idle_glyph
        || is_version_like_command(&pane.tmux.pane_current_command)
}

fn is_proc_fallback_launcher_command(command: &str) -> bool {
    matches!(command, "node" | "bun")
        || proc_evidence::command_is_python(command)
        || command.eq_ignore_ascii_case("pi")
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
