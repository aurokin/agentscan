#[test]
fn fixture_snapshot_parses_expected_provider_cases() {
    let rows = tmux::parse_pane_rows(TMUX_SNAPSHOT_FIXTURE).expect("fixture snapshot should parse");
    let panes: Vec<_> = rows.into_iter().map(classify::pane_from_row).collect();

    assert_fixture_codex_cases(&panes);
    assert_fixture_claude_cases(&panes);
    assert_fixture_gemini_cases(&panes);
    assert_fixture_opencode_case(&panes);
    assert_fixture_copilot_case(&panes);
    assert_fixture_cursor_cli_title_case(&panes);
    assert_fixture_cursor_cli_command_case(&panes);
    assert_fixture_pi_case(&panes);
}

#[test]
fn fixture_snapshot_preserves_wrapper_prefixes() {
    let rows = tmux::parse_pane_rows(TMUX_SNAPSHOT_FIXTURE).expect("fixture snapshot should parse");
    let panes: Vec<_> = rows.into_iter().map(classify::pane_from_row).collect();

    let wrapped_codex = pane_by_id(&panes, "%89");
    assert_eq!(wrapped_codex.provider, Some(Provider::Codex));
    assert_eq!(wrapped_codex.display.label, "(bront) parallel-n64");
    assert_eq!(
        wrapped_codex.display.activity_label.as_deref(),
        Some("(bront) parallel-n64")
    );
    assert_eq!(wrapped_codex.status.kind, StatusKind::Unknown);
    assert_eq!(wrapped_codex.tmux.session_id.as_deref(), Some("$8"));
    assert_eq!(wrapped_codex.tmux.window_id.as_deref(), Some("@8"));
}

#[test]
fn ambiguous_fixture_documents_current_unresolved_behavior() {
    let rows =
        tmux::parse_pane_rows(TMUX_AMBIGUOUS_FIXTURE).expect("ambiguous fixture should parse");
    let panes: Vec<_> = rows.into_iter().map(classify::pane_from_row).collect();

    assert_eq!(panes.len(), 5);
    assert_unresolved_ambiguous_pane(pane_by_id(&panes, "%600"), "(bront) ~/code/agent-wrapper");
    assert_unresolved_ambiguous_pane(pane_by_id(&panes, "%601"), "Working");
    assert_unresolved_ambiguous_pane(pane_by_id(&panes, "%602"), "agent bootstrap");
    assert_unresolved_ambiguous_pane(pane_by_id(&panes, "%603"), "pi - agentscan");
    assert_unresolved_ambiguous_pane(pane_by_id(&panes, "%604"), "review_auth_flow");
}

#[test]
fn proc_fallback_resolves_only_targeted_ambiguous_candidates() {
    let rows =
        tmux::parse_pane_rows(TMUX_AMBIGUOUS_FIXTURE).expect("ambiguous fixture should parse");
    let inspector = FakeProcessInspector::new([
        (602001, vec!["codex".to_string()]),
        (602002, vec!["cursor-agent".to_string()]),
        (602003, vec!["claude".to_string()]),
    ]);
    let panes = classify::panes_from_rows_with_proc_fallback(rows, &inspector);

    assert_unresolved_ambiguous_pane(pane_by_id(&panes, "%600"), "(bront) ~/code/agent-wrapper");

    let node_launcher = pane_by_id(&panes, "%601");
    assert_eq!(node_launcher.provider, Some(Provider::Codex));
    assert_eq!(node_launcher.status.kind, StatusKind::Busy);
    assert_eq!(
        node_launcher.classification.matched_by,
        Some(super::ClassificationMatchKind::ProcProcessTree)
    );
    assert_eq!(
        node_launcher.classification.reasons,
        vec!["proc_descendant_command=codex"]
    );
    assert_eq!(
        node_launcher.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::Resolved
    );
    assert_eq!(
        node_launcher.diagnostics.proc_fallback.commands,
        vec!["codex".to_string()]
    );

    let python_launcher = pane_by_id(&panes, "%602");
    assert_eq!(python_launcher.provider, Some(Provider::CursorCli));
    assert_eq!(python_launcher.status.kind, StatusKind::Unknown);
    assert_eq!(
        python_launcher.classification.matched_by,
        Some(super::ClassificationMatchKind::ProcProcessTree)
    );
    assert_eq!(
        python_launcher.classification.reasons,
        vec!["proc_descendant_command=cursor-agent"]
    );
    assert_eq!(
        python_launcher.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::Resolved
    );

    assert_unresolved_ambiguous_pane(pane_by_id(&panes, "%603"), "pi - agentscan");
    assert_unresolved_ambiguous_pane(pane_by_id(&panes, "%604"), "review_auth_flow");
    assert_eq!(inspector.calls(), vec![602001, 602002]);
}

#[test]
fn proc_fallback_options_can_skip_process_inspection() {
    let rows =
        tmux::parse_pane_rows(TMUX_AMBIGUOUS_FIXTURE).expect("ambiguous fixture should parse");
    let inspector = FakeProcessInspector::new([(602001, vec!["codex".to_string()])]);
    let panes = classify::panes_from_rows_with_proc_fallback_options(rows, &inspector, true);

    let node_launcher = pane_by_id(&panes, "%601");
    assert_eq!(node_launcher.provider, None);
    assert_eq!(
        node_launcher.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::Skipped
    );
    assert_eq!(
        node_launcher.diagnostics.proc_fallback.reason,
        "proc fallback disabled by configuration"
    );
    assert!(node_launcher.diagnostics.proc_fallback.commands.is_empty());
    assert!(inspector.calls().is_empty());
}

#[test]
fn proc_fallback_leaves_candidate_unknown_without_provider_evidence() {
    let mut pane = tmux_pane_row(700)
        .command("node")
        .title("Working")
        .current_path("/tmp/node-wrapper")
        .pane();
    let inspector =
        FakeProcessInspector::new([(700, vec!["node".to_string(), "helper".to_string()])]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_unresolved_ambiguous_pane(&pane, "Working");
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::NoMatch
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

    classify::apply_proc_fallback(&mut pane, &inspector);

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

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, Some(Provider::Claude));
    assert_eq!(pane.status.kind, StatusKind::Idle);
    assert_eq!(pane.status.source, super::StatusSource::TmuxTitle);
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

    classify::apply_proc_fallback(&mut pane, &inspector);

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

    classify::apply_proc_fallback(&mut pane, &inspector);

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

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_unresolved_ambiguous_pane(&pane, "Working");
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::NoMatch
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

    classify::apply_proc_fallback(&mut pane, &inspector);

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

    classify::apply_proc_fallback(&mut pane, &inspector);

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
                "/Users/auro/project/node_modules/opencode-darwin-arm64/bin/opencode".to_string(),
            ],
            env: Vec::new(),
        }],
    )]);

    classify::apply_proc_fallback(&mut pane, &inspector);

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

    classify::apply_proc_fallback(&mut pane, &inspector);

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

    classify::apply_proc_fallback(&mut pane, &inspector);

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

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, Some(Provider::Opencode));
    assert_eq!(
        pane.classification.reasons,
        vec!["proc_descendant_env=OPENCODE"]
    );
}

#[test]
fn proc_fallback_resolves_copilot_from_npm_loader_path() {
    let mut pane = proc_fallback_pane(760, "node", "agent wrapper");
    let loader_path =
        "/Users/auro/.local/share/mise/installs/npm-github-copilot/latest/lib/node_modules/@github/copilot/npm-loader.js";
    let inspector = FakeProcessInspector::with_processes([(
        760,
        vec![proc::ProcessEvidence {
            pid: 761,
            command: "node".to_string(),
            argv: vec!["node".to_string(), loader_path.to_string(), "--yolo".to_string()],
            env: Vec::new(),
        }],
    )]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, Some(Provider::Copilot));
    assert_eq!(
        pane.classification.reasons,
        vec![format!("proc_descendant_argv={loader_path}")]
    );
}

#[test]
fn proc_fallback_resolves_copilot_from_platform_package_path() {
    let mut pane = proc_fallback_pane(761, "node", "agent wrapper");
    let native_path =
        "/Users/auro/.local/share/mise/installs/npm-github-copilot/1.0.39/lib/node_modules/@github/copilot/node_modules/@github/copilot-darwin-arm64/copilot";
    let inspector = FakeProcessInspector::with_processes([(
        761,
        vec![proc::ProcessEvidence {
            pid: 762,
            command: "/Users/auro/.loc".to_string(),
            argv: vec![native_path.to_string(), "--yolo".to_string()],
            env: Vec::new(),
        }],
    )]);

    classify::apply_proc_fallback(&mut pane, &inspector);

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

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, None);
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::NoMatch
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

        classify::apply_proc_fallback(&mut pane, &inspector);

        assert_unresolved_ambiguous_pane(&pane, "Review plan");
        assert_eq!(
            pane.diagnostics.proc_fallback.outcome,
            super::ProcFallbackOutcome::NoMatch,
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

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_unresolved_ambiguous_pane(&pane, "Review plan");
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::NoMatch
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

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_unresolved_ambiguous_pane(&pane, "Review plan");
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::NoMatch
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

    classify::apply_proc_fallback(&mut pane, &inspector);

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

    classify::apply_proc_fallback(&mut pane, &inspector);

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

    classify::apply_proc_fallback(&mut pane, &inspector);

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

    classify::apply_proc_fallback(&mut pane, &inspector);

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

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_unresolved_ambiguous_pane(&pane, "Review plan");
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::NoMatch
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

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, None);
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::NoMatch
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

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_unresolved_ambiguous_pane(&pane, "Review plan");
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::NoMatch
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

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, Some(Provider::Hermes));
    assert_eq!(pane.status.kind, StatusKind::Unknown);
    assert_eq!(
        pane.classification.matched_by,
        Some(super::ClassificationMatchKind::ProcProcessTree)
    );
    assert_eq!(
        pane.classification.reasons,
        vec!["proc_descendant_argv=/Users/auro/.hermes/hermes-agent/venv/bin/python3"]
    );
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::Resolved
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

    classify::apply_proc_fallback(&mut pane, &inspector);

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

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, Some(Provider::Aider));
    assert_eq!(pane.status.kind, StatusKind::Unknown);
    assert_eq!(
        pane.classification.matched_by,
        Some(super::ClassificationMatchKind::ProcProcessTree)
    );
    assert_eq!(
        pane.classification.reasons,
        vec!["proc_descendant_argv=python -m aider"]
    );
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::Resolved
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

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_unresolved_ambiguous_pane(&pane, "aider");
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::NoMatch
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

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_unresolved_ambiguous_pane(&pane, "aider");
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::NoMatch
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

    classify::apply_proc_fallback(&mut pane, &inspector);

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

    classify::apply_proc_fallback(&mut pane, &inspector);

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

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_unresolved_ambiguous_pane(&pane, "aider");
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::NoMatch
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

    classify::apply_proc_fallback(&mut pane, &inspector);

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

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_unresolved_ambiguous_pane(&pane, "aider");
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::NoMatch
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

    classify::apply_proc_fallback(&mut pane, &inspector);

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

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_unresolved_ambiguous_pane(&pane, "aider");
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::NoMatch
    );
}

#[test]
fn hermes_title_text_alone_does_not_classify_provider() {
    assert!(classify::classify_provider(None, "zsh", "Hermes Agent").is_none());
    assert!(classify::classify_provider(None, "zsh", "⚕ gpt-5.5").is_none());
}

#[test]
fn proc_fallback_resolves_claude_from_title_glyph_and_descendant_command() {
    let mut pane = tmux_pane_row(711)
        .command("2.1.119")
        .title("✳ Analyze Linear Issue AUR-126 and plan implementation")
        .current_path("/tmp/claude-wrapper")
        .pane();
    let inspector = FakeProcessInspector::new([(711, vec!["claude".to_string()])]);

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, Some(Provider::Claude));
    assert_eq!(pane.status.kind, StatusKind::Idle);
    assert_eq!(pane.status.source, super::StatusSource::TmuxTitle);
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

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, Some(Provider::Claude));
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::Resolved
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

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_unresolved_ambiguous_pane(&pane, "Ready");
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::NoMatch
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

    classify::apply_proc_fallback(&mut pane, &inspector);

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

    classify::apply_proc_fallback(&mut pane, &inspector);

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

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_unresolved_ambiguous_pane(&pane, "worker-a");
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::NoMatch
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

        classify::apply_proc_fallback(&mut pane, &inspector);

        assert_unresolved_ambiguous_pane(&pane, "Working");
        assert_eq!(
            pane.diagnostics.proc_fallback.outcome,
            super::ProcFallbackOutcome::NoMatch
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

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_eq!(pane.provider, Some(Provider::Claude));
    assert_eq!(
        pane.classification.matched_by,
        Some(super::ClassificationMatchKind::PaneMetadata)
    );
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::Skipped
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

    classify::apply_proc_fallback(&mut pane, &inspector);

    assert_unresolved_ambiguous_pane(&pane, "(bront) ~/code/agent-wrapper");
    assert_eq!(
        pane.diagnostics.proc_fallback.outcome,
        super::ProcFallbackOutcome::Skipped
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

    classify::apply_proc_fallback(&mut pane, &inspector);

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

#[test]
fn inspect_text_reports_provider_status_and_fallback_provenance() {
    let rows =
        tmux::parse_pane_rows(TMUX_AMBIGUOUS_FIXTURE).expect("ambiguous fixture should parse");
    let inspector = FakeProcessInspector::new([(602001, vec!["codex".to_string()])]);
    let panes = classify::panes_from_rows_with_proc_fallback(rows, &inspector);
    let text = output::inspect_text(pane_by_id(&panes, "%601"));

    assert!(text.contains("provider: codex"));
    assert!(text.contains("provider_source: proc_process_tree"));
    assert!(text.contains("provider_confidence: high"));
    assert!(text.contains("status: busy"));
    assert!(text.contains("status_source: tmux_title"));
    assert!(text.contains("classification:\n  - proc_descendant_command=codex"));
    assert!(text.contains("proc_fallback:\n  outcome: resolved"));
    assert!(text.contains("  reason: resolved provider from process evidence"));
    assert!(text.contains("  commands:\n    - codex"));
}

#[test]
fn inspect_text_reports_unresolved_fallback_decision() {
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%703".to_string(),
        pane_pid: 703,
        pane_current_command: "node".to_string(),
        pane_title_raw: "Working".to_string(),
        pane_tty: "/dev/pts/703".to_string(),
        pane_current_path: "/tmp/node-wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
        pane_active: false,
        window_active: false,
    });
    let inspector = FakeProcessInspector::new([(703, vec!["node".to_string()])]);

    classify::apply_proc_fallback(&mut pane, &inspector);
    let text = output::inspect_text(&pane);

    assert!(text.contains("provider: unknown"));
    assert!(text.contains("provider_source: none"));
    assert!(text.contains("status_source: not_checked"));
    assert!(text.contains("classification: none"));
    assert!(text.contains("proc_fallback:\n  outcome: no_match"));
    assert!(text.contains("  reason: no known provider evidence found in descendants"));
    assert!(text.contains("  commands:\n    - node"));
}

#[test]
fn pane_metadata_overrides_display_provider_and_status_when_title_is_ambiguous() {
    let pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "wrapper".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%500".to_string(),
        pane_pid: 500,
        pane_current_command: "zsh".to_string(),
        pane_title_raw: "(bront) ~/code/wrapper".to_string(),
        pane_tty: "/dev/pts/500".to_string(),
        pane_current_path: "/home/auro/code/wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: Some("claude".to_string()),
        agent_label: Some("Wrapper Claude Task".to_string()),
        agent_cwd: Some("/tmp/wrapper".to_string()),
        agent_state: Some("idle".to_string()),
        agent_session_id: Some("sess-123".to_string()),
        pane_active: false,
        window_active: false,
    });

    assert_eq!(pane.provider, Some(Provider::Claude));
    assert_eq!(pane.display.label, "Wrapper Claude Task");
    assert_eq!(
        pane.display.activity_label.as_deref(),
        Some("Wrapper Claude Task")
    );
    assert_eq!(pane.status.kind, StatusKind::Idle);
    assert_eq!(pane.status.source, super::StatusSource::PaneMetadata);
    assert_eq!(
        pane.classification.matched_by,
        Some(super::ClassificationMatchKind::PaneMetadata)
    );
    assert_eq!(pane.agent_metadata.provider.as_deref(), Some("claude"));
    assert_eq!(pane.agent_metadata.session_id.as_deref(), Some("sess-123"));
}

#[test]
fn fixture_snapshot_with_metadata_parses_wrapper_fields() {
    let rows = tmux::parse_pane_rows(TMUX_METADATA_FIXTURE).expect("metadata fixture should parse");
    let panes: Vec<_> = rows.into_iter().map(classify::pane_from_row).collect();

    let pane = pane_by_id(&panes, "%400");
    assert_eq!(pane.provider, Some(Provider::Claude));
    assert_eq!(pane.display.label, "Wrapped Claude Task");
    assert_eq!(pane.status.kind, StatusKind::Busy);
    assert_eq!(pane.status.source, super::StatusSource::PaneMetadata);
    assert_eq!(
        pane.agent_metadata.cwd.as_deref(),
        Some("/tmp/wrapper-meta")
    );
    assert_eq!(pane.agent_metadata.session_id.as_deref(), Some("sess-123"));
}

