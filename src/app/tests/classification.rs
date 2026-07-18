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
    // Disabled fallback must not even capture the process table: the per-scan
    // snapshot is lazy and only a gated candidate's first query triggers it.
    assert_eq!(inspector.snapshot_captures(), 0);
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
        agent_pid: None,
        agent_version: None,
        agent_model: None,
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
        agent_pid: None,
        agent_version: None,
        agent_model: None,
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
fn pidless_metadata_keeps_v0_trust_without_process_inspection() {
    let inspector = FakeProcessInspector::new([]);
    let panes = classify::panes_from_rows_with_proc_fallback(
        vec![tmux_pane_row(500)
            .command("codex")
            .title("Ready")
            .agent_provider("claude")
            .agent_state("busy")
            .agent_model("claude-opus-4-1")
            .build()],
        &inspector,
    );

    assert_eq!(panes[0].provider, Some(Provider::Claude));
    assert_eq!(panes[0].status, PaneStatus::metadata(StatusKind::Busy));
    assert_eq!(
        panes[0].agent_metadata.model.as_deref(),
        Some("claude-opus-4-1")
    );
    assert_eq!(inspector.snapshot_captures(), 0);
}

#[test]
fn live_descendant_pid_trusts_the_entire_metadata_block() {
    let inspector = FakeProcessInspector::with_single_process(500, 501, "helper", &["helper"]);
    let panes = classify::panes_from_rows_with_proc_fallback(
        vec![tmux_pane_row(500)
            .command("codex")
            .title("Ready")
            .agent_provider("claude")
            .agent_label("Trusted task")
            .agent_state("waiting")
            .agent_session_id("session-1")
            .agent_pid("501")
            .agent_version("1")
            .agent_model("claude-opus-4-1")
            .build()],
        &inspector,
    );

    assert_eq!(panes[0].provider, Some(Provider::Claude));
    assert_eq!(panes[0].display.label, "Trusted task");
    assert_eq!(panes[0].status, PaneStatus::metadata(StatusKind::Waiting));
    assert_eq!(panes[0].agent_metadata.pid.as_deref(), Some("501"));
    assert_eq!(panes[0].agent_metadata.session_id.as_deref(), Some("session-1"));
    assert_eq!(panes[0].agent_metadata.v.as_deref(), Some("1"));
    assert_eq!(
        panes[0].agent_metadata.model.as_deref(),
        Some("claude-opus-4-1")
    );
    let inspect = output::inspect_text(&panes[0]);
    assert!(inspect.contains("  pid: 501"));
    assert!(inspect.contains("  v: 1"));
    assert!(inspect.contains("  model: claude-opus-4-1"));
    assert_eq!(inspector.calls(), vec![500]);
    assert_eq!(inspector.snapshot_captures(), 1);
}

#[test]
fn pane_root_pid_is_trusted_as_equal_process_tree_member() {
    let inspector = FakeProcessInspector::with_single_process(500, 500, "codex", &["codex"]);
    let panes = classify::panes_from_rows_with_proc_fallback(
        vec![tmux_pane_row(500)
            .command("codex")
            .title("Ready")
            .agent_provider("claude")
            .agent_state("busy")
            .agent_pid("500")
            .build()],
        &inspector,
    );

    assert_eq!(panes[0].provider, Some(Provider::Claude));
    assert_eq!(panes[0].status, PaneStatus::metadata(StatusKind::Busy));
}

#[test]
fn invalid_published_pid_rejects_the_entire_metadata_block() {
    for (published_pid, processes, expected_snapshot_captures) in [
        (
            "999",
            vec![process_evidence(501, "helper", &["helper"])],
            1,
        ),
        ("501", Vec::new(), 1),
        (
            "0",
            vec![process_evidence(501, "helper", &["helper"])],
            0,
        ),
        (
            "garbage",
            vec![process_evidence(501, "helper", &["helper"])],
            0,
        ),
    ] {
        let inspector = FakeProcessInspector::with_processes([(500, processes)]);
        let panes = classify::panes_from_rows_with_proc_fallback(
            vec![tmux_pane_row(500)
                .command("codex")
                .title("Ready")
                .agent_provider("claude")
                .agent_label("Stale task")
                .agent_cwd("/stale")
                .agent_state("busy")
                .agent_session_id("stale-session")
                .agent_pid(published_pid)
                .agent_version("1")
                .agent_model("stale-model")
                .build()],
            &inspector,
        );

        assert_eq!(panes[0].provider, Some(Provider::Codex), "pid={published_pid}");
        assert_eq!(panes[0].status, PaneStatus::title(StatusKind::Idle));
        assert_eq!(panes[0].display.label, "Ready");
        assert_eq!(panes[0].agent_metadata, crate::app::AgentMetadata::default());
        assert_eq!(
            inspector.snapshot_captures(),
            expected_snapshot_captures,
            "pid={published_pid}"
        );
    }
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
    // The fixture predates recency stamping: the absent field reads as None.
    assert_eq!(snapshot.panes[0].last_focus_seq, None);
}

#[test]
fn pane_last_focus_seq_round_trips_and_omits_none() {
    let snapshot: SnapshotEnvelope =
        serde_json::from_str(CACHE_SNAPSHOT_FIXTURE).expect("cache fixture should parse");
    let mut pane = snapshot.panes[0].clone();

    let unstamped = serde_json::to_string(&pane).expect("serialize");
    assert!(
        !unstamped.contains("last_focus_seq"),
        "None must be omitted from the wire form"
    );

    pane.last_focus_seq = Some(42);
    let stamped = serde_json::to_string(&pane).expect("serialize");
    let reparsed: PaneRecord = serde_json::from_str(&stamped).expect("round trip");
    assert_eq!(reparsed.last_focus_seq, Some(42));
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
                agent_pid: None,
                agent_version: None,
                agent_model: None,
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
                agent_pid: None,
                agent_version: None,
                agent_model: None,
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
                agent_pid: None,
                agent_version: None,
                agent_model: None,
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
    assert_eq!(StatusKind::Waiting.as_str(), "waiting");
    assert_eq!(StatusKind::Unknown.as_str(), "unknown");
    assert_eq!(
        serde_json::to_string(&StatusKind::Waiting).expect("serialize waiting status"),
        "\"waiting\""
    );
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
fn kimi_code_display_metadata_suppresses_generic_startup_title() {
    let generic = classify::display_metadata(
        Some(Provider::KimiCode),
        Some(super::ClassificationMatchKind::PaneCurrentCommand),
        None,
        "Kimi Code",
        "kimi",
        "kimi",
    );
    assert_eq!(generic.label, "Kimi Code");
    assert_eq!(generic.activity_label, None);

    let session = classify::display_metadata(
        Some(Provider::KimiCode),
        Some(super::ClassificationMatchKind::PaneCurrentCommand),
        None,
        "reply with exactly OK",
        "kimi",
        "kimi",
    );
    assert_eq!(session.label, "reply with exactly OK");
    assert_eq!(
        session.activity_label.as_deref(),
        Some("reply with exactly OK")
    );
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
