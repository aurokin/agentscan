use super::*;

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

#[cfg(test)]
mod tests {
    use crate::app::proc;
    use crate::app::tests::{
        FakeProcessInspector, assert_unresolved_ambiguous_pane, proc_fallback_pane, tmux_pane_row,
    };
    use crate::app::{Provider, StatusKind};

    #[test]
    fn proc_fallback_leaves_candidate_unknown_without_provider_evidence() {
        let mut pane = tmux_pane_row(700)
            .command("node")
            .title("Working")
            .current_path("/tmp/node-wrapper")
            .pane();
        let inspector =
            FakeProcessInspector::new([(700, vec!["node".to_string(), "helper".to_string()])]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_unresolved_ambiguous_pane(&pane, "Working");
        assert_eq!(
            pane.diagnostics.proc_fallback.outcome,
            crate::app::ProcFallbackOutcome::NoMatch
        );
        assert_eq!(
            pane.diagnostics.proc_fallback.reason,
            "no known provider evidence found in descendants"
        );
        assert_eq!(
            pane.diagnostics.proc_fallback.commands,
            vec!["node".to_string(), "helper".to_string()]
        );
        assert_eq!(inspector.calls(), vec![700]);
    }

    #[test]
    fn proc_fallback_resolves_provider_from_argv0_when_command_is_interpreter() {
        let mut pane = tmux_pane_row(703)
            .command("node")
            .title("Ready")
            .current_path("/tmp/node-wrapper")
            .pane();
        let inspector = FakeProcessInspector::with_processes([(
            703,
            vec![proc::ProcessEvidence {
                pid: 704,
                command: "node".to_string(),
                argv: vec!["codex".to_string(), "/tmp/wrapper.js".to_string()],
                env: Vec::new(),
            }],
        )]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Codex));
        assert_eq!(
            pane.classification.reasons,
            vec!["proc_descendant_command=codex"]
        );
    }

