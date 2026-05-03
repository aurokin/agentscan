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
fn proc_fallback_leaves_candidate_unknown_without_provider_evidence() {
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%700".to_string(),
        pane_pid: 700,
        pane_current_command: "node".to_string(),
        pane_title_raw: "Working".to_string(),
        pane_tty: "/dev/pts/700".to_string(),
        pane_current_path: "/tmp/node-wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
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
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%703".to_string(),
        pane_pid: 703,
        pane_current_command: "node".to_string(),
        pane_title_raw: "Ready".to_string(),
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
    });
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
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%704".to_string(),
        pane_pid: 704,
        pane_current_command: "node".to_string(),
        pane_title_raw: "✳ Refactor auth flow".to_string(),
        pane_tty: "/dev/pts/704".to_string(),
        pane_current_path: "/tmp/node-wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
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
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%705".to_string(),
        pane_pid: 705,
        pane_current_command: "node".to_string(),
        pane_title_raw: "Review deployment plan".to_string(),
        pane_tty: "/dev/pts/705".to_string(),
        pane_current_path: "/tmp/node-wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
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
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%706".to_string(),
        pane_pid: 706,
        pane_current_command: "node".to_string(),
        pane_title_raw: "Review deployment plan".to_string(),
        pane_tty: "/dev/pts/706".to_string(),
        pane_current_path: "/tmp/node-wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
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
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%707".to_string(),
        pane_pid: 707,
        pane_current_command: "node".to_string(),
        pane_title_raw: "Working".to_string(),
        pane_tty: "/dev/pts/707".to_string(),
        pane_current_path: "/tmp/node-wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
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
fn proc_fallback_resolves_claude_from_title_glyph_and_descendant_command() {
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%711".to_string(),
        pane_pid: 711,
        pane_current_command: "2.1.119".to_string(),
        pane_title_raw: "✳ Analyze Linear Issue AUR-126 and plan implementation".to_string(),
        pane_tty: "/dev/pts/711".to_string(),
        pane_current_path: "/tmp/claude-wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
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
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%712".to_string(),
        pane_pid: 712,
        pane_current_command: "2.1.119".to_string(),
        pane_title_raw: "Ready".to_string(),
        pane_tty: "/dev/pts/712".to_string(),
        pane_current_path: "/tmp/claude-wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
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
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%713".to_string(),
        pane_pid: 713,
        pane_current_command: "2.1.119".to_string(),
        pane_title_raw: "Ready".to_string(),
        pane_tty: "/dev/pts/713".to_string(),
        pane_current_path: "/tmp/unknown".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
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
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%705".to_string(),
        pane_pid: 705,
        pane_current_command: "node".to_string(),
        pane_title_raw: "worker-a".to_string(),
        pane_tty: "/dev/pts/705".to_string(),
        pane_current_path: "/tmp/node-wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
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
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%707".to_string(),
        pane_pid: 707,
        pane_current_command: "node".to_string(),
        pane_title_raw: "worker-a".to_string(),
        pane_tty: "/dev/pts/707".to_string(),
        pane_current_path: "/tmp/node-wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
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
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%706".to_string(),
        pane_pid: 706,
        pane_current_command: "node".to_string(),
        pane_title_raw: "worker-a".to_string(),
        pane_tty: "/dev/pts/706".to_string(),
        pane_current_path: "/tmp/node-wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
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
        let mut pane = classify::pane_from_row(super::TmuxPaneRow {
            session_name: "ambiguous".to_string(),
            window_index: 1,
            pane_index: 1,
            pane_id: format!("%{pid}"),
            pane_pid: pid,
            pane_current_command: "node".to_string(),
            pane_title_raw: "Working".to_string(),
            pane_tty: format!("/dev/pts/{pid}"),
            pane_current_path: "/tmp/node-wrapper".to_string(),
            window_name: "ai".to_string(),
            session_id: None,
            window_id: None,
            agent_provider: None,
            agent_label: None,
            agent_cwd: None,
            agent_state: None,
            agent_session_id: None,
        });
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
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "metadata".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%701".to_string(),
        pane_pid: 701,
        pane_current_command: "node".to_string(),
        pane_title_raw: "Working".to_string(),
        pane_tty: "/dev/pts/701".to_string(),
        pane_current_path: "/tmp/node-wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: Some("claude".to_string()),
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
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
    let mut pane = classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: "%702".to_string(),
        pane_pid: 702,
        pane_current_command: "make".to_string(),
        pane_title_raw: "(bront) ~/code/agent-wrapper".to_string(),
        pane_tty: "/dev/pts/702".to_string(),
        pane_current_path: "/tmp/wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    });
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
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Pi), "π - refactor - agentscan"),
        "refactor - agentscan"
    );
    assert_eq!(
        classify::normalize_title_for_display(Some(Provider::Pi), "pi - agentscan"),
        "pi - agentscan"
    );
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
fn copilot_pane_output_marks_busy_only_after_provider_is_known() {
    let mut copilot = pane_output_status_pane(745, Provider::Copilot, "GitHub Copilot");

    classify::apply_pane_output_status_fallback(
        &mut copilot,
        "❯ Review patch\n\n\
         ● Thinking (Esc to cancel · 616 B)\n\
         /tmp/probe [main]\n\
         ────────────────────\n\
         ❯\n\
         ────────────────────\n\
         / commands · ? help\n",
    );

    assert_eq!(copilot.status.kind, StatusKind::Busy);
    assert_eq!(copilot.status.source, super::StatusSource::PaneOutput);

    let mut unknown = proc_fallback_pane(746, "node", "custom title");
    classify::apply_pane_output_status_fallback(
        &mut unknown,
        "❯ Review patch\n\n\
         ● Thinking (Esc to cancel · 616 B)\n\
         /tmp/probe [main]\n\
         ────────────────────\n\
         ❯\n\
         ────────────────────\n\
         / commands · ? help\n",
    );

    assert_eq!(unknown.status.kind, StatusKind::Unknown);
    assert_eq!(unknown.status.source, super::StatusSource::NotChecked);
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