#[test]
fn cache_fixture_deserializes_into_current_schema() {
    let snapshot: SnapshotEnvelope =
        serde_json::from_str(CACHE_SNAPSHOT_FIXTURE).expect("cache fixture should parse");

    assert_eq!(snapshot.schema_version, CACHE_SCHEMA_VERSION);
    assert_eq!(snapshot.source.kind, SourceKind::Daemon);
    assert_eq!(
        snapshot.source.daemon_generated_at.as_deref(),
        Some("2026-03-28T00:00:00Z")
    );
    assert_eq!(snapshot.panes.len(), 1);
    assert_eq!(snapshot.panes[0].pane_id, "%67");
    assert_eq!(snapshot.panes[0].status.kind, StatusKind::Idle);
    assert_eq!(snapshot.panes[0].tmux.session_id.as_deref(), Some("$1"));
    assert_eq!(snapshot.panes[0].tmux.window_id.as_deref(), Some("@1"));
    assert_eq!(
        snapshot.panes[0].diagnostics.cache_origin,
        "daemon_snapshot"
    );
}

#[test]
fn cache_summary_counts_fixture_contents() {
    let snapshot: SnapshotEnvelope =
        serde_json::from_str(CACHE_SNAPSHOT_FIXTURE).expect("cache fixture should parse");

    let summary = snapshot::summarize_snapshot(&snapshot).expect("cache fixture should summarize");
    assert_eq!(summary.pane_count, 1);
    assert_eq!(summary.agent_pane_count, 1);
    assert_eq!(summary.provider_counts, vec![(Provider::Codex, 1)]);
    assert_eq!(summary.status_counts, vec![(StatusKind::Idle, 1)]);
}

#[test]
fn snapshot_sort_orders_panes_by_location() {
    let mut snapshot = SnapshotEnvelope {
        schema_version: CACHE_SCHEMA_VERSION,
        generated_at: "2026-03-28T00:00:00Z".to_string(),
        source: super::SnapshotSource {
            kind: SourceKind::Snapshot,
            tmux_version: None,
            daemon_generated_at: None,
        },
        panes: vec![
            classify::pane_from_row(super::TmuxPaneRow {
                session_name: "zeta".to_string(),
                window_index: 2,
                pane_index: 1,
                pane_id: "%3".to_string(),
                pane_pid: 3,
                pane_current_command: "codex".to_string(),
                pane_title_raw: "Ready".to_string(),
                pane_tty: "/dev/pts/3".to_string(),
                pane_current_path: "/tmp/zeta".to_string(),
                window_name: "editor".to_string(),
                session_id: None,
                window_id: None,
                agent_provider: None,
                agent_label: None,
                agent_cwd: None,
                agent_state: None,
                agent_session_id: None,
                pane_active: false,
                window_active: false,
            }),
            classify::pane_from_row(super::TmuxPaneRow {
                session_name: "alpha".to_string(),
                window_index: 1,
                pane_index: 2,
                pane_id: "%2".to_string(),
                pane_pid: 2,
                pane_current_command: "claude".to_string(),
                pane_title_raw: "✳ Review".to_string(),
                pane_tty: "/dev/pts/2".to_string(),
                pane_current_path: "/tmp/alpha".to_string(),
                window_name: "ai".to_string(),
                session_id: None,
                window_id: None,
                agent_provider: None,
                agent_label: None,
                agent_cwd: None,
                agent_state: None,
                agent_session_id: None,
                pane_active: false,
                window_active: false,
            }),
            classify::pane_from_row(super::TmuxPaneRow {
                session_name: "alpha".to_string(),
                window_index: 1,
                pane_index: 1,
                pane_id: "%1".to_string(),
                pane_pid: 1,
                pane_current_command: "codex".to_string(),
                pane_title_raw: "Working".to_string(),
                pane_tty: "/dev/pts/1".to_string(),
                pane_current_path: "/tmp/alpha".to_string(),
                window_name: "editor".to_string(),
                session_id: None,
                window_id: None,
                agent_provider: None,
                agent_label: None,
                agent_cwd: None,
                agent_state: None,
                agent_session_id: None,
                pane_active: false,
                window_active: false,
            }),
        ],
    };

    snapshot::sort_snapshot_panes(&mut snapshot);

    let ordered_ids: Vec<_> = snapshot
        .panes
        .iter()
        .map(|pane| pane.pane_id.as_str())
        .collect();
    assert_eq!(ordered_ids, vec!["%1", "%2", "%3"]);
}

#[test]
fn validate_snapshot_rejects_unsupported_schema_version() {
    let mut snapshot: SnapshotEnvelope =
        serde_json::from_str(CACHE_SNAPSHOT_FIXTURE).expect("cache fixture should parse");
    snapshot.schema_version = CACHE_SCHEMA_VERSION - 1;

    let error =
        snapshot::validate_snapshot(&snapshot).expect_err("old schema version should fail");
    assert!(
        error
            .to_string()
            .contains("unsupported snapshot schema version"),
        "unexpected error: {error}"
    );
}

#[test]
fn validate_snapshot_rejects_future_schema_version() {
    let mut snapshot: SnapshotEnvelope =
        serde_json::from_str(CACHE_SNAPSHOT_FIXTURE).expect("cache fixture should parse");
    snapshot.schema_version = CACHE_SCHEMA_VERSION + 1;

    let error =
        snapshot::validate_snapshot(&snapshot).expect_err("future schema version should fail");
    assert!(
        error
            .to_string()
            .contains("unsupported snapshot schema version"),
        "unexpected error: {error}"
    );
}

#[test]
fn status_names_match_serialized_values() {
    assert_eq!(StatusKind::Busy.as_str(), "busy");
    assert_eq!(StatusKind::Idle.as_str(), "idle");
    assert_eq!(StatusKind::Unknown.as_str(), "unknown");
    assert_eq!(
        super::StatusSource::PaneOutput.as_str(),
        "pane_output"
    );
}

#[test]
fn known_status_glyph_stripping_preserves_normal_prefixes() {
    assert_eq!(
        classify::strip_known_status_glyph("(bront) parallel-n64: codex"),
        "(bront) parallel-n64: codex"
    );
    assert_eq!(
        classify::strip_known_status_glyph("✳ Review and summarize todo list"),
        "Review and summarize todo list"
    );
}

#[test]
fn title_normalization_strips_claude_and_opencode_prefixes() {
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Claude), "Claude Code | Query"),
        "Query"
    );
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Claude), "Claude | Ready"),
        "Ready"
    );
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Opencode), "OC | Query planner"),
        "Query planner"
    );
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Copilot), "Copilot | Review patch"),
        "Review patch"
    );
    assert_eq!(
        classify::normalize_title_for_display(
            Some(Provider::CursorCli),
            "Cursor CLI | Query planner"
        ),
        "Query planner"
    );
}

#[test]
fn title_normalization_strips_droid_and_pi_prefixes() {
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Droid), "⛬ Basic Math Question"),
        "Basic Math Question"
    );
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Pi), "π - refactor - agentscan"),
        "refactor - agentscan"
    );
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Pi), "pi - agentscan"),
        "pi - agentscan"
    );
}

#[test]
fn title_normalization_strips_codex_wrapper_suffixes() {
    assert_eq!(
        classify::normalize_title_for_display(
            Some(Provider::Codex),
            "(bront) parallel-n64: /home/auro/.zshrc.d/scripts/lgpt.sh"
        ),
        "(bront) parallel-n64"
    );
    assert_eq!(
        classify::normalize_title_for_display(
            Some(Provider::Codex),
            "(repo) task: codex --model gpt-5"
        ),
        "(repo) task"
    );
    assert_eq!(
        classify::normalize_title_for_display(
            Some(Provider::Codex),
            "(repo) task codex --model gpt-5"
        ),
        "(repo) task codex --model gpt-5"
    );
    assert_eq!(
        classify::normalize_title_for_display(
            Some(Provider::Codex),
            "pi - refactor - agentscan: codex"
        ),
        "pi - refactor - agentscan"
    );
    assert_eq!(
        classify::normalize_title_for_display(
            Some(Provider::Codex),
            "Copilot | Review patch: codex"
        ),
        "Copilot | Review patch"
    );
    assert_eq!(
        classify::normalize_title_for_display(
            Some(Provider::Codex),
            "Cursor CLI | Parser work: codex"
        ),
        "Cursor CLI | Parser work"
    );
}

#[test]
fn title_normalization_strips_codex_run_state_segments() {
    assert_eq!(
        classify::normalize_title_for_display(
            Some(Provider::Codex),
            "Working | Review code quality in repository"
        ),
        "Review code quality in repository"
    );
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Codex), "agentscan | Waiting"),
        "agentscan"
    );
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Codex), "Ready | Working"),
        "Ready"
    );
    assert_eq!(
        classify::normalize_title_for_display(
            Some(Provider::Codex),
            "review codex login | Working"
        ),
        "review codex login"
    );
}

#[test]
fn title_normalization_strips_codex_command_args_only_after_wrapper_prefix() {
    assert_eq!(
        classify::normalize_title_for_display(
            Some(Provider::Codex),
            "(repo) task: codex --model gpt-5 | Working"
        ),
        "(repo) task"
    );
    assert_eq!(
        classify::normalize_title_for_display(
            Some(Provider::Codex),
            "(repo) task codex --model gpt-5 | Working"
        ),
        "(repo) task codex --model gpt-5"
    );
    assert_eq!(
        classify::normalize_title_for_display(
            Some(Provider::Codex),
            "repo: /path/lgpt.sh | Working"
        ),
        "repo"
    );
    assert_eq!(
        classify::normalize_title_for_display(
            Some(Provider::Codex),
            "Working | (repo) task: codex --model gpt-5"
        ),
        "(repo) task"
    );
    assert_eq!(
        classify::normalize_title_for_display(
            Some(Provider::Codex),
            "Working | (repo) task codex --model gpt-5"
        ),
        "(repo) task codex --model gpt-5"
    );
}

#[test]
fn title_normalization_preserves_non_codex_status_like_titles() {
    assert_eq!(
        classify::normalize_title_for_display(None, "Working | deploy notes"),
        "Working | deploy notes"
    );
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Claude), "Working | deploy notes"),
        "Working | deploy notes"
    );
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Codex), "Copilot | Review patch"),
        "Copilot | Review patch"
    );
}

#[test]
fn title_normalization_strips_gemini_status_titles() {
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Gemini), "◇  Ready (workspace)"),
        "workspace"
    );
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Gemini), "✦  Working… (workspace)"),
        "workspace"
    );
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Gemini), "✦  Working… (repo (copy))"),
        "repo (copy)"
    );
    assert_eq!(
        classify::normalize_title_for_display(
            Some(Provider::Gemini),
            "✦  Processing request (workspace)"
        ),
        "Processing request"
    );
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Gemini), "Gemini CLI (workspace)"),
        "workspace"
    );
}

#[test]
fn display_metadata_extracts_activity_labels_from_titles() {
    let codex = classify::display_metadata(
        Some(Provider::Codex),
        None,
        None,
        "⠹ agentscan | Working",
        "codex",
        "editor",
    );
    assert_eq!(codex.label, "agentscan");
    assert_eq!(codex.activity_label.as_deref(), Some("agentscan"));

    let codex_status_first = classify::display_metadata(
        Some(Provider::Codex),
        None,
        None,
        "Working | Review code quality in repository",
        "codex",
        "editor",
    );
    assert_eq!(
        codex_status_first.label,
        "Review code quality in repository"
    );
    assert_eq!(
        codex_status_first.activity_label.as_deref(),
        Some("Review code quality in repository")
    );

    let codex_status_last_wins = classify::display_metadata(
        Some(Provider::Codex),
        None,
        None,
        "Ready | Working",
        "codex",
        "editor",
    );
    assert_eq!(codex_status_last_wins.label, "Ready");
    assert_eq!(
        codex_status_last_wins.activity_label.as_deref(),
        Some("Ready")
    );

    let codex_wrapped = classify::display_metadata(
        Some(Provider::Codex),
        None,
        None,
        "pi - refactor - agentscan: codex",
        "zsh",
        "editor",
    );
    assert_eq!(codex_wrapped.label, "pi - refactor - agentscan");
    assert_eq!(
        codex_wrapped.activity_label.as_deref(),
        Some("pi - refactor - agentscan")
    );

    let claude = classify::display_metadata(
        Some(Provider::Claude),
        None,
        None,
        "✳ Review and summarize todo list",
        "claude",
        "ai",
    );
    assert_eq!(claude.label, "Review and summarize todo list");
    assert_eq!(
        claude.activity_label.as_deref(),
        Some("Review and summarize todo list")
    );

    let generic = classify::display_metadata(
        Some(Provider::Codex),
        None,
        None,
        "Ready",
        "codex",
        "editor",
    );
    assert_eq!(generic.label, "Ready");
    assert_eq!(generic.activity_label, None);

    let wrapped_codex = classify::display_metadata(
        Some(Provider::Codex),
        None,
        None,
        "(bront) parallel-n64: /home/auro/.zshrc.d/scripts/lgpt.sh",
        "zsh",
        "editor",
    );
    assert_eq!(wrapped_codex.label, "(bront) parallel-n64");
    assert_eq!(
        wrapped_codex.activity_label.as_deref(),
        Some("(bront) parallel-n64")
    );

    let published = classify::display_metadata(
        Some(Provider::Claude),
        None,
        Some("Wrapper Claude Task"),
        "Claude Code | Working",
        "zsh",
        "ai",
    );
    assert_eq!(published.label, "Wrapper Claude Task");
    assert_eq!(
        published.activity_label.as_deref(),
        Some("Wrapper Claude Task")
    );

    let copilot = classify::display_metadata(
        Some(Provider::Copilot),
        None,
        None,
        "Copilot | Review patch",
        "copilot",
        "ai",
    );
    assert_eq!(copilot.label, "Review patch");
    assert_eq!(copilot.activity_label.as_deref(), Some("Review patch"));

    let cursor = classify::display_metadata(
        Some(Provider::CursorCli),
        Some(super::ClassificationMatchKind::PaneCurrentCommand),
        None,
        "Cursor CLI | Query planner",
        "cursor-agent",
        "ai",
    );
    assert_eq!(cursor.label, "Query planner");
    assert_eq!(cursor.activity_label.as_deref(), Some("Query planner"));

    let pi = classify::display_metadata(
        Some(Provider::Pi),
        None,
        None,
        "π - refactor - agentscan",
        "pi",
        "ai",
    );
    assert_eq!(pi.label, "refactor - agentscan");
    assert_eq!(pi.activity_label, None);

    let prefixed_codex = classify::display_metadata(
        Some(Provider::Codex),
        Some(super::ClassificationMatchKind::PaneTitle),
        None,
        "Copilot | Review patch: codex",
        "zsh",
        "ai",
    );
    assert_eq!(prefixed_codex.label, "Copilot | Review patch");
    assert_eq!(
        prefixed_codex.activity_label.as_deref(),
        Some("Copilot | Review patch")
    );
}

#[test]
fn droid_display_metadata_uses_title_after_provider_identity() {
    let droid = classify::display_metadata(
        Some(Provider::Droid),
        Some(super::ClassificationMatchKind::PaneCurrentCommand),
        None,
        "⛬ Basic Math Question",
        "droid",
        "ai",
    );

    assert_eq!(droid.label, "Basic Math Question");
    assert_eq!(
        droid.activity_label.as_deref(),
        Some("Basic Math Question")
    );
}

#[test]
fn grok_display_metadata_strips_title_suffix() {
    let grok = classify::display_metadata(
        Some(Provider::Grok),
        Some(super::ClassificationMatchKind::PaneTitle),
        None,
        "⠹ - Running: shell - agentscan - grok",
        "grok-0.1.212-ma",
        "ai",
    );
    assert_eq!(grok.label, "Running: shell - agentscan");
    assert_eq!(
        grok.activity_label.as_deref(),
        Some("Running: shell - agentscan")
    );

    let grok_home = classify::display_metadata(
        Some(Provider::Grok),
        Some(super::ClassificationMatchKind::PaneTitle),
        None,
        "grok",
        "grok",
        "ai",
    );
    assert_eq!(grok_home.label, "grok");
    assert_eq!(grok_home.activity_label, None);
}

#[test]
fn copilot_default_title_does_not_invent_activity_label() {
    let copilot_default = classify::display_metadata(
        Some(Provider::Copilot),
        Some(super::ClassificationMatchKind::PaneTitle),
        None,
        "GitHub Copilot",
        "node",
        "ai",
    );

    assert_eq!(copilot_default.label, "GitHub Copilot");
    assert_eq!(copilot_default.activity_label, None);
}

#[test]
fn hermes_display_metadata_keeps_title_without_activity_state() {
    let hermes = classify::display_metadata(
        Some(Provider::Hermes),
        Some(super::ClassificationMatchKind::ProcProcessTree),
        None,
        "agentscan: hermes",
        "python3.11",
        "python3.11",
    );

    assert_eq!(hermes.label, "agentscan: hermes");
    assert_eq!(hermes.activity_label, None);
}