    #[test]
    fn proc_fallback_resolves_claude_from_node_cli_path_and_title_status() {
        let mut pane = tmux_pane_row(704)
            .command("node")
            .title("✳ Refactor auth flow")
            .current_path("/tmp/node-wrapper")
            .pane();
        let inspector = FakeProcessInspector::with_processes([(
            704,
            vec![proc::ProcessEvidence {
                pid: 705,
                command: "node".to_string(),
                argv: vec![
                    "node".to_string(),
                    "/Users/auro/.claude/local/node_modules/@anthropic-ai/claude-code/cli.mjs"
                        .to_string(),
                ],
                env: Vec::new(),
            }],
        )]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Claude));
        assert_eq!(pane.status.kind, StatusKind::Idle);
        assert_eq!(pane.status.source, crate::app::StatusSource::TmuxTitle);
        assert_eq!(pane.display.label, "Refactor auth flow");
        assert_eq!(
        pane.classification.reasons,
        vec![
            "proc_descendant_argv=/Users/auro/.claude/local/node_modules/@anthropic-ai/claude-code/cli.mjs"
                .to_string()
        ]
    );
    }

    #[test]
    fn proc_fallback_resolves_gemini_from_node_cli_path() {
        let mut pane = tmux_pane_row(705)
            .command("node")
            .title("Review deployment plan")
            .current_path("/tmp/node-wrapper")
            .pane();
        let inspector = FakeProcessInspector::with_processes([(
            705,
            vec![proc::ProcessEvidence {
                pid: 706,
                command: "node".to_string(),
                argv: vec![
                    "node".to_string(),
                    "/opt/homebrew/lib/node_modules/@google/gemini-cli/dist/index.js".to_string(),
                ],
                env: Vec::new(),
            }],
        )]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Gemini));
        assert_eq!(pane.display.label, "Review deployment plan");
        assert_eq!(
            pane.classification.reasons,
            vec![
            "proc_descendant_argv=/opt/homebrew/lib/node_modules/@google/gemini-cli/dist/index.js"
                .to_string()
        ]
        );
    }

    #[test]
    fn proc_fallback_resolves_gemini_from_node_bin_shim() {
        let mut pane = tmux_pane_row(706)
            .command("node")
            .title("Review deployment plan")
            .current_path("/tmp/node-wrapper")
            .pane();
        let inspector = FakeProcessInspector::with_processes([(
            706,
            vec![proc::ProcessEvidence {
                pid: 707,
                command: "node".to_string(),
                argv: vec!["node".to_string(), "/opt/homebrew/bin/gemini".to_string()],
                env: Vec::new(),
            }],
        )]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Gemini));
        assert_eq!(
            pane.classification.reasons,
            vec!["proc_descendant_argv=/opt/homebrew/bin/gemini"]
        );
    }

    #[test]
    fn proc_fallback_does_not_treat_arbitrary_gemini_paths_as_gemini() {
        let mut pane = tmux_pane_row(707)
            .command("node")
            .title("Working")
            .current_path("/tmp/node-wrapper")
            .pane();
        let inspector = FakeProcessInspector::with_processes([(
            707,
            vec![proc::ProcessEvidence {
                pid: 708,
                command: "node".to_string(),
                argv: vec!["node".to_string(), "/workspace/tools/gemini".to_string()],
                env: Vec::new(),
            }],
        )]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_unresolved_ambiguous_pane(&pane, "Working");
        assert_eq!(
            pane.diagnostics.proc_fallback.outcome,
            crate::app::ProcFallbackOutcome::NoMatch
        );
    }

    #[test]
    fn proc_fallback_resolves_opencode_from_node_package_shim() {
        let mut pane = proc_fallback_pane(720, "node", "Review plan");
        let inspector = FakeProcessInspector::with_processes([(
            720,
            vec![proc::ProcessEvidence {
                pid: 721,
                command: "node".to_string(),
                argv: vec![
                    "node".to_string(),
                    "/Users/auro/project/node_modules/opencode/bin/opencode".to_string(),
                ],
                env: Vec::new(),
            }],
        )]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Opencode));
        assert_eq!(pane.status.kind, StatusKind::Unknown);
        assert_eq!(pane.display.label, "Review plan");
        assert_eq!(pane.display.activity_label.as_deref(), Some("Review plan"));
        assert_eq!(
            pane.classification.reasons,
            vec!["proc_descendant_argv=/Users/auro/project/node_modules/opencode/bin/opencode"]
        );
    }

    #[test]
    fn proc_fallback_resolves_opencode_from_published_npm_package() {
        let mut pane = proc_fallback_pane(728, "node", "Review plan");
        let inspector = FakeProcessInspector::with_processes([(
            728,
            vec![proc::ProcessEvidence {
                pid: 729,
                command: "node".to_string(),
                argv: vec![
                    "node".to_string(),
                    "/Users/auro/project/node_modules/opencode-ai/bin/opencode".to_string(),
                ],
                env: Vec::new(),
            }],
        )]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Opencode));
        assert_eq!(
            pane.classification.reasons,
            vec!["proc_descendant_argv=/Users/auro/project/node_modules/opencode-ai/bin/opencode"]
        );
    }

    #[test]
    fn proc_fallback_resolves_opencode_from_platform_binary_package() {
        let mut pane = proc_fallback_pane(721, "node", "Review plan");
        let inspector = FakeProcessInspector::with_processes([(
            721,
            vec![proc::ProcessEvidence {
                pid: 722,
                command: "node".to_string(),
                argv: vec![
                    "node".to_string(),
                    "/Users/auro/project/node_modules/opencode-darwin-arm64/bin/opencode"
                        .to_string(),
                ],
                env: Vec::new(),
            }],
        )]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Opencode));
        assert_eq!(
        pane.classification.reasons,
        vec![
            "proc_descendant_argv=/Users/auro/project/node_modules/opencode-darwin-arm64/bin/opencode"
                .to_string()
        ]
    );
    }

    #[test]
    fn proc_fallback_resolves_opencode_from_source_entrypoint() {
        let mut pane = proc_fallback_pane(722, "bun", "Review plan");
        let inspector = FakeProcessInspector::with_processes([(
            722,
            vec![proc::ProcessEvidence {
                pid: 723,
                command: "bun".to_string(),
                argv: vec![
                    "bun".to_string(),
                    "/Users/auro/code/upstream/opencode/packages/opencode/src/index.ts".to_string(),
                ],
                env: Vec::new(),
            }],
        )]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Opencode));
        assert_eq!(pane.display.label, "Review plan");
        assert_eq!(pane.display.activity_label.as_deref(), Some("Review plan"));
        assert_eq!(
            pane.classification.reasons,
            vec![
            "proc_descendant_argv=/Users/auro/code/upstream/opencode/packages/opencode/src/index.ts"
                .to_string()
        ]
        );
    }

    #[test]
    fn proc_fallback_resolves_opencode_from_known_bin_shim() {
        let mut pane = proc_fallback_pane(723, "node", "Review plan");
        let inspector = FakeProcessInspector::with_processes([(
            723,
            vec![proc::ProcessEvidence {
                pid: 724,
                command: "node".to_string(),
                argv: vec!["node".to_string(), "/opt/homebrew/bin/opencode".to_string()],
                env: Vec::new(),
            }],
        )]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Opencode));
        assert_eq!(
            pane.classification.reasons,
            vec!["proc_descendant_argv=/opt/homebrew/bin/opencode"]
        );
    }

    #[test]
    fn proc_fallback_resolves_opencode_from_env_marker() {
        let mut pane = proc_fallback_pane(724, "node", "");
        let inspector = FakeProcessInspector::with_processes([(
            724,
            vec![proc::ProcessEvidence {
                pid: 724,
                command: "node".to_string(),
                argv: vec!["node".to_string()],
                env: vec![
                    ("OPENCODE".to_string(), "1".to_string()),
                    ("OPENCODE_PID".to_string(), "724".to_string()),
                ],
            }],
        )]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Opencode));
        assert_eq!(
            pane.classification.reasons,
            vec!["proc_descendant_env=OPENCODE"]
        );
    }

    #[test]
    fn proc_fallback_resolves_copilot_from_npm_loader_path() {
        let mut pane = proc_fallback_pane(760, "node", "agent wrapper");
        let loader_path = "/Users/auro/.local/share/mise/installs/npm-github-copilot/latest/lib/node_modules/@github/copilot/npm-loader.js";
        let inspector = FakeProcessInspector::with_processes([(
            760,
            vec![proc::ProcessEvidence {
                pid: 761,
                command: "node".to_string(),
                argv: vec![
                    "node".to_string(),
                    loader_path.to_string(),
                    "--yolo".to_string(),
                ],
                env: Vec::new(),
            }],
        )]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Copilot));
        assert_eq!(
            pane.classification.reasons,
            vec![format!("proc_descendant_argv={loader_path}")]
        );
    }

    #[test]
    fn proc_fallback_resolves_copilot_from_platform_package_path() {
        let mut pane = proc_fallback_pane(761, "node", "agent wrapper");
        let native_path = "/Users/auro/.local/share/mise/installs/npm-github-copilot/1.0.39/lib/node_modules/@github/copilot/node_modules/@github/copilot-darwin-arm64/copilot";
        let inspector = FakeProcessInspector::with_processes([(
            761,
            vec![proc::ProcessEvidence {
                pid: 762,
                command: "/Users/auro/.loc".to_string(),
                argv: vec![native_path.to_string(), "--yolo".to_string()],
                env: Vec::new(),
            }],
        )]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Copilot));
        assert_eq!(
            pane.classification.reasons,
            vec!["proc_descendant_command=copilot"]
        );
    }

    #[test]
    fn proc_fallback_does_not_treat_arbitrary_copilot_paths_as_copilot() {
        let mut pane = proc_fallback_pane(762, "node", "agent wrapper");
        let inspector = FakeProcessInspector::with_processes([(
            762,
            vec![proc::ProcessEvidence {
                pid: 763,
                command: "node".to_string(),
                argv: vec![
                    "node".to_string(),
                    "/tmp/copilot-experiment/npm-loader.js".to_string(),
                ],
                env: Vec::new(),
            }],
        )]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, None);
        assert_eq!(
            pane.diagnostics.proc_fallback.outcome,
            crate::app::ProcFallbackOutcome::NoMatch
        );
    }

    #[test]
    fn proc_fallback_does_not_treat_arbitrary_opencode_paths_as_opencode() {
        for (pid, argv_path) in [
            (725, "/workspace/tools/opencode"),
            (
                726,
                "/Users/auro/project/node_modules/opencode/bin/opencode-helper",
            ),
            (
                727,
                "/Users/auro/project/node_modules/opencode-helper/bin/opencode",
            ),
            (
                728,
                "/Users/auro/project/node_modules/opencode-ai-helper/bin/opencode",
            ),
        ] {
            let mut pane = proc_fallback_pane(pid, "node", "Review plan");
            let inspector = FakeProcessInspector::with_processes([(
                pid,
                vec![proc::ProcessEvidence {
                    pid: pid + 100,
                    command: "node".to_string(),
                    argv: vec!["node".to_string(), argv_path.to_string()],
                    env: Vec::new(),
                }],
            )]);

            let snapshot = proc::LazyProcessSnapshot::new(&inspector);
            super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

            assert_unresolved_ambiguous_pane(&pane, "Review plan");
            assert_eq!(
                pane.diagnostics.proc_fallback.outcome,
                crate::app::ProcFallbackOutcome::NoMatch,
                "unexpected opencode match for {argv_path}"
            );
        }
    }

    #[test]
    fn proc_fallback_does_not_treat_opencode_env_text_in_argv_as_opencode() {
        let mut pane = proc_fallback_pane(726, "node", "Review plan");
        let inspector = FakeProcessInspector::with_processes([(
            726,
            vec![proc::ProcessEvidence {
                pid: 727,
                command: "node".to_string(),
                argv: vec![
                    "node".to_string(),
                    "script.js".to_string(),
                    "--data".to_string(),
                    "OPENCODE=1 OPENCODE_PID=727".to_string(),
                ],
                env: Vec::new(),
            }],
        )]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_unresolved_ambiguous_pane(&pane, "Review plan");
        assert_eq!(
            pane.diagnostics.proc_fallback.outcome,
            crate::app::ProcFallbackOutcome::NoMatch
        );
    }

    #[test]
    fn proc_fallback_requires_correlated_opencode_env() {
        let mut pane = proc_fallback_pane(727, "node", "Review plan");
        let inspector = FakeProcessInspector::with_processes([(
            727,
            vec![proc::ProcessEvidence {
                pid: 728,
                command: "node".to_string(),
                argv: vec!["node".to_string()],
                env: vec![
                    ("OPENCODE".to_string(), "1".to_string()),
                    ("AGENT".to_string(), "1".to_string()),
                ],
            }],
        )]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_unresolved_ambiguous_pane(&pane, "Review plan");
        assert_eq!(
            pane.diagnostics.proc_fallback.outcome,
            crate::app::ProcFallbackOutcome::NoMatch
        );
    }

    #[test]
    fn proc_fallback_resolves_pi_from_env_marker() {
        let mut pane = proc_fallback_pane(730, "pi", "");
        let inspector = FakeProcessInspector::with_processes([(
            730,
            vec![proc::ProcessEvidence {
                pid: 730,
                command: "pi".to_string(),
                argv: vec!["pi".to_string()],
                env: vec![("PI_CODING_AGENT".to_string(), "true".to_string())],
            }],
        )]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Pi));
        assert_eq!(
            pane.classification.reasons,
            vec!["proc_descendant_env=PI_CODING_AGENT"]
        );
        assert_eq!(pane.status.kind, StatusKind::Unknown);
    }

    #[test]
    fn proc_fallback_resolves_pi_from_package_cli_path() {
        let mut pane = proc_fallback_pane(731, "node", "Review plan");
        let inspector = FakeProcessInspector::with_processes([(
            731,
            vec![proc::ProcessEvidence {
                pid: 732,
                command: "node".to_string(),
                argv: vec![
                    "node".to_string(),
                    "/opt/homebrew/lib/node_modules/@mariozechner/pi-coding-agent/dist/cli.js"
                        .to_string(),
                ],
                env: Vec::new(),
            }],
        )]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Pi));
        assert_eq!(pane.display.label, "Review plan");
        assert_eq!(
        pane.classification.reasons,
        vec![
            "proc_descendant_argv=/opt/homebrew/lib/node_modules/@mariozechner/pi-coding-agent/dist/cli.js"
                .to_string()
        ]
    );
    }

    #[test]
    fn proc_fallback_resolves_pi_from_build_binary_path() {
        let mut pane = proc_fallback_pane(732, "bun", "Review plan");
        let inspector = FakeProcessInspector::with_processes([(
            732,
            vec![proc::ProcessEvidence {
                pid: 733,
                command: "bun".to_string(),
                argv: vec![
                    "bun".to_string(),
                    "/Users/auro/code/upstream/pi-mono/packages/coding-agent/dist/pi".to_string(),
                ],
                env: Vec::new(),
            }],
        )]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Pi));
        assert_eq!(
            pane.classification.reasons,
            vec![
            "proc_descendant_argv=/Users/auro/code/upstream/pi-mono/packages/coding-agent/dist/pi"
                .to_string()
        ]
        );
    }

    #[test]
    fn proc_fallback_resolves_pi_from_known_bin_shim() {
        let mut pane = proc_fallback_pane(733, "node", "Review plan");
        let inspector = FakeProcessInspector::with_processes([(
            733,
            vec![proc::ProcessEvidence {
                pid: 734,
                command: "node".to_string(),
                argv: vec!["node".to_string(), "/opt/homebrew/bin/pi".to_string()],
                env: Vec::new(),
            }],
        )]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Pi));
        assert_eq!(
            pane.classification.reasons,
            vec!["proc_descendant_argv=/opt/homebrew/bin/pi"]
        );
    }

    #[test]
    fn proc_fallback_does_not_treat_arbitrary_pi_paths_as_pi() {
        let mut pane = proc_fallback_pane(734, "node", "Review plan");
        let inspector = FakeProcessInspector::with_processes([(
            734,
            vec![proc::ProcessEvidence {
                pid: 735,
                command: "node".to_string(),
                argv: vec!["node".to_string(), "/workspace/tools/pi".to_string()],
                env: Vec::new(),
            }],
        )]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_unresolved_ambiguous_pane(&pane, "Review plan");
        assert_eq!(
            pane.diagnostics.proc_fallback.outcome,
            crate::app::ProcFallbackOutcome::NoMatch
        );
    }

    #[test]
    fn proc_fallback_does_not_treat_bare_pi_process_as_pi() {
        let mut pane = proc_fallback_pane(735, "pi", "");
        let inspector = FakeProcessInspector::with_processes([(
            735,
            vec![proc::ProcessEvidence {
                pid: 735,
                command: "pi".to_string(),
                argv: vec!["pi".to_string()],
                env: Vec::new(),
            }],
        )]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, None);
        assert_eq!(
            pane.diagnostics.proc_fallback.outcome,
            crate::app::ProcFallbackOutcome::NoMatch
        );
    }

    #[test]
    fn proc_fallback_does_not_treat_pi_env_text_in_argv_as_pi() {
        let mut pane = proc_fallback_pane(736, "node", "Review plan");
        let inspector = FakeProcessInspector::with_processes([(
            736,
            vec![proc::ProcessEvidence {
                pid: 737,
                command: "node".to_string(),
                argv: vec![
                    "node".to_string(),
                    "script.js".to_string(),
                    "--data".to_string(),
                    "PI_CODING_AGENT=true".to_string(),
                ],
                env: Vec::new(),
            }],
        )]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_unresolved_ambiguous_pane(&pane, "Review plan");
        assert_eq!(
            pane.diagnostics.proc_fallback.outcome,
            crate::app::ProcFallbackOutcome::NoMatch
        );
    }

    #[test]
    fn proc_fallback_resolves_hermes_from_python_bin_shim_path() {
        let mut pane = proc_fallback_pane(763, "python3.11", "agentscan: hermes");
        let inspector = FakeProcessInspector::with_processes([(
            763,
            vec![proc::ProcessEvidence {
                pid: 764,
                command: "/Users/auro/.her".to_string(),
                argv: vec![
                    "/Users/auro/.hermes/hermes-agent/venv/bin/python3".to_string(),
                    "/Users/auro/.local/bin/hermes".to_string(),
                ],
                env: Vec::new(),
            }],
        )]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Hermes));
        assert_eq!(pane.status.kind, StatusKind::Unknown);
        assert_eq!(
            pane.classification.matched_by,
            Some(crate::app::ClassificationMatchKind::ProcProcessTree)
        );
        assert_eq!(
            pane.classification.reasons,
            vec!["proc_descendant_argv=/Users/auro/.hermes/hermes-agent/venv/bin/python3"]
        );
        assert_eq!(
            pane.diagnostics.proc_fallback.outcome,
            crate::app::ProcFallbackOutcome::Resolved
        );
    }

    #[test]
    fn proc_fallback_resolves_hermes_from_user_local_hermes_agent_shim_path() {
        let mut pane = proc_fallback_pane(769, "python3.12", "agentscan: hermes");
        let inspector = FakeProcessInspector::with_processes([(
            769,
            vec![proc::ProcessEvidence {
                pid: 770,
                command: "python3.12".to_string(),
                argv: vec![
                    "/opt/python/bin/python3".to_string(),
                    "/Users/auro/.local/bin/hermes-agent".to_string(),
                ],
                env: Vec::new(),
            }],
        )]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Hermes));
        assert_eq!(
            pane.classification.reasons,
            vec!["proc_descendant_argv=/Users/auro/.local/bin/hermes-agent"]
        );
    }

    #[test]
    fn proc_fallback_resolves_aider_from_python_module_invocation() {
        let mut pane = proc_fallback_pane(770, "python3.12", "aider");
        let inspector = FakeProcessInspector::with_single_process(
            770,
            771,
            "python3.12",
            &[
                "/Users/auro/.local/share/uv/tools/aider-chat/bin/python3",
                "-m",
                "aider",
            ],
        );

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Aider));
        assert_eq!(pane.status.kind, StatusKind::Unknown);
        assert_eq!(
            pane.classification.matched_by,
            Some(crate::app::ClassificationMatchKind::ProcProcessTree)
        );
        assert_eq!(
            pane.classification.reasons,
            vec!["proc_descendant_argv=python -m aider"]
        );
        assert_eq!(
            pane.diagnostics.proc_fallback.outcome,
            crate::app::ProcFallbackOutcome::Resolved
        );
    }

    #[test]
    fn proc_fallback_does_not_treat_script_args_as_aider_module_invocation() {
        let mut pane = proc_fallback_pane(779, "python3.12", "aider");
        let inspector = FakeProcessInspector::with_single_process(
            779,
            780,
            "python3.12",
            &["/usr/bin/python3", "/tmp/helper.py", "-m", "aider"],
        );

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_unresolved_ambiguous_pane(&pane, "aider");
        assert_eq!(
            pane.diagnostics.proc_fallback.outcome,
            crate::app::ProcFallbackOutcome::NoMatch
        );
    }

    #[test]
    fn proc_fallback_does_not_treat_string_args_as_aider_module_invocation() {
        let mut pane = proc_fallback_pane(781, "python3.12", "aider");
        let inspector = FakeProcessInspector::with_single_process(
            781,
            782,
            "python3.12",
            &["/usr/bin/python3", "/tmp/helper.py", "--message=-m aider"],
        );

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_unresolved_ambiguous_pane(&pane, "aider");
        assert_eq!(
            pane.diagnostics.proc_fallback.outcome,
            crate::app::ProcFallbackOutcome::NoMatch
        );
    }

    #[test]
    fn proc_fallback_resolves_aider_from_known_python_console_script_path() {
        let mut pane = proc_fallback_pane(771, "python3.12", "aider");
        let inspector = FakeProcessInspector::with_single_process(
            771,
            772,
            "python3.12",
            &[
                "/Users/auro/.local/share/pipx/venvs/aider-chat/bin/python3",
                "/Users/auro/.local/bin/aider",
            ],
        );

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Aider));
        assert_eq!(
            pane.classification.reasons,
            vec!["proc_descendant_argv=/Users/auro/.local/bin/aider"]
        );
    }

    #[test]
    fn proc_fallback_resolves_aider_from_venv_console_script_path() {
        let mut pane = proc_fallback_pane(783, "python3.12", "aider");
        let inspector = FakeProcessInspector::with_single_process(
            783,
            784,
            "python3.12",
            &[
                "/Users/auro/code/project/.venv/bin/python3",
                "/Users/auro/code/project/.venv/bin/aider",
            ],
        );

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Aider));
        assert_eq!(
            pane.classification.reasons,
            vec!["proc_descendant_argv=/Users/auro/code/project/.venv/bin/aider"]
        );
    }

    #[test]
    fn proc_fallback_does_not_treat_venv_aider_argument_as_console_script() {
        let mut pane = proc_fallback_pane(787, "python3.12", "aider");
        let inspector = FakeProcessInspector::with_single_process(
            787,
            788,
            "python3.12",
            &[
                "/Users/auro/code/project/.venv/bin/python3",
                "-c",
                "print('not aider')",
                "/Users/auro/code/project/.venv/bin/aider",
            ],
        );

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_unresolved_ambiguous_pane(&pane, "aider");
        assert_eq!(
            pane.diagnostics.proc_fallback.outcome,
            crate::app::ProcFallbackOutcome::NoMatch
        );
    }

    #[test]
    fn proc_fallback_resolves_aider_from_site_packages_path() {
        let mut pane = proc_fallback_pane(773, "python3.12", "aider");
        let inspector = FakeProcessInspector::with_single_process(
            773,
            774,
            "python3.12",
            &[
                "/Users/auro/.local/share/pipx/venvs/aider-chat/bin/python3",
                "/Users/auro/.local/share/pipx/venvs/aider-chat/lib/python3.12/site-packages/aider/main.py",
            ],
        );

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Aider));
        assert_eq!(
            pane.classification.reasons,
            vec![
                "proc_descendant_argv=/Users/auro/.local/share/pipx/venvs/aider-chat/lib/python3.12/site-packages/aider/main.py"
            ]
        );
    }

    #[test]
    fn proc_fallback_does_not_treat_arbitrary_aider_module_files_as_aider() {
        let mut pane = proc_fallback_pane(785, "python3.12", "aider");
        let inspector = FakeProcessInspector::with_single_process(
            785,
            786,
            "python3.12",
            &[
                "/Users/auro/.local/share/pipx/venvs/aider-chat/bin/python3",
                "/Users/auro/.local/share/pipx/venvs/aider-chat/lib/python3.12/site-packages/aider/args.py",
            ],
        );

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_unresolved_ambiguous_pane(&pane, "aider");
        assert_eq!(
            pane.diagnostics.proc_fallback.outcome,
            crate::app::ProcFallbackOutcome::NoMatch
        );
    }

    #[test]
    fn proc_fallback_resolves_aider_from_uv_tool_console_script_path() {
        let mut pane = proc_fallback_pane(775, "python3.12", "aider");
        let inspector = FakeProcessInspector::with_single_process(
            775,
            776,
            "python3.12",
            &[
                "/Users/auro/.local/share/uv/tools/aider-chat/bin/python3",
                "/Users/auro/.local/share/uv/tools/aider-chat/bin/aider",
            ],
        );

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Aider));
        assert_eq!(
            pane.classification.reasons,
            vec!["proc_descendant_argv=/Users/auro/.local/share/uv/tools/aider-chat/bin/aider"]
        );
    }

    #[test]
    fn proc_fallback_does_not_treat_arbitrary_aider_paths_as_aider() {
        let mut pane = proc_fallback_pane(777, "python3.12", "aider");
        let inspector = FakeProcessInspector::with_single_process(
            777,
            778,
            "python3.12",
            &["/opt/python/bin/python3", "/workspace/tools/aider"],
        );

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_unresolved_ambiguous_pane(&pane, "aider");
        assert_eq!(
            pane.diagnostics.proc_fallback.outcome,
            crate::app::ProcFallbackOutcome::NoMatch
        );
    }

    #[test]
    fn proc_fallback_resolves_claude_from_title_glyph_and_descendant_command() {
        let mut pane = tmux_pane_row(711)
            .command("2.1.119")
            .title("✳ Analyze Linear Issue AUR-126 and plan implementation")
            .current_path("/tmp/claude-wrapper")
            .pane();
        let inspector = FakeProcessInspector::new([(711, vec!["claude".to_string()])]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Claude));
        assert_eq!(pane.status.kind, StatusKind::Idle);
        assert_eq!(pane.status.source, crate::app::StatusSource::TmuxTitle);
        assert_eq!(
            pane.display.label,
            "Analyze Linear Issue AUR-126 and plan implementation"
        );
        assert_eq!(
            pane.classification.reasons,
            vec!["proc_descendant_command=claude"]
        );
        assert_eq!(inspector.calls(), vec![711]);
    }

    #[test]
    fn proc_fallback_resolves_version_like_current_command_via_process_tree() {
        let mut pane = tmux_pane_row(712)
            .command("2.1.119")
            .title("Ready")
            .current_path("/tmp/claude-wrapper")
            .pane();
        let inspector = FakeProcessInspector::new([(712, vec!["claude".to_string()])]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Claude));
        assert_eq!(
            pane.diagnostics.proc_fallback.outcome,
            crate::app::ProcFallbackOutcome::Resolved
        );
        assert_eq!(
            pane.classification.reasons,
            vec!["proc_descendant_command=claude"]
        );
        assert_eq!(inspector.calls(), vec![712]);
    }

    #[test]
    fn proc_fallback_returns_no_match_for_version_like_command_without_provider_evidence() {
        let mut pane = tmux_pane_row(713)
            .command("2.1.119")
            .title("Ready")
            .current_path("/tmp/unknown")
            .pane();
        let inspector = FakeProcessInspector::new([(713, vec!["unrelated".to_string()])]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_unresolved_ambiguous_pane(&pane, "Ready");
        assert_eq!(
            pane.diagnostics.proc_fallback.outcome,
            crate::app::ProcFallbackOutcome::NoMatch
        );
        assert_eq!(inspector.calls(), vec![713]);
    }

    #[test]
    fn proc_fallback_resolves_claude_teammate_flags_with_claudecode_env() {
        let mut pane = tmux_pane_row(705)
            .command("node")
            .title("worker-a")
            .current_path("/tmp/node-wrapper")
            .pane();
        let inspector = FakeProcessInspector::with_processes([(
            705,
            vec![proc::ProcessEvidence {
                pid: 706,
                command: "node".to_string(),
                argv: vec![
                    "node".to_string(),
                    "/tmp/cli.mjs".to_string(),
                    "--agent-id".to_string(),
                    "worker-a@team".to_string(),
                    "--agent-name".to_string(),
                    "worker-a".to_string(),
                    "--team-name".to_string(),
                    "team".to_string(),
                ],
                env: vec![("CLAUDECODE".to_string(), "1".to_string())],
            }],
        )]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Claude));
        assert_eq!(
            pane.classification.reasons,
            vec!["proc_descendant_argv=claude teammate flags"]
        );
        assert_eq!(pane.display.label, "worker-a");
    }

    #[test]
    fn proc_fallback_resolves_claude_teammate_from_shell_env_assignment() {
        let mut pane = tmux_pane_row(707)
            .command("node")
            .title("worker-a")
            .current_path("/tmp/node-wrapper")
            .pane();
        let inspector = FakeProcessInspector::with_processes([(
            707,
            vec![proc::ProcessEvidence {
                pid: 708,
                command: "sh".to_string(),
                argv: vec![
                "sh".to_string(),
                "-c".to_string(),
                "env CLAUDECODE=1 claude --agent-id worker-a --agent-name worker-a --team-name team"
                    .to_string(),
            ],
                env: Vec::new(),
            }],
        )]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Claude));
        assert_eq!(
            pane.classification.reasons,
            vec!["proc_descendant_argv=claude teammate flags"]
        );
        assert_eq!(pane.display.label, "worker-a");
    }

    #[test]
    fn proc_fallback_does_not_treat_teammate_flags_without_claude_env_as_claude() {
        let mut pane = tmux_pane_row(706)
            .command("node")
            .title("worker-a")
            .current_path("/tmp/node-wrapper")
            .pane();
        let inspector = FakeProcessInspector::with_processes([(
            706,
            vec![proc::ProcessEvidence {
                pid: 707,
                command: "node".to_string(),
                argv: vec![
                    "node".to_string(),
                    "/tmp/not-claude.js".to_string(),
                    "--agent-id=worker-a".to_string(),
                    "--agent-name=worker-a".to_string(),
                    "--team-name=team".to_string(),
                ],
                env: Vec::new(),
            }],
        )]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_unresolved_ambiguous_pane(&pane, "worker-a");
        assert_eq!(
            pane.diagnostics.proc_fallback.outcome,
            crate::app::ProcFallbackOutcome::NoMatch
        );
    }

    #[test]
    fn proc_fallback_does_not_treat_claude_substrings_as_claude() {
        for (pid, argv_path) in [
            (708, "/project/node_modules/.bin/claude-lint"),
            (709, "/work/claude-helper/cli.mjs"),
            (710, "/workspace/tools/claude"),
            (711, "/workspace/tools/claude-code"),
        ] {
            let mut pane = tmux_pane_row(pid)
                .command("node")
                .title("Working")
                .current_path("/tmp/node-wrapper")
                .pane();
            let inspector = FakeProcessInspector::with_processes([(
                pid,
                vec![proc::ProcessEvidence {
                    pid: pid + 100,
                    command: "node".to_string(),
                    argv: vec!["node".to_string(), argv_path.to_string()],
                    env: Vec::new(),
                }],
            )]);

            let snapshot = proc::LazyProcessSnapshot::new(&inspector);
            super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

            assert_unresolved_ambiguous_pane(&pane, "Working");
            assert_eq!(
                pane.diagnostics.proc_fallback.outcome,
                crate::app::ProcFallbackOutcome::NoMatch
            );
        }
    }

    #[test]
    fn proc_fallback_skips_panes_resolved_by_existing_precedence() {
        let mut pane = tmux_pane_row(701)
            .session_name("metadata")
            .command("node")
            .title("Working")
            .current_path("/tmp/node-wrapper")
            .agent_provider("claude")
            .pane();
        let inspector = FakeProcessInspector::new([(701, vec!["codex".to_string()])]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Claude));
        assert_eq!(
            pane.classification.matched_by,
            Some(crate::app::ClassificationMatchKind::PaneMetadata)
        );
        assert_eq!(
            pane.diagnostics.proc_fallback.outcome,
            crate::app::ProcFallbackOutcome::Skipped
        );
        assert_eq!(
            pane.diagnostics.proc_fallback.reason,
            "provider already resolved by pane_metadata"
        );
        assert!(inspector.calls().is_empty());
    }

    #[test]
    fn proc_fallback_records_skip_reason_for_untargeted_unresolved_pane() {
        let mut pane = tmux_pane_row(702)
            .command("make")
            .title("(bront) ~/code/agent-wrapper")
            .current_path("/tmp/wrapper")
            .pane();
        let inspector = FakeProcessInspector::new([(702, vec!["codex".to_string()])]);

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_unresolved_ambiguous_pane(&pane, "(bront) ~/code/agent-wrapper");
        assert_eq!(
            pane.diagnostics.proc_fallback.outcome,
            crate::app::ProcFallbackOutcome::Skipped
        );
        assert_eq!(
            pane.diagnostics.proc_fallback.reason,
            "pane_current_command=make is not a targeted proc fallback launcher"
        );
        assert!(inspector.calls().is_empty());
    }

    #[test]
    fn proc_fallback_resolves_shell_pane_from_foreground_process() {
        let mut pane = proc_fallback_pane(740, "zsh", "agent wrapper");
        pane.tmux.pane_tty = "/dev/ttys740".to_string();
        let inspector = FakeProcessInspector::with_foreground(
            [(740, vec!["background-codex".to_string()])],
            [("/dev/ttys740".to_string(), vec!["copilot".to_string()])],
        );

        let snapshot = proc::LazyProcessSnapshot::new(&inspector);
        super::apply_proc_fallback_with_options(&mut pane, &snapshot, false);

        assert_eq!(pane.provider, Some(Provider::Copilot));
        assert_eq!(
            pane.classification.reasons,
            vec!["proc_foreground_command=copilot"]
        );
        assert_eq!(inspector.calls(), Vec::<u32>::new());
        assert_eq!(
            inspector.foreground_calls(),
            vec!["/dev/ttys740".to_string()]
        );
    }
}