#[test]
fn antigravity_display_metadata_keeps_generic_title_without_activity_state() {
    let antigravity = classify::display_metadata(
        Some(Provider::Antigravity),
        Some(super::ClassificationMatchKind::PaneCurrentCommand),
        None,
        "koopa.home.arpa",
        "agy",
        "idle",
    );

    assert_eq!(antigravity.label, "koopa.home.arpa");
    assert_eq!(antigravity.activity_label, None);
}

#[test]
fn hermes_display_metadata_preserves_published_label_activity_state() {
    let hermes = classify::display_metadata(
        Some(Provider::Hermes),
        Some(super::ClassificationMatchKind::PaneMetadata),
        Some("Review auth flow"),
        "agentscan: hermes",
        "python3.11",
        "python3.11",
    );

    assert_eq!(hermes.label, "Review auth flow");
    assert_eq!(hermes.activity_label.as_deref(), Some("Review auth flow"));
}

#[test]
fn aider_display_metadata_keeps_inherited_title_without_activity_state() {
    let aider = classify::display_metadata(
        Some(Provider::Aider),
        Some(super::ClassificationMatchKind::PaneCurrentCommand),
        None,
        "worktree-aider",
        "aider",
        "ai",
    );

    assert_eq!(aider.label, "worktree-aider");
    assert_eq!(aider.activity_label, None);
}

#[test]
fn aider_display_metadata_preserves_published_label_activity_state() {
    let aider = classify::display_metadata(
        Some(Provider::Aider),
        Some(super::ClassificationMatchKind::PaneMetadata),
        Some("Review CLI support"),
        "worktree-aider",
        "aider",
        "ai",
    );

    assert_eq!(aider.label, "Review CLI support");
    assert_eq!(aider.activity_label.as_deref(), Some("Review CLI support"));
}

#[test]
fn aider_pane_output_does_not_infer_status_from_generic_prompt() {
    let mut aider = pane_output_status_pane(820, Provider::Aider, "aider");

    classify::apply_pane_output_status_fallback(
        &mut aider,
        "Aider v0.86.0\n\
         > \n",
    );

    assert_eq!(aider.status.kind, StatusKind::Unknown);
    assert_eq!(aider.status.source, super::StatusSource::NotChecked);
}

#[test]
fn copilot_pane_output_marks_busy_only_after_provider_is_known() {
    let output = "❯ Review patch\n\n\
         ● Thinking (Esc to cancel · 616 B)\n\
         /tmp/probe [main]\n\
         ────────────────────\n\
         ❯\n\
         ────────────────────\n\
         / commands · ? help\n";
    assert_pane_output_status(
        745,
        Provider::Copilot,
        "GitHub Copilot",
        output,
        StatusKind::Busy,
        super::StatusSource::PaneOutput,
    );
    assert_unprovidered_pane_output_unchanged(746, "node", "custom title", output);
}

#[test]
fn copilot_pane_output_marks_current_working_footer_busy() {
    let output = "~/code/agentscan [⎇ aur-550]\n\
         ──────────────────────────────────────────────────────────────────────────\n\
         ❯\n\
         ──────────────────────────────────────────────────────────────────────────\n\
         ◉ Working esc cancel                                      GPT-5 mini\n";

    assert_pane_output_status(
        817,
        Provider::Copilot,
        "GitHub Copilot",
        output,
        StatusKind::Busy,
        super::StatusSource::PaneOutput,
    );
    assert_unprovidered_pane_output_unchanged(818, "node", "custom title", output);
}

#[test]
fn copilot_pane_output_marks_bordered_prompt_idle() {
    // Observed from Copilot v1.0.65: the ready prompt renders as a bordered empty input box
    // instead of the older standalone `❯` line.
    let mut copilot = pane_output_status_pane(822, Provider::Copilot, "GitHub Copilot");

    classify::apply_pane_output_status_fallback(
        &mut copilot,
        "~/code/agentscan [⎇ main*%]                                      Session: 0 AIC used\n\
         ╻▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄\n\
         ┃\n\
         ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
          / commands · ? help · tab next tab                                         GPT-5.5\n",
    );

    assert_eq!(copilot.status.kind, StatusKind::Idle);
    assert_eq!(copilot.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn copilot_pane_output_marks_bordered_prompt_with_draft_text_idle() {
    // When the user has typed but not submitted text, Copilot swaps the idle footer from
    // `/ commands · ? help` to attachment hints. The pane is still available for input.
    let mut copilot = pane_output_status_pane(825, Provider::Copilot, "GitHub Copilot");

    classify::apply_pane_output_status_fallback(
        &mut copilot,
        "~/code/agentscan [⎇ main*%]                                      Session: 0 AIC used\n\
         ╻▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄\n\
         ┃ 3\n\
         ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
          @ files · # issues                                                            GPT-5.5\n",
    );

    assert_eq!(copilot.status.kind, StatusKind::Idle);
    assert_eq!(copilot.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn copilot_pane_output_marks_bordered_working_footer_busy() {
    let mut copilot = pane_output_status_pane(823, Provider::Copilot, "GitHub Copilot");

    classify::apply_pane_output_status_fallback(
        &mut copilot,
        "~/code/agentscan [⎇ main*%]                                      Session: 0 AIC used\n\
         ╻▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄\n\
         ┃\n\
         ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
         ◉ Working esc cancel                                                           GPT-5.5\n",
    );

    assert_eq!(copilot.status.kind, StatusKind::Busy);
    assert_eq!(copilot.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn copilot_pane_output_ignores_stale_bordered_prompt() {
    let mut copilot = pane_output_status_pane(824, Provider::Copilot, "GitHub Copilot");

    classify::apply_pane_output_status_fallback(
        &mut copilot,
        "~/code/agentscan [⎇ main*%]                                      Session: 0 AIC used\n\
         ╻▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄\n\
         ┃\n\
         ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
         Reading files\n",
    );

    assert_eq!(copilot.status.kind, StatusKind::Unknown);
    assert_eq!(copilot.status.source, super::StatusSource::NotChecked);
}

#[test]
fn pane_output_status_fallback_requires_a_resolved_provider() {
    // Pane output is a provider-scoped status fallback: a pane with no resolved provider
    // must never be probed, even when its output looks agent-shaped.
    assert_unprovidered_pane_output_unchanged(
        747,
        "node",
        "custom title",
        "❯ Review patch\n\n\
         ● Thinking (Esc to cancel · 616 B)\n\
         /tmp/probe [main]\n\
         ────────────────────\n\
         ❯\n\
         ────────────────────\n\
         / commands · ? help\n",
    );
}

#[test]
fn droid_pane_output_marks_current_prompt_idle_only_after_provider_is_known() {
    let output = " Auto (High) - allow all commands            Droid Core (DeepSeek V4 Pro) (Max)\n\
         ╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ >                                                                            │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n\
         [⏱ 5s] ? for help                                                          IDE ◌\n";
    assert_pane_output_status(
        810,
        Provider::Droid,
        "⛬ New Session",
        output,
        StatusKind::Idle,
        super::StatusSource::PaneOutput,
    );
    assert_unprovidered_pane_output_unchanged(811, "zsh", "custom title", output);
}

#[test]
fn droid_pane_output_marks_current_tmux_footer_idle() {
    let output = "                       █████████    █████████     ████████    ███   █████████\n\
         \n\
                                  v0.156.2 (ctrl+j for changelog)\n\
         \n\
                    TIP: Use /context to see your context window usage breakdown\n\
         \n\
                         shift+tab to cycle modes · ctrl+N to cycle models\n\
                              ctrl+L for autonomy · tab for reasoning\n\
         \n\
                               Skills (21) ✓  MCPs (0) ✗  AGENTS.md ✓\n\
         \n\
         \n\
         Auto (High) · allow all commands                                       Droid Core (GLM-5.2) (High)\n\
         ╭──────────────────────────────────────────────────────────────────────────────────────────────────╮\n\
         │ > Try \"Review the changes in my current branch\"                                                  │\n\
         ╰──────────────────────────────────────────────────────────────────────────────────────────────────╯\n\
         ? for help                                                                                    TMUX ⧉\n";

    assert_pane_output_status(
        820,
        Provider::Droid,
        "⛬ New Session",
        output,
        StatusKind::Idle,
        super::StatusSource::PaneOutput,
    );
}

#[test]
fn droid_pane_output_marks_update_ready_tmux_footer_idle() {
    // Observed from Droid v0.156.2: the current prompt is followed by an update-ready footer
    // rather than the older `? for help` footer. The prompt box still anchors the live frame.
    let output = "Auto (High) · allow all commands                         Droid Core (GLM-5.2) (High)\n\
         ╭──────────────────────────────────────────────────────────────────────────────────────────────────╮\n\
         │ > Try \"How do I handle errors in async functions?\"                                               │\n\
         ╰──────────────────────────────────────────────────────────────────────────────────────────────────╯\n\
         ✓ v0.159.1 ready (restart to apply)                                                          TMUX ⧉\n";

    assert_pane_output_status(
        821,
        Provider::Droid,
        "⛬ New Session",
        output,
        StatusKind::Idle,
        super::StatusSource::PaneOutput,
    );
}

#[test]
fn droid_pane_output_marks_current_steer_prompt_busy() {
    // Mirrors a real busy droid frame (v0.134.0): the input box prompt switches to
    // "Enter to steer" during a turn, with a streaming line above it whose verb varies
    // ("Invoking tools…" here, not "Streaming…").
    let mut droid = pane_output_status_pane(812, Provider::Droid, "⛬ New Session");

    classify::apply_pane_output_status_fallback(
        &mut droid,
        "   Analyze this entire codebase and tell me about it\n\n\
         Plan · 0/7\n\
         ┃ ● Explore project structure and core files\n\n\
         ⠟ Invoking tools...  (Press ESC to stop)\n\n\
         Auto (High) - allow all commands            Droid Core (Kimi K2.6) (High)\n\
         ╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ > Enter to steer                                                             │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n\
         [⏱ 38s] ? for help                                                         IDE ◌\n",
    );

    assert_eq!(droid.status.kind, StatusKind::Busy);
    assert_eq!(droid.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn droid_pane_output_marks_streaming_busy_when_verb_is_not_streaming() {
    // The streaming fallback must recognize droid's varying verbs by the stop hint, not the
    // word "Streaming". Here the current frame has no steer/idle prompt in the box window, so
    // the streaming line is the deciding busy signal.
    let mut droid = pane_output_status_pane(814, Provider::Droid, "⛬ New Session");

    classify::apply_pane_output_status_fallback(
        &mut droid,
        "   Read the source and summarize\n\n\
         ⠹ Thinking...  (Press ESC to stop)\n\n\
         Auto (High) - allow all commands            Droid Core (Kimi K2.6) (High)\n\
         [⏱ 9s] ? for help                                                          IDE ◌\n",
    );

    assert_eq!(droid.status.kind, StatusKind::Busy);
    assert_eq!(droid.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn droid_pane_output_does_not_mark_busy_from_prose_stop_hint_without_spinner() {
    // The streaming fallback requires the live braille spinner glyph, not a bare "Press ESC to
    // stop" substring. Model output that mentions the phrase in prose (no leading spinner) sits
    // in the current frame with no steer/idle prompt, so the pane must not be marked busy.
    let mut droid = pane_output_status_pane(815, Provider::Droid, "⛬ New Session");

    classify::apply_pane_output_status_fallback(
        &mut droid,
        "   To cancel a running job, press ESC. The banner reads: Press ESC to stop\n\n\
         Auto (High) - allow all commands            Droid Core (Kimi K2.6) (High)\n\
         [⏱ 9s] ? for help                                                          IDE ◌\n",
    );

    assert_eq!(droid.status.kind, StatusKind::Unknown);
    assert_eq!(droid.status.source, super::StatusSource::NotChecked);
}

#[test]
fn droid_pane_output_ignores_stale_streaming_above_current_prompt() {
    let mut droid = pane_output_status_pane(813, Provider::Droid, "⛬ Basic Math Question");

    classify::apply_pane_output_status_fallback(
        &mut droid,
        " ⠄ Streaming...  (Press ESC to stop)\n\n\
         ⛬  2.\n\n\
         Auto (High) - allow all commands            Droid Core (DeepSeek V4 Pro) (Max)\n\
         ╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ >                                                                            │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n\
         [⏱ 5s] ? for help                                                          IDE ◌\n",
    );

    assert_eq!(droid.status.kind, StatusKind::Idle);
    assert_eq!(droid.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn claude_pane_output_marks_current_prompt_idle_only_after_provider_is_known() {
    let mut claude = pane_output_status_pane(804, Provider::Claude, "Claude Code");

    classify::apply_pane_output_status_fallback(
        &mut claude,
        "╭────────────────────────────────────────╮\n\
         ❯ Try \"fix the failing test\"\n\
         ╰────────────────────────────────────────╯\n\
         ? for shortcuts\n",
    );

    assert_eq!(claude.status.kind, StatusKind::Idle);
    assert_eq!(claude.status.source, super::StatusSource::PaneOutput);

    let mut unknown = proc_fallback_pane(805, "zsh", "custom title");
    classify::apply_pane_output_status_fallback(
        &mut unknown,
        "╭────────────────────────────────────────╮\n\
         ❯ Try \"fix the failing test\"\n\
         ╰────────────────────────────────────────╯\n\
         ? for shortcuts\n",
    );

    assert_eq!(unknown.status.kind, StatusKind::Unknown);
    assert_eq!(unknown.status.source, super::StatusSource::NotChecked);
}

#[test]
fn claude_pane_output_marks_current_interrupt_hint_busy() {
    let mut claude = pane_output_status_pane(806, Provider::Claude, "Claude Code");

    classify::apply_pane_output_status_fallback(
        &mut claude,
        "╭────────────────────────────────────────╮\n\
         ❯ \n\
         ╰────────────────────────────────────────╯\n\
         esc to interrupt\n",
    );

    assert_eq!(claude.status.kind, StatusKind::Busy);
    assert_eq!(claude.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn claude_pane_output_marks_current_permission_wait_busy() {
    let mut claude = pane_output_status_pane(807, Provider::Claude, "Claude Code");

    classify::apply_pane_output_status_fallback(
        &mut claude,
        "Waiting for permission…\n\
         \n\
         ╭────────────────────────────────────────╮\n\
         ❯ \n\
         ╰────────────────────────────────────────╯\n\
         ? for shortcuts\n",
    );

    assert_eq!(claude.status.kind, StatusKind::Busy);
    assert_eq!(claude.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn claude_pane_output_ignores_stale_prompt_without_current_footer() {
    let mut claude = pane_output_status_pane(808, Provider::Claude, "Claude Code");

    classify::apply_pane_output_status_fallback(
        &mut claude,
        "╭────────────────────────────────────────╮\n\
         ❯ Try \"fix the failing test\"\n\
         ╰────────────────────────────────────────╯\n\
         older transcript output\n\
         command result\n\
         done\n\
         shell prompt\n",
    );

    assert_eq!(claude.status.kind, StatusKind::Unknown);
    assert_eq!(claude.status.source, super::StatusSource::NotChecked);
}

#[test]
fn claude_pane_output_ignores_ascii_angle_output_near_footer() {
    let mut claude = pane_output_status_pane(809, Provider::Claude, "Claude Code");

    classify::apply_pane_output_status_fallback(
        &mut claude,
        "> quoted transcript output\n\
         ? for shortcuts\n",
    );

    assert_eq!(claude.status.kind, StatusKind::Unknown);
    assert_eq!(claude.status.source, super::StatusSource::NotChecked);
}

#[test]
fn codex_pane_output_marks_current_prompt_idle_only_after_provider_is_known() {
    let mut codex = pane_output_status_pane(793, Provider::Codex, "codex");

    classify::apply_pane_output_status_fallback(
        &mut codex,
        "› Ask Codex to do anything\n\
         \n\
           tab to queue message                                       100% context left\n",
    );

    assert_eq!(codex.status.kind, StatusKind::Idle);
    assert_eq!(codex.status.source, super::StatusSource::PaneOutput);

    let mut unknown = proc_fallback_pane(794, "zsh", "custom title");
    classify::apply_pane_output_status_fallback(
        &mut unknown,
        "› Ask Codex to do anything\n\
         \n\
           tab to queue message                                       100% context left\n",
    );

    assert_eq!(unknown.status.kind, StatusKind::Unknown);
    assert_eq!(unknown.status.source, super::StatusSource::NotChecked);
}

#[test]
fn codex_pane_output_marks_fast_mode_footer_idle() {
    let mut codex = pane_output_status_pane(795, Provider::Codex, "codex");

    classify::apply_pane_output_status_fallback(
        &mut codex,
        "› Ask Codex to do anything\n\
         \n\
           Fast on\n",
    );

    assert_eq!(codex.status.kind, StatusKind::Idle);
    assert_eq!(codex.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn codex_pane_output_marks_model_path_footer_idle() {
    let mut codex = pane_output_status_pane(800, Provider::Codex, "codex");

    classify::apply_pane_output_status_fallback(
        &mut codex,
        "› Ask Codex to do anything\n\
         \n\
           gpt-5.5 default · /tmp/project\n",
    );

    assert_eq!(codex.status.kind, StatusKind::Idle);
    assert_eq!(codex.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn codex_pane_output_marks_current_status_indicator_busy() {
    let mut codex = pane_output_status_pane(796, Provider::Codex, "codex");

    classify::apply_pane_output_status_fallback(
        &mut codex,
        "• Investigating rendering code (0s • esc to interrupt)\n\
         \n\
         \n\
         › Ask Codex to do anything\n\
         \n\
           tab to queue message                                       100% context left\n",
    );

    assert_eq!(codex.status.kind, StatusKind::Busy);
    assert_eq!(codex.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn codex_pane_output_marks_status_indicator_busy_with_model_path_footer() {
    let mut codex = pane_output_status_pane(801, Provider::Codex, "codex");

    classify::apply_pane_output_status_fallback(
        &mut codex,
        "• Working (0s • esc to interrupt)\n\
         \n\
         › Ask Codex to do anything\n\
         \n\
           gpt-5.5 default · /tmp/project\n",
    );

    assert_eq!(codex.status.kind, StatusKind::Busy);
    assert_eq!(codex.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn codex_pane_output_marks_status_indicator_with_details_busy() {
    let mut codex = pane_output_status_pane(803, Provider::Codex, "codex");

    classify::apply_pane_output_status_fallback(
        &mut codex,
        "• Working (0s • esc to interrupt)\n\
           └ cargo test -p codex-core -- --exact\n\
         \n\
         › Ask Codex to do anything\n\
         \n\
           gpt-5.5 default · /tmp/project\n",
    );

    assert_eq!(codex.status.kind, StatusKind::Busy);
    assert_eq!(codex.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn codex_pane_output_marks_current_approval_prompt_busy() {
    let mut codex = pane_output_status_pane(797, Provider::Codex, "codex");

    classify::apply_pane_output_status_fallback(
        &mut codex,
        "╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ Run command?                                                                  │\n\
         │                                                                              │\n\
         │ › 1. Yes, proceed (y)                                                        │\n\
         │   2. No, and tell Codex what to do differently                               │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n\
           Press enter to confirm or esc to cancel\n",
    );

    assert_eq!(codex.status.kind, StatusKind::Busy);
    assert_eq!(codex.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn codex_pane_output_uses_current_idle_footer_over_stale_busy_status() {
    let mut codex = pane_output_status_pane(798, Provider::Codex, "codex");

    classify::apply_pane_output_status_fallback(
        &mut codex,
        "• Working (0s • esc to interrupt)\n\
         Done.\n\
         \n\
         › Ask Codex to do anything\n\
         \n\
           gpt-5.4 high fast · ~/code/agentscan · Context 0% used\n",
    );

    assert_eq!(codex.status.kind, StatusKind::Idle);
    assert_eq!(codex.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn codex_pane_output_does_not_infer_idle_from_stale_model_path_footer() {
    let mut codex = pane_output_status_pane(802, Provider::Codex, "codex");

    classify::apply_pane_output_status_fallback(
        &mut codex,
        "› Ask Codex to do anything\n\
         \n\
           gpt-5.5 default · /tmp/project\n\
         \n\
         Planning edits\n\
         Reading files\n\
         Updating code\n\
         Running tests\n\
         Collecting output\n\
         Current line\n",
    );

    assert_eq!(codex.status.kind, StatusKind::Unknown);
    assert_eq!(codex.status.source, super::StatusSource::NotChecked);
}

#[test]
fn codex_pane_output_does_not_infer_idle_from_stale_prompt() {
    let mut codex = pane_output_status_pane(799, Provider::Codex, "codex");

    classify::apply_pane_output_status_fallback(
        &mut codex,
        "› Ask Codex to do anything\n\
         \n\
           tab to queue message                                       100% context left\n\
         \n\
         Planning edits\n\
         Reading files\n\
         Updating code\n\
         Running tests\n\
         Collecting output\n\
         Current line\n",
    );

    assert_eq!(codex.status.kind, StatusKind::Unknown);
    assert_eq!(codex.status.source, super::StatusSource::NotChecked);
}

#[test]
fn grok_pane_output_marks_current_prompt_box_idle_only_after_provider_is_known() {
    // Mirrors a real fresh idle grok pane (v0.2.3 capture): the rounded input box is the
    // current bottom UI with the version line below it, and the rest of the taller pane is
    // blank padding.
    let idle_screen = "   main ~/code/agentscan/\n\
         \n\
         Tip: Press Ctrl-W to start a parallel task in its own worktree.\n\
         \n\
         ╭────────────────────────────────────────────────╮\n\
         │ ❯                                                │\n\
         ╰──────────────── Grok Build · always-approve ─╯\n\
         \n\
         0.2.3 [stable] Beta\n\
         \n\
         \n\
         \n\
         \n";

    let mut grok = pane_output_status_pane(769, Provider::Grok, "grok");
    classify::apply_pane_output_status_fallback(&mut grok, idle_screen);

    assert_eq!(grok.status.kind, StatusKind::Idle);
    assert_eq!(grok.status.source, super::StatusSource::PaneOutput);

    let mut unknown = proc_fallback_pane(770, "zsh", "custom title");
    classify::apply_pane_output_status_fallback(&mut unknown, idle_screen);

    assert_eq!(unknown.status.kind, StatusKind::Unknown);
    assert_eq!(unknown.status.source, super::StatusSource::NotChecked);
}

#[test]
fn grok_pane_output_marks_channel_only_footer_idle() {
    // Observed from Grok Build Beta 0.2.60: the fresh prompt footer can be just `[stable]`
    // below the input box, with no dotted version token.
    let mut grok = pane_output_status_pane(771, Provider::Grok, "grok");

    classify::apply_pane_output_status_fallback(
        &mut grok,
        "╭────────────────────────────────────────────────────────────────────────────╮\n\
         │ ❯                                                                          │\n\
         ╰──────────────────────────────────── Composer 2.5 Fast · always-approve ─╯\n\
         \n\
         [stable]\n",
    );

    assert_eq!(grok.status.kind, StatusKind::Idle);
    assert_eq!(grok.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn grok_pane_output_does_not_treat_plain_channel_word_below_box_as_chrome() {
    let mut grok = pane_output_status_pane(783, Provider::Grok, "grok");

    classify::apply_pane_output_status_fallback(
        &mut grok,
        "╭────────────────────────────────────────────────────────────────────────────╮\n\
         │ ❯                                                                          │\n\
         ╰──────────────────────────────────── Composer 2.5 Fast · always-approve ─╯\n\
         \n\
         stable\n",
    );

    assert_eq!(grok.status.kind, StatusKind::Unknown);
    assert_eq!(grok.status.source, super::StatusSource::NotChecked);
}

#[test]
fn grok_pane_output_marks_used_session_keybind_footer_idle() {
    // Mirrors a real used grok session (v0.2.3): after a completed turn the input box is the
    // current bottom UI with the idle keybind footer (mode/shortcuts only) below it.
    let mut grok = pane_output_status_pane(778, Provider::Grok, "grok");

    classify::apply_pane_output_status_fallback(
        &mut grok,
        "     ❯ hi                                          2:23 PM\n\
         \n\
         Turn completed in 1.9s.\n\
         \n\
         ╭────────────────────────────────────────────────╮\n\
         │ ❯                                                │\n\
         ╰──────────────── Grok Build · always-approve ─╯\n\
         \n\
         Shift+Tab:mode  │  Ctrl+.:shortcuts\n",
    );

    assert_eq!(grok.status.kind, StatusKind::Idle);
    assert_eq!(grok.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn grok_pane_output_marks_active_turn_footer_busy() {
    // Mirrors a real busy grok pane (v0.2.3): the input box stays pinned at the bottom during
    // a turn, with the running spinner above it and the active-turn footer (adding
    // cancel/interject keybinds) below it.
    let mut grok = pane_output_status_pane(779, Provider::Grok, "grok");

    classify::apply_pane_output_status_fallback(
        &mut grok,
        "     ◆ Search \"disable_reconcile\" in src (28 matches)\n\
         \n\
         ⠹ Thinking… 0.4s                              42s ⇣80.3k [✗]\n\
         \n\
         ╭────────────────────────────────────────────────╮\n\
         │ ❯                                                │\n\
         ╰──────────────── Grok Build · always-approve ─╯\n\
         \n\
         Shift+Tab:mode  │  Ctrl+c:cancel  │  Ctrl+Enter:interject  │  Ctrl+.:shortcuts\n",
    );

    assert_eq!(grok.status.kind, StatusKind::Busy);
    assert_eq!(grok.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn grok_pane_output_marks_active_turn_busy_via_spinner_when_footer_reworded() {
    // Same live-turn layout as the active-turn footer case, but the footer hints are reworded
    // so `cancel`/`interject` are absent (mirrors grok relabeling its interrupt keybinds). The
    // run spinner sitting directly above the pinned box still proves the turn is in flight, so
    // the pane stays busy without depending on the footer wording.
    let mut grok = pane_output_status_pane(780, Provider::Grok, "grok");

    classify::apply_pane_output_status_fallback(
        &mut grok,
        "     ◆ Search \"disable_reconcile\" in src (28 matches)\n\
         \n\
         ⠹ Thinking… 0.4s                              42s ⇣80.3k [✗]\n\
         \n\
         ╭────────────────────────────────────────────────╮\n\
         │ ❯                                                │\n\
         ╰──────────────── Grok Build · always-approve ─╯\n\
         \n\
         Shift+Tab:mode  │  Ctrl+x:stop  │  Ctrl+.:shortcuts\n",
    );

    assert_eq!(grok.status.kind, StatusKind::Busy);
    assert_eq!(grok.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn grok_pane_output_marks_running_spinner_busy() {
    let mut grok = pane_output_status_pane(771, Provider::Grok, "grok");

    classify::apply_pane_output_status_fallback(
        &mut grok,
        "╭────────────────────────────────────────────────╮\n\
         │ ❯                                                │\n\
         ╰──────────────── Grok Build · always-approve ─╯\n\
         ⠹ Running: shell - agentscan 8s … ⇣123 [✗]\n",
    );

    assert_eq!(grok.status.kind, StatusKind::Busy);
    assert_eq!(grok.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn grok_pane_output_marks_running_body_marker_busy() {
    let mut grok = pane_output_status_pane(774, Provider::Grok, "grok");

    classify::apply_pane_output_status_fallback(
        &mut grok,
        "Turn completed in 2.8s.\n\
         ⠹ Editing files 5s … ⇣42 [✗]\n",
    );

    assert_eq!(grok.status.kind, StatusKind::Busy);
    assert_eq!(grok.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn grok_pane_output_ignores_stale_spinner_above_current_prompt_box() {
    // The 30-row capture still holds a prior turn's running spinner, but the current bottom
    // UI is the idle input box, so the pane is idle — the stale spinner must not force busy.
    let mut grok = pane_output_status_pane(775, Provider::Grok, "grok");

    classify::apply_pane_output_status_fallback(
        &mut grok,
        "⠹ Running: shell - agentscan 8s … ⇣123 [✗]\n\
         Turn completed in 4.2s.\n\
         ╭────────────────────────────────────────────────╮\n\
         │ ❯                                                │\n\
         ╰──────────────── Grok Build · always-approve ─╯\n",
    );

    assert_eq!(grok.status.kind, StatusKind::Idle);
    assert_eq!(grok.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn grok_pane_output_does_not_infer_idle_with_output_just_below_box_border() {
    // Even a single output row below the box border means the box is a stale frame in the
    // scrollback capture, not the current prompt — distance alone must not call it idle.
    let mut grok = pane_output_status_pane(776, Provider::Grok, "grok");

    classify::apply_pane_output_status_fallback(
        &mut grok,
        "╭────────────────────────────────────────────────╮\n\
         │ ❯                                                │\n\
         ╰──────────────── Grok Build · always-approve ─╯\n\
         Reading files\n",
    );

    assert_eq!(grok.status.kind, StatusKind::Unknown);
    assert_eq!(grok.status.source, super::StatusSource::NotChecked);
}

#[test]
fn grok_pane_output_does_not_infer_idle_without_current_prompt_box() {
    // A completed-turn line scrolled near the bottom with no current input box must not
    // be read as idle.
    let mut grok = pane_output_status_pane(772, Provider::Grok, "grok");

    classify::apply_pane_output_status_fallback(
        &mut grok,
        "Turn completed in 2.8s.\n\
         Reviewing the diff before the next step.\n",
    );

    assert_eq!(grok.status.kind, StatusKind::Unknown);
    assert_eq!(grok.status.source, super::StatusSource::NotChecked);
}

#[test]
fn grok_pane_output_does_not_infer_idle_from_scrolled_away_prompt_box() {
    // The input box exists in scrollback but a later turn pushed it far from the current
    // bottom, so it is no longer the active prompt.
    let mut grok = pane_output_status_pane(773, Provider::Grok, "grok");

    classify::apply_pane_output_status_fallback(
        &mut grok,
        "╭────────────────────────────────────────────────╮\n\
         │ ❯                                                │\n\
         ╰──────────────── Grok Build · always-approve ─╯\n\
         Reading files\n\
         Planning edits\n\
         Updating code\n\
         Running tests\n\
         Collecting output\n\
         Drafting summary\n\
         Current line\n",
    );

    assert_eq!(grok.status.kind, StatusKind::Unknown);
    assert_eq!(grok.status.source, super::StatusSource::NotChecked);
}

#[test]
fn grok_pane_output_does_not_treat_release_channel_prose_below_box_as_chrome() {
    // The version footer is shape-anchored (a couple of version/channel tokens). A prose
    // line that merely mentions a channel word like "Beta" sits below the box as real output,
    // so the box is a stale frame and the pane must not be inferred idle.
    let mut grok = pane_output_status_pane(777, Provider::Grok, "grok");

    classify::apply_pane_output_status_fallback(
        &mut grok,
        "╭────────────────────────────────────────────────╮\n\
         │ ❯                                                │\n\
         ╰──────────────── Grok Build · always-approve ─╯\n\
         Beta access for the new planner is rolling out now\n",
    );

    assert_eq!(grok.status.kind, StatusKind::Unknown);
    assert_eq!(grok.status.source, super::StatusSource::NotChecked);
}

#[test]
fn grok_pane_output_does_not_treat_ctrl_prose_below_box_as_keybind_footer() {
    // The keybind footer is matched by its `Ctrl+<key>:<action>` shape, not bare `Ctrl+`. Model
    // output that mentions `Ctrl+C` in prose sits below the box as real output, so the box is a
    // stale frame and the pane must not be inferred idle.
    let mut grok = pane_output_status_pane(780, Provider::Grok, "grok");

    classify::apply_pane_output_status_fallback(
        &mut grok,
        "╭────────────────────────────────────────────────╮\n\
         │ ❯                                                │\n\
         ╰──────────────── Grok Build · always-approve ─╯\n\
         You can press Ctrl+C to stop the dev server\n",
    );

    assert_eq!(grok.status.kind, StatusKind::Unknown);
    assert_eq!(grok.status.source, super::StatusSource::NotChecked);
}

#[test]
fn grok_pane_output_does_not_treat_shift_tab_prose_below_box_as_keybind_footer() {
    // The keybind footer is matched by its `Key:action` shape, not a bare `Shift+Tab` substring.
    // Prose mentioning Shift+Tab below a stale box is real output, so the pane stays unknown.
    let mut grok = pane_output_status_pane(782, Provider::Grok, "grok");

    classify::apply_pane_output_status_fallback(
        &mut grok,
        "╭────────────────────────────────────────────────╮\n\
         │ ❯                                                │\n\
         ╰──────────────── Grok Build · always-approve ─╯\n\
         Press Shift+Tab to cycle between the open editor tabs\n",
    );

    assert_eq!(grok.status.kind, StatusKind::Unknown);
    assert_eq!(grok.status.source, super::StatusSource::NotChecked);
}

#[test]
fn grok_pane_output_does_not_treat_version_led_prose_below_box_as_chrome() {
    // The version footer's trailing tokens must be release-channel labels, so version-led prose
    // like `0.2.3 docs` is real output below a stale box and the pane must not be inferred idle.
    let mut grok = pane_output_status_pane(781, Provider::Grok, "grok");

    classify::apply_pane_output_status_fallback(
        &mut grok,
        "╭────────────────────────────────────────────────╮\n\
         │ ❯                                                │\n\
         ╰──────────────── Grok Build · always-approve ─╯\n\
         0.2.3 docs\n",
    );

    assert_eq!(grok.status.kind, StatusKind::Unknown);
    assert_eq!(grok.status.source, super::StatusSource::NotChecked);
}

#[test]
fn antigravity_pane_output_marks_current_prompt_idle_only_after_provider_is_known() {
    // Mirrors a real idle antigravity pane: the bordered `>` prompt and `? for shortcuts`
    // footer sit at the top of a much taller pane padded with blank rows below.
    let idle_screen = "Antigravity CLI 1.0.1\n\
         auro@hsadler.com\n\
         Gemini 3.5 Flash (Medium)\n\
         ~/code/agentscan\n\
         ────────────────────────────────────────────────\n\
         >\n\
         ────────────────────────────────────────────────\n\
         ? for shortcuts                       Gemini 3.5 Flash (Medium)\n\
         \n\
         \n\
         \n\
         \n";

    let mut antigravity = pane_output_status_pane(795, Provider::Antigravity, "koopa.home.arpa");
    classify::apply_pane_output_status_fallback(&mut antigravity, idle_screen);

    assert_eq!(antigravity.status.kind, StatusKind::Idle);
    assert_eq!(antigravity.status.source, super::StatusSource::PaneOutput);

    let mut unknown = proc_fallback_pane(796, "zsh", "custom title");
    classify::apply_pane_output_status_fallback(&mut unknown, idle_screen);

    assert_eq!(unknown.status.kind, StatusKind::Unknown);
    assert_eq!(unknown.status.source, super::StatusSource::NotChecked);
}

#[test]
fn antigravity_pane_output_marks_active_turn_busy() {
    // Mirrors a real busy antigravity pane (CLI 1.0.2): a `… Generating…` spinner above the
    // `>` box, with the footer flipped from `? for shortcuts` to `esc to cancel` — the
    // current-frame busy anchor in the same position as the idle footer.
    let mut antigravity = pane_output_status_pane(802, Provider::Antigravity, "koopa.home.arpa");

    classify::apply_pane_output_status_fallback(
        &mut antigravity,
        "● Read(/Users/auro/code/agentscan/src/app/proc.rs) (ctrl+o to expand)\n\
         ⡿ Generating...\n\
         ────────────────────────────────────────────────\n\
         >\n\
         ────────────────────────────────────────────────\n\
         esc to cancel                         Gemini 3.5 Flash (Medium)\n\
         \n\
         \n",
    );

    assert_eq!(antigravity.status.kind, StatusKind::Busy);
    assert_eq!(antigravity.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn antigravity_pane_output_marks_busy_with_stale_idle_footer_in_scrollback() {
    // A prior turn's idle footer sits in scrollback above a fresh active turn. Because the
    // live bottom footer is `esc to cancel`, the pane is busy — the stale idle footer above
    // must not win.
    let mut antigravity = pane_output_status_pane(803, Provider::Antigravity, "koopa.home.arpa");

    classify::apply_pane_output_status_fallback(
        &mut antigravity,
        "? for shortcuts                       Gemini 3.5 Flash (Medium)\n\
         ⡿ Generating...\n\
         ────────────────────────────────────────────────\n\
         >\n\
         ────────────────────────────────────────────────\n\
         esc to cancel                         Gemini 3.5 Flash (Medium)\n",
    );

    assert_eq!(antigravity.status.kind, StatusKind::Busy);
    assert_eq!(antigravity.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn antigravity_pane_output_leaves_footerless_screen_unknown() {
    // Busy/idle are anchored on the footer below the `>` box. Free prose with neither footer
    // (nor the box) stays unknown rather than risk a guessed state.
    let mut antigravity = pane_output_status_pane(797, Provider::Antigravity, "koopa.home.arpa");

    classify::apply_pane_output_status_fallback(
        &mut antigravity,
        "Working on the request\n\
         The described approach will stop the leak.\n\
         Streaming the diff now\n",
    );

    assert_eq!(antigravity.status.kind, StatusKind::Unknown);
    assert_eq!(antigravity.status.source, super::StatusSource::NotChecked);
}

#[test]
fn antigravity_pane_output_does_not_infer_idle_from_scrolled_away_footer() {
    let mut antigravity = pane_output_status_pane(798, Provider::Antigravity, "koopa.home.arpa");

    classify::apply_pane_output_status_fallback(
        &mut antigravity,
        "? for shortcuts                       Gemini 3.5 Flash (Medium)\n\
         Reading files\n\
         Planning edits\n\
         Running tests\n\
         Current line\n",
    );

    assert_eq!(antigravity.status.kind, StatusKind::Unknown);
    assert_eq!(antigravity.status.source, super::StatusSource::NotChecked);
}

#[test]
fn antigravity_pane_output_idle_survives_stale_cancel_hint_in_scrollback() {
    // A prior turn's cancel hint is still in the scrollback capture above a fresh idle
    // footer. Because the live footer is the current bottom, the pane is idle — the stale
    // cancel hint must not force busy.
    let mut antigravity = pane_output_status_pane(801, Provider::Antigravity, "koopa.home.arpa");

    classify::apply_pane_output_status_fallback(
        &mut antigravity,
        "esc to cancel                         Gemini 3.5 Flash (Medium)\n\
         Done with the previous request.\n\
         ────────────────────────────────────────────────\n\
         >\n\
         ────────────────────────────────────────────────\n\
         ? for shortcuts                       Gemini 3.5 Flash (Medium)\n",
    );

    assert_eq!(antigravity.status.kind, StatusKind::Idle);
    assert_eq!(antigravity.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn antigravity_pane_output_does_not_infer_idle_with_output_just_below_footer() {
    // A single output row below the `? for shortcuts` footer means it is a stale frame; only
    // blank rows may follow the current idle footer.
    let mut antigravity = pane_output_status_pane(800, Provider::Antigravity, "koopa.home.arpa");

    classify::apply_pane_output_status_fallback(
        &mut antigravity,
        "────────────────────────────────────────────────\n\
         >\n\
         ────────────────────────────────────────────────\n\
         ? for shortcuts                       Gemini 3.5 Flash (Medium)\n\
         Reading files\n",
    );

    assert_eq!(antigravity.status.kind, StatusKind::Unknown);
    assert_eq!(antigravity.status.source, super::StatusSource::NotChecked);
}

#[test]
fn copilot_pane_output_ignores_stale_thinking_lines() {
    let mut copilot = pane_output_status_pane(748, Provider::Copilot, "GitHub Copilot");

    classify::apply_pane_output_status_fallback(
        &mut copilot,
        "● Thinking (Esc to cancel · 616 B)\n\
         ● Done! Created result.txt.\n\
         \n\
         /tmp/probe [main]\n\
         ────────────────────\n\
         ❯\n\
         ────────────────────\n\
         / commands · ? help\n",
    );

    assert_eq!(copilot.status.kind, StatusKind::Idle);
    assert_eq!(copilot.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn copilot_pane_output_marks_current_trust_prompt_busy() {
    let mut copilot = pane_output_status_pane(749, Provider::Copilot, "GitHub Copilot");

    classify::apply_pane_output_status_fallback(
        &mut copilot,
        "╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ Confirm folder trust                                                         │\n\
         │ Do you trust the files in this folder?                                       │\n\
         │ ❯ 1. Yes                                                                     │\n\
         │   2. Yes, and remember this folder for future sessions                       │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n",
    );

    assert_eq!(copilot.status.kind, StatusKind::Busy);
    assert_eq!(copilot.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn copilot_pane_output_does_not_infer_idle_from_prompt() {
    let mut copilot = pane_output_status_pane(747, Provider::Copilot, "GitHub Copilot");

    classify::apply_pane_output_status_fallback(
        &mut copilot,
        "/tmp/probe [main]\n────────────────────\n❯\n",
    );

    assert_eq!(copilot.status.kind, StatusKind::Unknown);
    assert_eq!(copilot.status.source, super::StatusSource::NotChecked);
}

#[test]
fn copilot_pane_output_marks_current_prompt_idle() {
    let mut copilot = pane_output_status_pane(757, Provider::Copilot, "GitHub Copilot");

    classify::apply_pane_output_status_fallback(
        &mut copilot,
        "╭──────────────────────────────────────────────────────────────────────────╮\n\
         │  GitHub Copilot v1.0.39                                           │\n\
         ╰──────────────────────────────────────────────────────────────────────────╯\n\
         \n\
         ● Environment loaded: 1 custom instruction, 22 skills\n\
         \n\
         ~/code/agentscan [⎇ master*]\n\
         ──────────────────────────────────────────────────────────────────────────\n\
         ❯\n\
         ──────────────────────────────────────────────────────────────────────────\n\
          / commands · ? help                                      Claude Haiku 4.5\n",
    );

    assert_eq!(copilot.status.kind, StatusKind::Idle);
    assert_eq!(copilot.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn copilot_pane_output_marks_absolute_path_prompt_idle() {
    let mut copilot = pane_output_status_pane(759, Provider::Copilot, "GitHub Copilot");

    classify::apply_pane_output_status_fallback(
        &mut copilot,
        "╭──────────────────────────────────────────────────────────────────────────╮\n\
         │  GitHub Copilot v1.0.39                                           │\n\
         ╰──────────────────────────────────────────────────────────────────────────╯\n\
         \n\
         ● Environment loaded: 22 skills, 1 MCP server, 2 agents\n\
         \n\
         /private/tmp/agentscan-copilot-idle-smoke\n\
         ──────────────────────────────────────────────────────────────────────────\n\
         ❯\n\
         ──────────────────────────────────────────────────────────────────────────\n\
          / commands · ? help                                      Claude Haiku 4.5\n",
    );

    assert_eq!(copilot.status.kind, StatusKind::Idle);
    assert_eq!(copilot.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn copilot_pane_output_uses_current_prompt_over_stale_thinking() {
    let mut copilot = pane_output_status_pane(758, Provider::Copilot, "GitHub Copilot");

    classify::apply_pane_output_status_fallback(
        &mut copilot,
        "● Thinking (Esc to cancel · 616 B)\n\
         ● Done! Created result.txt.\n\
         \n\
         ~/code/agentscan [⎇ master*]\n\
         ──────────────────────────────────────────────────────────────────────────\n\
         ❯\n\
         ──────────────────────────────────────────────────────────────────────────\n\
          / commands · ? help                                      Claude Haiku 4.5\n",
    );

    assert_eq!(copilot.status.kind, StatusKind::Idle);
    assert_eq!(copilot.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn copilot_pane_output_uses_current_prompt_over_stale_working_footer() {
    let mut copilot = pane_output_status_pane(819, Provider::Copilot, "GitHub Copilot");

    classify::apply_pane_output_status_fallback(
        &mut copilot,
        "~/code/agentscan [⎇ aur-550]\n\
         ──────────────────────────────────────────────────────────────────────────\n\
         ❯\n\
         ──────────────────────────────────────────────────────────────────────────\n\
         ◉ Working esc cancel                                      GPT-5 mini\n\
         ● Finished running command.\n\
         \n\
         ~/code/agentscan [⎇ aur-550]\n\
         ──────────────────────────────────────────────────────────────────────────\n\
         ❯\n\
         ──────────────────────────────────────────────────────────────────────────\n\
         / commands · ? help · tab next tab                         GPT-5 mini\n",
    );

    assert_eq!(copilot.status.kind, StatusKind::Idle);
    assert_eq!(copilot.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn cursor_cli_pane_output_marks_current_running_prompt_busy() {
    let mut cursor = pane_output_status_pane(750, Provider::CursorCli, "Command Runner");

    classify::apply_pane_output_status_fallback(
        &mut cursor,
        "  $ sleep 15; printf cursor-smoke-ok > result.txt 11s in\n\
           /tmp/agentscan-cursor-smoke\n\
         \n\
         ⠳⠀ Running  187 tokens\n\
         ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄\n\
          → Add a follow-up                                             ctrl+c to stop\n\
         ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
          Auto · 5%                                                           Auto-run\n\
          /private/tmp/agentscan-cursor-smoke\n",
    );

    assert_eq!(cursor.status.kind, StatusKind::Busy);
    assert_eq!(cursor.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn cursor_cli_pane_output_marks_prompt_footer_running_busy() {
    let mut cursor = pane_output_status_pane(753, Provider::CursorCli, "Command Runner");

    classify::apply_pane_output_status_fallback(
        &mut cursor,
        "  $ sleep 12; printf cursor-smoke-ok-3 > result3.txt 12s\n\
         \n\
         ⠜⠃ Running  238 tokens\n\
         ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄\n\
          → Please run exactly this shell command: sleep 12; printf cursor-smoke-ok-3\n\
            > result3.txt. Do not edit anything else.\n\
         ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
          Auto · 5.1%                                                         Auto-run\n\
          /private/tmp/agentscan-cursor-smoke\n",
    );

    assert_eq!(cursor.status.kind, StatusKind::Busy);
    assert_eq!(cursor.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn cursor_cli_pane_output_ignores_stale_running_lines() {
    let mut cursor = pane_output_status_pane(751, Provider::CursorCli, "Command Runner");

    classify::apply_pane_output_status_fallback(
        &mut cursor,
        " ⠳⠀ Running  187 tokens\n\
         \n\
          Completed. I ran exactly:\n\
         \n\
          sleep 15; printf cursor-smoke-ok > result.txt\n\
         \n\
         ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄\n\
          → Add a follow-up\n\
         ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
          Auto · 5.1%                                                         Auto-run\n\
          /private/tmp/agentscan-cursor-smoke\n",
    );

    assert_eq!(cursor.status.kind, StatusKind::Idle);
    assert_eq!(cursor.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn cursor_cli_pane_output_uses_latest_footer() {
    let mut cursor = pane_output_status_pane(752, Provider::CursorCli, "Command Runner");

    classify::apply_pane_output_status_fallback(
        &mut cursor,
        " ⠳⠀ Running  187 tokens\n\
         ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄\n\
          → Add a follow-up                                             ctrl+c to stop\n\
         ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
         \n\
          Completed. I ran exactly:\n\
         \n\
          sleep 15; printf cursor-smoke-ok > result.txt\n\
         \n\
         ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄\n\
          → Add a follow-up\n\
         ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
          Auto · 5.1%                                                         Auto-run\n\
          /private/tmp/agentscan-cursor-smoke\n",
    );

    assert_eq!(cursor.status.kind, StatusKind::Idle);
    assert_eq!(cursor.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn cursor_cli_pane_output_ignores_stale_running_footer_block() {
    let mut cursor = pane_output_status_pane(754, Provider::CursorCli, "Command Runner");

    classify::apply_pane_output_status_fallback(
        &mut cursor,
        " ⠜⠃ Running  238 tokens\n\
         ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄\n\
          → Please run exactly this shell command: sleep 12; printf cursor-smoke-ok-3\n\
            > result3.txt. Do not edit anything else.\n\
         ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
         \n\
          Completed. I ran exactly:\n\
         \n\
          sleep 12; printf cursor-smoke-ok-3 > result3.txt\n\
         \n\
         ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄\n\
          → Add a follow-up\n\
         ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
          Auto · 5.1%                                                         Auto-run\n\
          /private/tmp/agentscan-cursor-smoke\n",
    );

    assert_eq!(cursor.status.kind, StatusKind::Idle);
    assert_eq!(cursor.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn cursor_cli_pane_output_ignores_response_text_before_idle_footer() {
    let mut cursor = pane_output_status_pane(755, Provider::CursorCli, "Command Runner");

    classify::apply_pane_output_status_fallback(
        &mut cursor,
        "  Running `cargo test` now passes.\n\
         \n\
         ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄\n\
          → Add a follow-up\n\
         ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
          Auto · 5.1%                                                         Auto-run\n\
          /private/tmp/agentscan-cursor-smoke\n",
    );

    assert_eq!(cursor.status.kind, StatusKind::Idle);
    assert_eq!(cursor.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn cursor_cli_pane_output_marks_initial_prompt_idle() {
    let mut cursor = pane_output_status_pane(756, Provider::CursorCli, "Cursor Agent");

    classify::apply_pane_output_status_fallback(
        &mut cursor,
        "  Cursor Agent\n\
          v2026.04.28-e984b46\n\
         \n\
         ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄\n\
          → Plan, search, build anything\n\
         ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
          Auto\n\
          ~/code/agentscan · master\n",
    );

    assert_eq!(cursor.status.kind, StatusKind::Idle);
    assert_eq!(cursor.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn cursor_cli_pane_output_marks_borderless_initial_prompt_idle() {
    // Observed from Cursor Agent v2026.06.04: the initial prompt no longer has the
    // `▄▄▄▄`/`▀▀▀▀` footer borders, but still renders the `→ Plan...` prompt above
    // Cursor's composer/path footer.
    let mut cursor = pane_output_status_pane(757, Provider::CursorCli, "Cursor Agent");

    classify::apply_pane_output_status_fallback(
        &mut cursor,
        "  Cursor Agent\n\
          v2026.06.04-5fd875e\n\
          Use /run-everything to skip all approvals.\n\
         \n\
         \n\
           → Plan, search, build anything\n\
         \n\
         \n\
           Composer 2.5                                                   Auto-run -- INSERT --\n\
           /private/tmp/agentscan-cursor-smoke · main\n",
    );

    assert_eq!(cursor.status.kind, StatusKind::Idle);
    assert_eq!(cursor.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn cursor_cli_pane_output_marks_run_everything_footer_idle() {
    let mut cursor = pane_output_status_pane(758, Provider::CursorCli, "Cursor Agent");

    classify::apply_pane_output_status_fallback(
        &mut cursor,
        "  Cursor Agent\n\
         \n\
           → Plan, search, build anything\n\
         \n\
         \n\
           Composer 2.5                                             Run Everything -- INSERT --\n\
           /private/tmp/agentscan-cursor-smoke · main\n",
    );

    assert_eq!(cursor.status.kind, StatusKind::Idle);
    assert_eq!(cursor.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn cursor_cli_pane_output_marks_borderless_stop_hint_busy() {
    let mut cursor = pane_output_status_pane(759, Provider::CursorCli, "Command Runner");

    classify::apply_pane_output_status_fallback(
        &mut cursor,
        "$ sleep 90 40s\n\
           ctrl+b twice to send to background\n\
         \n\
         ⠘⠤ Running  46 tokens\n\
            Tip: Use subagents to parallelize work and preserve context.\n\
         \n\
           → Add a follow-up                                                     ctrl+c to stop\n\
         \n\
         \n\
           1 task\n\
           Composer 2.5                                             Run Everything -- INSERT --\n\
           /private/tmp/agentscan-cursor-smoke · main\n",
    );

    assert_eq!(cursor.status.kind, StatusKind::Busy);
    assert_eq!(cursor.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn cursor_cli_pane_output_ignores_stale_borderless_stop_hint() {
    let mut cursor = pane_output_status_pane(760, Provider::CursorCli, "Command Runner");

    classify::apply_pane_output_status_fallback(
        &mut cursor,
        "$ sleep 90 40s\n\
           ctrl+b twice to send to background\n\
         \n\
         ⠘⠤ Running  46 tokens\n\
            Tip: Use subagents to parallelize work and preserve context.\n\
         \n\
           → Add a follow-up                                                     ctrl+c to stop\n\
         \n\
         Completed. I ran the requested command.\n\
         There is no current Cursor composer footer below this output.\n",
    );

    assert_eq!(cursor.status.kind, StatusKind::Unknown);
    assert_eq!(cursor.status.source, super::StatusSource::NotChecked);
}

#[test]
fn hermes_pane_output_marks_busy_only_after_provider_is_known() {
    let mut hermes = pane_output_status_pane(765, Provider::Hermes, "agentscan: hermes");

    classify::apply_pane_output_status_fallback(
        &mut hermes,
        "╭─ task ─╮\n\
         │ working │\n\
         ╰─────────╯\n\
         ⚕ gpt-5.5 │ 65.4K/272K │ [██░░░░░░░░] 24% │ 2m │ ⏱ 1m 19s\n\
         ⚕ ❯ msg=interrupt · /queue · /bg · /steer · Ctrl+C cancel\n",
    );

    assert_eq!(hermes.status.kind, StatusKind::Busy);
    assert_eq!(hermes.status.source, super::StatusSource::PaneOutput);

    let mut unknown = proc_fallback_pane(766, "python3.11", "agentscan: hermes");
    classify::apply_pane_output_status_fallback(
        &mut unknown,
        "⚕ gpt-5.5 │ 65.4K/272K │ [██░░░░░░░░] 24% │ 2m │ ⏱ 1m 19s\n\
         ⚕ ❯ msg=interrupt · /queue · /bg · /steer · Ctrl+C cancel\n",
    );

    assert_eq!(unknown.status.kind, StatusKind::Unknown);
    assert_eq!(unknown.status.source, super::StatusSource::NotChecked);
}

#[test]
fn hermes_pane_output_marks_current_prompt_idle() {
    let mut hermes = pane_output_status_pane(767, Provider::Hermes, "agentscan: hermes");

    classify::apply_pane_output_status_fallback(
        &mut hermes,
        "┌─────────────────────────────────────────────────────────────────────────────┐\n\
         │ Hermes Agent                                                                │\n\
         └─────────────────────────────────────────────────────────────────────────────┘\n\
         ⚕ gpt-5.5 │ ctx -- │ [░░░░░░░░░░] -- │ 2s │ ⏲ 0s\n\
         ❯\n",
    );

    assert_eq!(hermes.status.kind, StatusKind::Idle);
    assert_eq!(hermes.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn hermes_pane_output_marks_idle_with_unsubmitted_draft_prompt() {
    // The user has typed a message but not submitted it: the agent is not running a turn, so the
    // honest label is idle even though the prompt is no longer a bare `❯`. The busy prompt is
    // `⚕ ❯ …` (leading `⚕`), so a `❯ <draft>` line cannot be mistaken for it.
    let mut hermes = pane_output_status_pane(769, Provider::Hermes, "agentscan: hermes");

    classify::apply_pane_output_status_fallback(
        &mut hermes,
        "⚕ gpt-5.5 │ ctx -- │ [░░░░░░░░░░] -- │ 5s │ ⏲ 0s │ ⚠ YOLO\n\
         ─────────────────────────────────────────────────────────────\n\
         ❯ Analyze the entire repo, tell me what you like, tell me what you don't\n\
         ─────────────────────────────────────────────────────────────\n",
    );

    assert_eq!(hermes.status.kind, StatusKind::Idle);
    assert_eq!(hermes.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn hermes_pane_output_marks_initializing_turn_busy() {
    let mut hermes = pane_output_status_pane(771, Provider::Hermes, "agentscan: hermes");

    classify::apply_pane_output_status_fallback(
        &mut hermes,
        "────────────────────────────────────────\n\
         ● Print exactly the marker formed by joining these parts with underscores\n\
         Initializing agent...\n\
         ────────────────────────────────────────\n",
    );

    assert_eq!(hermes.status.kind, StatusKind::Busy);
    assert_eq!(hermes.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn hermes_pane_output_uses_idle_prompt_below_stale_initializing_turn() {
    let mut hermes = pane_output_status_pane(772, Provider::Hermes, "agentscan: hermes");

    classify::apply_pane_output_status_fallback(
        &mut hermes,
        "────────────────────────────────────────\n\
         ● Print exactly the marker formed by joining these parts with underscores\n\
         Initializing agent...\n\
         ────────────────────────────────────────\n\
         ╭─ ⚕ Hermes ────────────────────────────╮\n\
             AGENTSCAN_E2E_DONE_hermes_123\n\
         ╰───────────────────────────────────────╯\n\
         ⚕ gpt-5.5 │ 16.4K/272K │ [█░░░░░░░░░] 6% │ 8s │ ⏲ 4s │ ⚠ YOLO\n\
         ────────────────────────────────────────\n\
         ❯\n\
         ────────────────────────────────────────\n",
    );

    assert_eq!(hermes.status.kind, StatusKind::Idle);
    assert_eq!(hermes.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn hermes_pane_output_does_not_infer_idle_from_stale_draft_prompt_in_scrollback() {
    // A `❯ …` draft prompt (with its status bar) sits in the scrollback capture, but the turn
    // ran and agent output scrolled below it with no current prompt/busy footer at the bottom.
    // The broadened `❯ <draft>` idle match must not resurrect that stale line — the prompt is
    // far from the current footer, so the pane stays unknown.
    let mut hermes = pane_output_status_pane(770, Provider::Hermes, "agentscan: hermes");

    classify::apply_pane_output_status_fallback(
        &mut hermes,
        "⚕ gpt-5.5 │ ctx -- │ [░░░░░░░░░░] -- │ 5s │ ⏲ 0s │ ⚠ YOLO\n\
         ─────────────────────────────────────────────────────────────\n\
         ❯ Analyze the entire repo, tell me what you like\n\
         ─────────────────────────────────────────────────────────────\n\
         ⚕ Reading src/app/classify/pane_output.rs\n\
         ⚕ Reading src/app/classify/provider_match.rs\n\
         ⚕ Grepping for hermes_idle_prompt_line\n\
         Found 3 matches across the classify module.\n\
         Next I will outline the strengths and weaknesses I see.\n\
         Starting with the daemon event loop and classification ladder.\n",
    );

    assert_eq!(hermes.status.kind, StatusKind::Unknown);
    assert_eq!(hermes.status.source, super::StatusSource::NotChecked);
}

#[test]
fn hermes_pane_output_does_not_infer_idle_from_prompt_like_line_with_prose_above() {
    // A status bar in scrollback followed by prose (`Run this:`) and a `❯ <command>` line is
    // agent output, not the live input box. Proximity alone would accept it; the intervening
    // line between the status bar and the prompt must be a box rule or blank.
    let mut hermes = pane_output_status_pane(772, Provider::Hermes, "agentscan: hermes");

    classify::apply_pane_output_status_fallback(
        &mut hermes,
        "⚕ gpt-5.5 │ ctx -- │ [░░░░░░░░░░] -- │ 5s │ ⏲ 0s │ ⚠ YOLO\n\
         Run this:\n\
         ❯ npm test\n",
    );

    assert_eq!(hermes.status.kind, StatusKind::Unknown);
    assert_eq!(hermes.status.source, super::StatusSource::NotChecked);
}

#[test]
fn hermes_pane_output_does_not_infer_idle_from_terminal_output_line_at_bottom() {
    // Agent output (e.g. a quoted shell prompt like `❯ npm test`) ends up as the last line of
    // the capture with a hermes status bar still sitting far above in scrollback. Nothing
    // follows the matched line so the current-frame guard trivially passes, but proximity to
    // the status bar must hold — an unrelated bottom line with no adjacent status bar is not
    // the live prompt.
    let mut hermes = pane_output_status_pane(771, Provider::Hermes, "agentscan: hermes");

    classify::apply_pane_output_status_fallback(
        &mut hermes,
        "⚕ gpt-5.5 │ ctx -- │ [░░░░░░░░░░] -- │ 5s │ ⏲ 0s │ ⚠ YOLO\n\
         ─────────────────────────────────────────────────────────────\n\
         ❯ Audit the build scripts\n\
         ─────────────────────────────────────────────────────────────\n\
         ⚕ Reading scripts/build.sh\n\
         ⚕ Reading scripts/test.sh\n\
         Run this to reproduce locally:\n\
         ❯ npm test\n",
    );

    assert_eq!(hermes.status.kind, StatusKind::Unknown);
    assert_eq!(hermes.status.source, super::StatusSource::NotChecked);
}

#[test]
fn hermes_pane_output_uses_current_prompt_over_stale_busy_footer() {
    let mut hermes = pane_output_status_pane(768, Provider::Hermes, "agentscan: hermes");

    classify::apply_pane_output_status_fallback(
        &mut hermes,
        "⚕ gpt-5.5 │ 65.4K/272K │ [██░░░░░░░░] 24% │ 2m │ ⏱ 1m 19s\n\
         ⚕ ❯ msg=interrupt · /queue · /bg · /steer · Ctrl+C cancel\n\
         \n\
         ⚕ Hermes\n\
         Done.\n\
         ⚕ gpt-5.5 │ 16K/272K │ [█░░░░░░░░░] 6% │ 6s │ ⏲ 3s\n\
         ❯\n",
    );

    assert_eq!(hermes.status.kind, StatusKind::Idle);
    assert_eq!(hermes.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn gemini_pane_output_marks_current_prompt_idle_only_after_provider_is_known() {
    let mut gemini = pane_output_status_pane(775, Provider::Gemini, "Gemini CLI");

    classify::apply_pane_output_status_fallback(
        &mut gemini,
        "Welcome to Gemini CLI\n\
         \n\
         >   Type your message or @path/to/file\n\
         Workspace   Sandbox    Model\n\
         ~/code/app  no sandbox gemini-2.5-pro\n",
    );

    assert_eq!(gemini.status.kind, StatusKind::Idle);
    assert_eq!(gemini.status.source, super::StatusSource::PaneOutput);

    let mut unknown = proc_fallback_pane(776, "zsh", "custom title");
    classify::apply_pane_output_status_fallback(
        &mut unknown,
        "Welcome to Gemini CLI\n\
         \n\
         >   Type your message or @path/to/file\n\
         Workspace   Sandbox    Model\n\
         ~/code/app  no sandbox gemini-2.5-pro\n",
    );

    assert_eq!(unknown.status.kind, StatusKind::Unknown);
    assert_eq!(unknown.status.source, super::StatusSource::NotChecked);
}

#[test]
fn gemini_pane_output_marks_action_required_busy() {
    let mut gemini = pane_output_status_pane(777, Provider::Gemini, "Gemini CLI");

    classify::apply_pane_output_status_fallback(
        &mut gemini,
        "╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ Action Required                                                             │\n\
         │ ?  ls list directory                                                        │\n\
         │ Allow execution of [ls]?                                                    │\n\
         │   1. Yes                                                                    │\n\
         │   2. No, suggest changes (esc)                                              │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n",
    );

    assert_eq!(gemini.status.kind, StatusKind::Busy);
    assert_eq!(gemini.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn gemini_pane_output_marks_auth_prompt_busy() {
    let mut gemini = pane_output_status_pane(780, Provider::Gemini, "Gemini CLI");

    classify::apply_pane_output_status_fallback(
        &mut gemini,
        "Gemini CLI v0.49.0\n\
         \n\
         ╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │  Opening authentication page in your browser.                                │\n\
         │                                                                              │\n\
         │  Do you want to continue?                                                    │\n\
         │                                                                              │\n\
         │  ● 1. Yes                                                                    │\n\
         │    2. No                                                                     │\n\
         │                                                                              │\n\
         │  Enter to select · ↑/↓ to navigate · Esc to cancel                           │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n",
    );

    assert_eq!(gemini.status.kind, StatusKind::Busy);
    assert_eq!(gemini.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn gemini_pane_output_refines_ready_title_when_auth_prompt_is_visible() {
    let mut gemini = pane_output_status_pane(781, Provider::Gemini, "◇  Ready (gemini)");
    gemini.status = super::PaneStatus::title(StatusKind::Idle);

    classify::apply_pane_output_status_fallback(
        &mut gemini,
        "Gemini CLI v0.49.0\n\
         ╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │  Opening authentication page in your browser.                                │\n\
         │  Do you want to continue?                                                    │\n\
         │  Enter to select · ↑/↓ to navigate · Esc to cancel                           │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n",
    );

    assert_eq!(gemini.status.kind, StatusKind::Busy);
    assert_eq!(gemini.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn gemini_pane_output_uses_current_idle_prompt_over_stale_auth_prompt() {
    let mut gemini = pane_output_status_pane(782, Provider::Gemini, "Gemini CLI");

    classify::apply_pane_output_status_fallback(
        &mut gemini,
        "Gemini CLI v0.49.0\n\
         ╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │  Opening authentication page in your browser.                                │\n\
         │  Do you want to continue?                                                    │\n\
         │  Enter to select · ↑/↓ to navigate · Esc to cancel                           │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n\
         \n\
         >   Type your message or @path/to/file\n\
         Workspace   Sandbox    Model\n\
         ~/code/app  no sandbox gemini-2.5-pro\n",
    );

    assert_eq!(gemini.status.kind, StatusKind::Idle);
    assert_eq!(gemini.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn gemini_pane_output_uses_current_busy_marker_over_stale_idle_prompt() {
    let mut gemini = pane_output_status_pane(778, Provider::Gemini, "Gemini CLI");

    classify::apply_pane_output_status_fallback(
        &mut gemini,
        ">   Type your message or @path/to/file\n\
         Workspace   Sandbox    Model\n\
         ~/code/app  no sandbox gemini-2.5-pro\n\
         \n\
         ╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ Action Required                                                             │\n\
         │ Apply this change?                                                          │\n\
         │   1. Yes                                                                    │\n\
         │   2. No, suggest changes (esc)                                              │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n",
    );

    assert_eq!(gemini.status.kind, StatusKind::Busy);
    assert_eq!(gemini.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn gemini_pane_output_does_not_infer_idle_from_stale_prompt() {
    let mut gemini = pane_output_status_pane(779, Provider::Gemini, "Gemini CLI");

    classify::apply_pane_output_status_fallback(
        &mut gemini,
        ">   Type your message or @path/to/file\n\
         Workspace   Sandbox    Model\n\
         ~/code/app  no sandbox gemini-2.5-pro\n\
         \n\
         ✦ Working on the latest request\n\
         Reading files\n\
         Preparing answer\n\
         Updating edits\n\
         Running tests\n\
         Collecting output\n\
         Still working\n\
         More output\n\
         Current line\n",
    );

    assert_eq!(gemini.status.kind, StatusKind::Unknown);
    assert_eq!(gemini.status.source, super::StatusSource::NotChecked);
}

#[test]
fn pi_pane_output_marks_current_editor_footer_idle_only_after_provider_is_known() {
    let mut pi = pane_output_status_pane(787, Provider::Pi, "π - agentscan");

    classify::apply_pane_output_status_fallback(
        &mut pi,
        "Completed prior turn.\n\
         ────────────────────────────────\n\
                                         \n\
         ────────────────────────────────\n\
         ~/code/app\n\
         0.0%/200k                                      claude-sonnet\n",
    );

    assert_eq!(pi.status.kind, StatusKind::Idle);
    assert_eq!(pi.status.source, super::StatusSource::PaneOutput);

    let mut unknown = proc_fallback_pane(788, "zsh", "custom title");
    classify::apply_pane_output_status_fallback(
        &mut unknown,
        "Completed prior turn.\n\
         ────────────────────────────────\n\
                                         \n\
         ────────────────────────────────\n\
         ~/code/app\n\
         0.0%/200k                                      claude-sonnet\n",
    );

    assert_eq!(unknown.status.kind, StatusKind::Unknown);
    assert_eq!(unknown.status.source, super::StatusSource::NotChecked);
}

#[test]
fn pi_pane_output_marks_current_working_loader_busy() {
    let mut pi = pane_output_status_pane(789, Provider::Pi, "π - agentscan");

    classify::apply_pane_output_status_fallback(
        &mut pi,
        "⠋ Working... (ctrl+c to interrupt)\n\
         ────────────────────────────────\n\
                                        \n\
         ────────────────────────────────\n\
         ~/code/app\n\
         0.0%/200k                                      claude-sonnet\n",
    );

    assert_eq!(pi.status.kind, StatusKind::Busy);
    assert_eq!(pi.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn pi_pane_output_marks_current_retry_loader_busy() {
    let mut pi = pane_output_status_pane(790, Provider::Pi, "π - agentscan");

    classify::apply_pane_output_status_fallback(
        &mut pi,
        "Retrying (2/3) in 4s... (ctrl+c to cancel)\n",
    );

    assert_eq!(pi.status.kind, StatusKind::Busy);
    assert_eq!(pi.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn pi_pane_output_uses_current_idle_footer_over_stale_busy_loader() {
    let mut pi = pane_output_status_pane(791, Provider::Pi, "π - agentscan");

    classify::apply_pane_output_status_fallback(
        &mut pi,
        "⠋ Working... (ctrl+c to interrupt)\n\
         Finished.\n\
         ────────────────────────────────\n\
                                         \n\
         ────────────────────────────────\n\
         ~/code/app\n\
         ?/200k                                      claude-sonnet\n",
    );

    assert_eq!(pi.status.kind, StatusKind::Idle);
    assert_eq!(pi.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn pi_pane_output_does_not_infer_idle_from_stale_editor_frame() {
    let mut pi = pane_output_status_pane(792, Provider::Pi, "π - agentscan");

    classify::apply_pane_output_status_fallback(
        &mut pi,
        "────────────────────────────────\n\
                                        \n\
         ────────────────────────────────\n\
         ~/code/app\n\
         0.0%/200k                                      claude-sonnet\n\
         \n\
         Planning edits\n\
         Reading files\n\
         Updating code\n\
         Running tests\n\
         Collecting output\n\
         Current line\n",
    );

    assert_eq!(pi.status.kind, StatusKind::Unknown);
    assert_eq!(pi.status.source, super::StatusSource::NotChecked);
}

#[test]
fn pi_pane_output_marks_idle_through_trailing_blank_padding() {
    // Real-world regression: a freshly started pi renders its editor frame and footer at
    // the top, leaving the rest of the taller pane blank. The trailing blank rows must not
    // push the current footer out of the "near the bottom" window.
    let mut pi = pane_output_status_pane(799, Provider::Pi, "π - agentscan");

    classify::apply_pane_output_status_fallback(
        &mut pi,
        "────────────────────────────────\n\
         \n\
         ────────────────────────────────\n\
         ~/code/agentscan (main)\n\
         $0.000 (sub) 0.0%/272k (auto)              (openai-codex) gpt-5.5 • medium\n\
         \n\
         \n\
         \n\
         \n\
         \n\
         \n\
         \n\
         \n",
    );

    assert_eq!(pi.status.kind, StatusKind::Idle);
    assert_eq!(pi.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn opencode_pane_output_marks_current_tui_prompt_idle_only_after_provider_is_known() {
    let mut opencode = pane_output_status_pane(780, Provider::Opencode, "OpenCode");

    classify::apply_pane_output_status_fallback(
        &mut opencode,
        "│ Build finished\n\
         ╹  Ask anything... \"Fix a TODO in the codebase\"\n\
            Build · gpt-5.1 OpenAI\n\
         ~/code/app                                                    /status\n",
    );

    assert_eq!(opencode.status.kind, StatusKind::Idle);
    assert_eq!(opencode.status.source, super::StatusSource::PaneOutput);

    let mut unknown = proc_fallback_pane(781, "zsh", "custom title");
    classify::apply_pane_output_status_fallback(
        &mut unknown,
        "│ Build finished\n\
         ╹  Ask anything... \"Fix a TODO in the codebase\"\n\
            Build · gpt-5.1 OpenAI\n\
         ~/code/app                                                    /status\n",
    );

    assert_eq!(unknown.status.kind, StatusKind::Unknown);
    assert_eq!(unknown.status.source, super::StatusSource::NotChecked);
}

#[test]
fn opencode_pane_output_marks_shell_prompt_idle() {
    let mut opencode = pane_output_status_pane(782, Provider::Opencode, "OC | Shell");

    classify::apply_pane_output_status_fallback(
        &mut opencode,
        "╹  Run a command... \"git status\"\n\
            Shell\n\
         ~/code/app                                                    /status\n",
    );

    assert_eq!(opencode.status.kind, StatusKind::Idle);
    assert_eq!(opencode.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn opencode_pane_output_marks_running_prompt_busy() {
    let mut opencode = pane_output_status_pane(783, Provider::Opencode, "OC | Working");

    classify::apply_pane_output_status_fallback(
        &mut opencode,
        "╹  Ask anything... \"Fix a TODO in the codebase\"\n\
            Build · gpt-5.1 OpenAI\n\
         ⠹ Reading files\n\
         esc interrupt\n",
    );

    assert_eq!(opencode.status.kind, StatusKind::Busy);
    assert_eq!(opencode.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn opencode_pane_output_marks_permission_prompt_busy() {
    let mut opencode = pane_output_status_pane(784, Provider::Opencode, "OC | Permission");

    classify::apply_pane_output_status_fallback(
        &mut opencode,
        "Permission required\n\
         → Edit src/app.rs\n\
         Allow once   Allow always   Reject\n\
         esc reject\n",
    );

    assert_eq!(opencode.status.kind, StatusKind::Busy);
    assert_eq!(opencode.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn opencode_pane_output_uses_current_busy_marker_over_stale_idle_prompt() {
    let mut opencode = pane_output_status_pane(785, Provider::Opencode, "OC | Working");

    classify::apply_pane_output_status_fallback(
        &mut opencode,
        "╹  Ask anything... \"Fix a TODO in the codebase\"\n\
            Build · gpt-5.1 OpenAI\n\
         ~/code/app                                                    /status\n\
         \n\
         Permission required\n\
         → Bash sleep 10\n\
         Allow once   Allow always   Reject\n",
    );

    assert_eq!(opencode.status.kind, StatusKind::Busy);
    assert_eq!(opencode.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn opencode_pane_output_does_not_force_busy_from_stale_approval_without_current_anchor() {
    // The 30-row capture holds an old approval prompt near the top, but the current bottom
    // frame is plain agent output with no live idle prompt or command bar below it. With no
    // current anchor the stale approval must not force busy — the honest answer is unknown.
    let mut opencode = pane_output_status_pane(795, Provider::Opencode, "OC | Working");

    classify::apply_pane_output_status_fallback(
        &mut opencode,
        "Permission required\n\
         → Bash sleep 10\n\
         Allow once   Allow always   Reject\n\
         \n\
         Reading files\n\
         Planning edits\n\
         Updating code\n\
         Running tests\n\
         Collecting results\n\
         Preparing response\n\
         Current line\n",
    );

    assert_eq!(opencode.status.kind, StatusKind::Unknown);
    assert_eq!(opencode.status.source, super::StatusSource::NotChecked);
}

#[test]
fn opencode_pane_output_does_not_infer_idle_from_stale_prompt() {
    let mut opencode = pane_output_status_pane(786, Provider::Opencode, "OC | Working");

    classify::apply_pane_output_status_fallback(
        &mut opencode,
        "╹  Ask anything... \"Fix a TODO in the codebase\"\n\
            Build · gpt-5.1 OpenAI\n\
         ~/code/app                                                    /status\n\
         \n\
         Planning edits\n\
         Reading files\n\
         Updating code\n\
         Running tests\n\
         Collecting results\n\
         Preparing response\n\
         Still working\n\
         Current line\n",
    );

    assert_eq!(opencode.status.kind, StatusKind::Unknown);
    assert_eq!(opencode.status.source, super::StatusSource::NotChecked);
}

#[test]
fn opencode_pane_output_marks_new_build_splash_idle() {
    // Newer "OpenCode Go" splash: the input box is centered with the command bar below it,
    // and the bottom status bar (path + version) sits far below at the true pane bottom.
    let mut opencode = pane_output_status_pane(801, Provider::Opencode, "OpenCode");

    classify::apply_pane_output_status_fallback(
        &mut opencode,
        "┃\n\
         ┃  Ask anything... \"Fix a TODO in the codebase\"\n\
         ┃\n\
         ┃  Build · Kimi K2.6 OpenCode Go\n\
         ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
         tab agents  ctrl+p commands\n\
         \n\
         ● Tip Use opencode run -f file.ts to attach files via CLI\n\
         \n\
         \n\
         ~/code/agentscan:main                                  1.15.11\n",
    );

    assert_eq!(opencode.status.kind, StatusKind::Idle);
    assert_eq!(opencode.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn opencode_pane_output_marks_wrapped_tip_splash_idle() {
    let mut opencode = pane_output_status_pane(813, Provider::Opencode, "OpenCode");

    classify::apply_pane_output_status_fallback(
        &mut opencode,
        concat!(
            "┃\n",
            "┃  Ask anything... \"What is the tech stack of this project?\"\n",
            "┃\n",
            "┃  Build · Kimi K2.6 OpenCode Go\n",
            "╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n",
            "tab agents  ctrl+p commands\n",
            "\n",
            "● Tip Press ctrl+f in the session list to pin a session so it stays at the\n",
            "      top\n",
            "\n",
            "\n",
            "\n",
            "~/code/agentscan:main                                  1.15.11\n",
        ),
    );

    assert_eq!(opencode.status.kind, StatusKind::Idle);
    assert_eq!(opencode.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn opencode_pane_output_does_not_treat_tip_followed_by_output_as_chrome() {
    let mut opencode = pane_output_status_pane(814, Provider::Opencode, "OpenCode");

    classify::apply_pane_output_status_fallback(
        &mut opencode,
        concat!(
            "┃\n",
            "┃  Ask anything... \"What is the tech stack of this project?\"\n",
            "┃\n",
            "┃  Build · Kimi K2.6 OpenCode Go\n",
            "╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n",
            "tab agents  ctrl+p commands\n",
            "\n",
            "● Tip Press ctrl+f in the session list to pin a session so it stays at the\n",
            "      cargo test\n",
            "\n",
            "~/code/agentscan:main                                  1.15.11\n",
        ),
    );

    assert_eq!(opencode.status.kind, StatusKind::Unknown);
    assert_eq!(opencode.status.source, super::StatusSource::NotChecked);
}

#[test]
fn opencode_pane_output_does_not_treat_ambiguous_tip_continuation_as_chrome() {
    let mut opencode = pane_output_status_pane(815, Provider::Opencode, "OpenCode");

    classify::apply_pane_output_status_fallback(
        &mut opencode,
        concat!(
            "┃\n",
            "┃  Ask anything... \"What is the tech stack of this project?\"\n",
            "┃\n",
            "┃  Build · Kimi K2.6 OpenCode Go\n",
            "╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n",
            "tab agents  ctrl+p commands\n",
            "\n",
            "\n",
            "\n",
            "\n",
            "\n",
            "\n",
            "● Tip Press ctrl+f in the session list to pin a session so it stays at the\n",
            "      cargo test\n",
        ),
    );

    assert_eq!(opencode.status.kind, StatusKind::Unknown);
    assert_eq!(opencode.status.source, super::StatusSource::NotChecked);
}

#[test]
fn opencode_pane_output_does_not_treat_top_after_other_tip_as_chrome() {
    let mut opencode = pane_output_status_pane(816, Provider::Opencode, "OpenCode");

    classify::apply_pane_output_status_fallback(
        &mut opencode,
        concat!(
            "┃\n",
            "┃  Ask anything... \"What is the tech stack of this project?\"\n",
            "┃\n",
            "┃  Build · Kimi K2.6 OpenCode Go\n",
            "╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n",
            "tab agents  ctrl+p commands\n",
            "\n",
            "● Tip Read the project notes before changing behavior\n",
            "      top\n",
            "~/code/agentscan:main                                  1.15.11\n",
        ),
    );

    assert_eq!(opencode.status.kind, StatusKind::Unknown);
    assert_eq!(opencode.status.source, super::StatusSource::NotChecked);
}

#[test]
fn opencode_pane_output_does_not_treat_top_without_spacer_as_chrome() {
    let mut opencode = pane_output_status_pane(817, Provider::Opencode, "OpenCode");

    classify::apply_pane_output_status_fallback(
        &mut opencode,
        concat!(
            "┃\n",
            "┃  Ask anything... \"What is the tech stack of this project?\"\n",
            "┃\n",
            "┃  Build · Kimi K2.6 OpenCode Go\n",
            "╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n",
            "tab agents  ctrl+p commands\n",
            "\n",
            "● Tip Press ctrl+f in the session list to pin a session so it stays at the\n",
            "      top\n",
            "~/code/agentscan:main                                  1.15.11\n",
        ),
    );

    assert_eq!(opencode.status.kind, StatusKind::Unknown);
    assert_eq!(opencode.status.source, super::StatusSource::NotChecked);
}

#[test]
fn opencode_pane_output_marks_new_build_active_session_idle() {
    // After a turn completes the placeholder is gone AND the live build drops the `tab agents`
    // hint, folding the command bar into the bottom status bar with token/cost usage stats. The
    // bordered input box's `╹▀▀▀` border is the stable anchor that keeps this the current idle
    // prompt — anchoring on `tab agents` alone would miss every used session.
    let mut opencode = pane_output_status_pane(802, Provider::Opencode, "OC | Greeting");

    classify::apply_pane_output_status_fallback(
        &mut opencode,
        "Hello! How can I help you today?\n\
         ▣  Build · Kimi K2.6 · 4.0s\n\
         ┃\n\
         ┃\n\
         ┃\n\
         ┃  Build · Kimi K2.6 OpenCode Go\n\
         ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
         11.8K (4%) · $0.01  ctrl+p commands    • OpenCode 1.15.11\n",
    );

    assert_eq!(opencode.status.kind, StatusKind::Idle);
    assert_eq!(opencode.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn opencode_pane_output_marks_live_build_used_session_idle() {
    // Real capture (build 1.15.11): a used session sits idle with the input box centered, a wide
    // blank gap below it, and the merged command/status bar (`<stats> ctrl+p commands · OpenCode`)
    // pinned at the true pane bottom. No placeholder, no `tab agents` — only the `╹▀▀▀` border
    // anchors it.
    let mut opencode = pane_output_status_pane(810, Provider::Opencode, "OC | Greeting");

    classify::apply_pane_output_status_fallback(
        &mut opencode,
        "  ┃  hi\n\
         \n\
         \n\
         \n\
         \n\
         \n\
            Hello! How can I help you today?\n\
            ▣  Build · Kimi K2.6 · 4.3s\n\
         \n\
         \n\
         \n\
           ┃\n\
           ┃\n\
           ┃  Build · Kimi K2.6 OpenCode Go                              ~/code/agentscan:main\n\
           ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
         \n\
         \n\
            11.8K (4%) · $0.01  ctrl+p commands    • OpenCode 1.15.11\n",
    );

    assert_eq!(opencode.status.kind, StatusKind::Idle);
    assert_eq!(opencode.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn opencode_pane_output_live_build_marks_busy_when_interrupt_hint_in_merged_bottom_bar() {
    // Real capture: the live build renders `esc interrupt` plus the braille run spinner in the
    // merged command/status bar directly *below* the input box border, not above it. The current
    // busy marker must win over the input box that the idle anchor now also recognizes.
    let mut opencode = pane_output_status_pane(811, Provider::Opencode, "OC | Repo review");

    classify::apply_pane_output_status_fallback(
        &mut opencode,
        "  ┃\n\
           ┃  Build · Kimi K2.6 OpenCode Go                              ~/code/agentscan:main\n\
           ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
            ⬝⬝⬝⬝⬝⬝⬝⬝  esc interrupt    139.2K (53%) · $0.23  ctrl+p commands    • OpenCode 1.15.11\n",
    );

    assert_eq!(opencode.status.kind, StatusKind::Busy);
    assert_eq!(opencode.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn opencode_pane_output_live_build_marks_busy_when_interrupt_hint_above_box_without_command_bar() {
    // Used session (no `tab agents`, so `command_bar_index` is None) with `esc interrupt`
    // rendered just *above* the input box. The box border is the only footer anchor, so the
    // current busy marker must still win over the input-box idle anchor rather than fall
    // through to idle.
    let mut opencode = pane_output_status_pane(812, Provider::Opencode, "OC | Greeting");

    classify::apply_pane_output_status_fallback(
        &mut opencode,
        "  Reading the codebase\n\
            esc interrupt\n\
           ┃\n\
           ┃  Build · Kimi K2.6 OpenCode Go\n\
           ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
            11.8K (4%) · $0.01  ctrl+p commands    • OpenCode 1.15.11\n",
    );

    assert_eq!(opencode.status.kind, StatusKind::Busy);
    assert_eq!(opencode.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn opencode_pane_output_new_build_yields_to_current_busy_marker() {
    // A busy marker still wins over the persistent command-bar input box.
    let mut opencode = pane_output_status_pane(803, Provider::Opencode, "OC | Working");

    classify::apply_pane_output_status_fallback(
        &mut opencode,
        "┃\n\
         ┃  Build · Kimi K2.6 OpenCode Go\n\
         ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
         tab agents  ctrl+p commands    • OpenCode 1.15.11\n\
         Permission required\n\
         Allow once   Allow always   Reject\n",
    );

    assert_eq!(opencode.status.kind, StatusKind::Busy);
    assert_eq!(opencode.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn opencode_pane_output_does_not_infer_new_build_idle_with_output_below_command_bar() {
    // A stale command bar + input box sit in the scrollback capture, but newer agent output
    // scrolled below them with no recognized busy marker. The command bar is no longer the
    // current prompt, so the pane must stay unknown rather than be reported idle.
    let mut opencode = pane_output_status_pane(805, Provider::Opencode, "OpenCode");

    classify::apply_pane_output_status_fallback(
        &mut opencode,
        "╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
         tab agents  ctrl+p commands    • OpenCode 1.15.11\n\
         Reading files\n\
         Planning edits\n\
         Updating code\n\
         Current line\n",
    );

    assert_eq!(opencode.status.kind, StatusKind::Unknown);
    assert_eq!(opencode.status.source, super::StatusSource::NotChecked);
}

#[test]
fn opencode_pane_output_new_build_marks_busy_when_interrupt_hint_above_persistent_command_bar() {
    // The newer build keeps its command bar pinned during a run, with the `esc interrupt`
    // status rendered just above the input box. The persistent command bar must not be read
    // as idle while that current interrupt hint is in the prompt footer.
    let mut opencode = pane_output_status_pane(809, Provider::Opencode, "OC | Greeting");

    classify::apply_pane_output_status_fallback(
        &mut opencode,
        "Reading the codebase\n\
         esc interrupt\n\
         ┃\n\
         ┃  Build · Kimi K2.6 OpenCode Go\n\
         ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
         tab agents  ctrl+p commands    • OpenCode 1.15.11\n",
    );

    assert_eq!(opencode.status.kind, StatusKind::Busy);
    assert_eq!(opencode.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn opencode_pane_output_new_build_idle_survives_stale_approval_in_scrollback() {
    // A resolved permission prompt is still in the scrollback capture above the current
    // command-bar input box. Because the live prompt sits below it, the pane is idle — the
    // stale approval must not preempt to busy.
    let mut opencode = pane_output_status_pane(806, Provider::Opencode, "OC | Greeting");

    classify::apply_pane_output_status_fallback(
        &mut opencode,
        "Permission required\n\
         Allow once   Allow always   Reject\n\
         Done. Applied the edit.\n\
         ┃\n\
         ┃  Build · Kimi K2.6 OpenCode Go\n\
         ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
         tab agents  ctrl+p commands    • OpenCode 1.15.11\n",
    );

    assert_eq!(opencode.status.kind, StatusKind::Idle);
    assert_eq!(opencode.status.source, super::StatusSource::PaneOutput);
}

#[test]
fn opencode_pane_output_does_not_treat_path_output_below_command_bar_as_chrome() {
    // A stale command bar sits in the scrollback capture with only file-path agent output
    // below it (common in coding output). Paths are not opencode chrome, so the stale
    // command bar is not the current prompt and the pane stays unknown.
    let mut opencode = pane_output_status_pane(807, Provider::Opencode, "OpenCode");

    classify::apply_pane_output_status_fallback(
        &mut opencode,
        "╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
         tab agents  ctrl+p commands\n\
         ~/code/agentscan/src/app/classify/pane_output.rs\n\
         /Users/auro/code/agentscan/src/app/scanner.rs\n",
    );

    assert_eq!(opencode.status.kind, StatusKind::Unknown);
    assert_eq!(opencode.status.source, super::StatusSource::NotChecked);
}

#[test]
fn opencode_pane_output_does_not_treat_semver_prose_below_command_bar_as_chrome() {
    // Agent output that merely mentions a semver or IP is not the pinned status bar, so a
    // stale command bar above such output must not be read as the current idle prompt.
    let mut opencode = pane_output_status_pane(808, Provider::Opencode, "OpenCode");

    classify::apply_pane_output_status_fallback(
        &mut opencode,
        "╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
         tab agents  ctrl+p commands\n\
         Updated SDK to 1.2.3 in the lockfile\n\
         See RFC 192.168.1.1 for details\n",
    );

    assert_eq!(opencode.status.kind, StatusKind::Unknown);
    assert_eq!(opencode.status.source, super::StatusSource::NotChecked);
}

#[test]
fn opencode_pane_output_does_not_infer_new_build_idle_without_input_box() {
    // The command bar alone (no bordered input box above it) is not enough to call idle.
    let mut opencode = pane_output_status_pane(804, Provider::Opencode, "OpenCode");

    classify::apply_pane_output_status_fallback(
        &mut opencode,
        "Reading files\n\
         Planning edits\n\
         tab agents  ctrl+p commands    • OpenCode 1.15.11\n",
    );

    assert_eq!(opencode.status.kind, StatusKind::Unknown);
    assert_eq!(opencode.status.source, super::StatusSource::NotChecked);
}

#[test]
fn opencode_display_metadata_uses_title_without_activity_state() {
    let opencode = classify::display_metadata(
        Some(Provider::Opencode),
        Some(super::ClassificationMatchKind::PaneTitle),
        None,
        "OC | Review patch",
        "zsh",
        "ai",
    );
    assert_eq!(opencode.label, "Review patch");
    assert_eq!(opencode.activity_label, None);

    let opencode_default = classify::display_metadata(
        Some(Provider::Opencode),
        Some(super::ClassificationMatchKind::PaneTitle),
        None,
        "OpenCode",
        "zsh",
        "ai",
    );
    assert_eq!(opencode_default.label, "OpenCode");
    assert_eq!(opencode_default.activity_label, None);
}

#[test]
fn gemini_display_metadata_separates_context_from_activity() {
    let idle = classify::display_metadata(
        Some(Provider::Gemini),
        Some(super::ClassificationMatchKind::PaneTitle),
        None,
        "◇  Ready (workspace)",
        "zsh",
        "ai",
    );
    assert_eq!(idle.label, "workspace");
    assert_eq!(idle.activity_label, None);

    let static_title = classify::display_metadata(
        Some(Provider::Gemini),
        Some(super::ClassificationMatchKind::PaneTitle),
        None,
        "Gemini CLI (workspace)",
        "zsh",
        "ai",
    );
    assert_eq!(static_title.label, "workspace");
    assert_eq!(static_title.activity_label, None);

    let thought = classify::display_metadata(
        Some(Provider::Gemini),
        Some(super::ClassificationMatchKind::PaneTitle),
        None,
        "✦  Processing request (workspace)",
        "zsh",
        "ai",
    );
    assert_eq!(thought.label, "Processing request");
    assert_eq!(
        thought.activity_label.as_deref(),
        Some("Processing request")
    );
}

#[test]
fn codex_status_activity_labels_strip_wrapper_suffixes() {
    let status_first = classify::display_metadata(
        Some(Provider::Codex),
        None,
        None,
        "Working | Review patch: codex",
        "codex",
        "editor",
    );
    assert_eq!(status_first.label, "Review patch");
    assert_eq!(status_first.activity_label.as_deref(), Some("Review patch"));

    let status_last = classify::display_metadata(
        Some(Provider::Codex),
        None,
        None,
        "Ready | Working: codex",
        "codex",
        "editor",
    );
    assert_eq!(status_last.label, "Ready");
    assert_eq!(status_last.activity_label.as_deref(), Some("Ready"));

    let status_middle = classify::display_metadata(
        Some(Provider::Codex),
        None,
        None,
        "gpt-5.5 | Ready | Review patch: codex",
        "codex",
        "editor",
    );
    assert_eq!(status_middle.label, "gpt-5.5 | Review patch");
    assert_eq!(
        status_middle.activity_label.as_deref(),
        Some("gpt-5.5 | Review patch")
    );

    let status_like_activity_segment = classify::display_metadata(
        Some(Provider::Codex),
        None,
        None,
        "Ready | gpt-5.5 | Working",
        "codex",
        "editor",
    );
    assert_eq!(status_like_activity_segment.label, "Ready | gpt-5.5");
    assert_eq!(
        status_like_activity_segment.activity_label.as_deref(),
        Some("Ready | gpt-5.5")
    );

    let ambiguous_middle_status = classify::display_metadata(
        Some(Provider::Codex),
        None,
        None,
        "Review | Ready | notes",
        "codex",
        "editor",
    );
    assert_eq!(ambiguous_middle_status.label, "Review | notes");
    assert_eq!(
        ambiguous_middle_status.activity_label.as_deref(),
        Some("Review | notes")
    );

    let non_command_codex_flag_text = classify::display_metadata(
        Some(Provider::Codex),
        None,
        None,
        "Investigate codex --model flag parsing | Working",
        "codex",
        "editor",
    );
    assert_eq!(
        non_command_codex_flag_text.label,
        "Investigate codex --model flag parsing"
    );
    assert_eq!(
        non_command_codex_flag_text.activity_label.as_deref(),
        Some("Investigate codex --model flag parsing")
    );

    let default_spinner_last = classify::display_metadata(
        Some(Provider::Codex),
        None,
        None,
        "agentscan ⠹",
        "codex",
        "editor",
    );
    assert_eq!(default_spinner_last.label, "agentscan");
    assert_eq!(
        default_spinner_last.activity_label.as_deref(),
        Some("agentscan")
    );

    let attached_spinner = classify::display_metadata(
        Some(Provider::Codex),
        None,
        None,
        "⠹agentscan",
        "codex",
        "editor",
    );
    assert_eq!(attached_spinner.label, "agentscan");
    assert_eq!(attached_spinner.activity_label.as_deref(), Some("agentscan"));

    let action_required = classify::display_metadata(
        Some(Provider::Codex),
        None,
        None,
        "[ ! ] Action Required | agentscan",
        "codex",
        "editor",
    );
    assert_eq!(action_required.label, "agentscan");
    assert_eq!(action_required.activity_label.as_deref(), Some("agentscan"));
}

#[test]
fn codex_display_label_corpus_covers_upstream_title_items() {
    let cases = [
        (
            "⠹ agentscan",
            "agentscan",
            Some("agentscan"),
            "default busy title",
        ),
        (
            "agentscan⠹",
            "agentscan",
            Some("agentscan"),
            "busy title with attached trailing spinner",
        ),
        (
            "agent⠹scan",
            "agent⠹scan",
            Some("agent⠹scan"),
            "spinner-like glyph inside normal title text",
        ),
        (
            "agentscan",
            "agentscan",
            Some("agentscan"),
            "default idle title has no state but still labels the known Codex pane",
        ),
        (
            "Ready | Review code quality in repository",
            "Review code quality in repository",
            Some("Review code quality in repository"),
            "this host's run-state/thread-title config",
        ),
        (
            "Review code quality in repository | Ready",
            "Review code quality in repository",
            Some("Review code quality in repository"),
            "thread-title/run-state reversed",
        ),
        (
            "gpt-5.5 | Ready | Review code quality in repository",
            "gpt-5.5 | Review code quality in repository",
            Some("gpt-5.5 | Review code quality in repository"),
            "run-state surrounded by other configured title items",
        ),
        (
            "Ready | gpt-5.5 | Working",
            "Ready | gpt-5.5",
            Some("Ready | gpt-5.5"),
            "status-like activity segment before rightmost run-state",
        ),
        (
            "Tasks 2/5 | gpt-5.5 | Ready | main",
            "Tasks 2/5 | gpt-5.5 | main",
            Some("Tasks 2/5 | gpt-5.5 | main"),
            "run-state before trailing configured title items",
        ),
        (
            "Review | Ready | notes",
            "Review | notes",
            Some("Review | notes"),
            "middle run-state segment without tagged item provenance",
        ),
        (
            "[ ! ] Action Required | agentscan",
            "agentscan",
            Some("agentscan"),
            "action-required activity prefix",
        ),
        (
            "[ ! ] Action Required | Ready | agentscan",
            "agentscan",
            Some("agentscan"),
            "action-required activity prefix takes precedence over later run-state",
        ),
        (
            "Ready | [ ! ] Action Required | agentscan",
            "agentscan",
            Some("agentscan"),
            "action-required activity prefix removes earlier run-state",
        ),
        (
            "repo: /path/lgpt.sh | Working",
            "repo",
            Some("repo"),
            "legacy wrapper title still strips wrapper suffix",
        ),
    ];

    for (title, expected_label, expected_activity, context) in cases {
        let display = classify::display_metadata(
            Some(Provider::Codex),
            None,
            None,
            title,
            "codex",
            "editor",
        );
        assert_eq!(display.label, expected_label, "{context}: label");
        assert_eq!(
            display.activity_label.as_deref(),
            expected_activity,
            "{context}: activity label"
        );
    }
}

#[test]
fn non_codex_status_shaped_titles_preserve_display_label() {
    let unresolved =
        classify::display_metadata(None, None, None, "Working | deploy notes", "zsh", "editor");
    assert_eq!(unresolved.label, "Working | deploy notes");
    assert_eq!(unresolved.activity_label, None);
}

#[test]
fn display_metadata_prefers_window_name_for_metadata_only_cursor_cli() {
    let cursor = classify::display_metadata(
        Some(Provider::CursorCli),
        Some(super::ClassificationMatchKind::PaneMetadata),
        None,
        "zsh",
        "zsh",
        "agent-pane",
    );
    assert_eq!(cursor.label, "agent-pane");
    assert_eq!(cursor.activity_label, None);
}

#[test]
fn display_metadata_keeps_task_titles_for_metadata_only_cursor_cli() {
    let cursor = classify::display_metadata(
        Some(Provider::CursorCli),
        Some(super::ClassificationMatchKind::PaneMetadata),
        None,
        "Implement parser",
        "zsh",
        "agent-pane",
    );
    assert_eq!(cursor.label, "Implement parser");
    assert_eq!(cursor.activity_label.as_deref(), Some("Implement parser"));
}

#[test]
fn display_metadata_ignores_stale_prefixed_titles_for_other_providers() {
    let stale_codex = classify::display_metadata(
        Some(Provider::Claude),
        Some(super::ClassificationMatchKind::PaneCurrentCommand),
        None,
        "repo: codex",
        "claude",
        "review",
    );
    assert_eq!(stale_codex.label, "review");
    assert_eq!(stale_codex.activity_label, None);

    let stale_claude = classify::display_metadata(
        Some(Provider::Codex),
        Some(super::ClassificationMatchKind::PaneCurrentCommand),
        None,
        "Claude Code | Ready",
        "codex",
        "agent-pane",
    );
    assert_eq!(stale_claude.label, "agent-pane");
    assert_eq!(stale_claude.activity_label, None);

    let stale_opencode = classify::display_metadata(
        Some(Provider::Codex),
        Some(super::ClassificationMatchKind::PaneCurrentCommand),
        None,
        "OC | Query planner",
        "codex",
        "agent-pane",
    );
    assert_eq!(stale_opencode.label, "agent-pane");
    assert_eq!(stale_opencode.activity_label, None);

    let stale_copilot = classify::display_metadata(
        Some(Provider::Codex),
        Some(super::ClassificationMatchKind::PaneCurrentCommand),
        None,
        "Copilot | Working",
        "codex",
        "agent-pane",
    );
    assert_eq!(stale_copilot.label, "agent-pane");
    assert_eq!(stale_copilot.activity_label, None);

    let stale_cursor = classify::display_metadata(
        Some(Provider::Claude),
        Some(super::ClassificationMatchKind::PaneMetadata),
        None,
        "Cursor CLI | Query planner",
        "zsh",
        "review",
    );
    assert_eq!(stale_cursor.label, "review");
    assert_eq!(stale_cursor.activity_label, None);

    let stale_pi = classify::display_metadata(
        Some(Provider::Claude),
        Some(super::ClassificationMatchKind::PaneMetadata),
        None,
        "π - refactor - agentscan",
        "zsh",
        "review",
    );
    assert_eq!(stale_pi.label, "review");
    assert_eq!(stale_pi.activity_label, None);
}

#[test]
fn display_metadata_keeps_plain_ascii_pi_task_titles_for_non_pi_providers() {
    let plain_ascii_pi = classify::display_metadata(
        Some(Provider::Claude),
        Some(super::ClassificationMatchKind::PaneCurrentCommand),
        None,
        "pi - refactor - agentscan",
        "claude",
        "review",
    );
    assert_eq!(plain_ascii_pi.label, "pi - refactor - agentscan");
    assert_eq!(
        plain_ascii_pi.activity_label.as_deref(),
        Some("pi - refactor - agentscan")
    );
}
