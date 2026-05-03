fn proc_fallback_pane(pid: u32, command: &str, title: &str) -> PaneRecord {
    classify::pane_from_row(super::TmuxPaneRow {
        session_name: "ambiguous".to_string(),
        window_index: 1,
        pane_index: 1,
        pane_id: format!("%{pid}"),
        pane_pid: pid,
        pane_current_command: command.to_string(),
        pane_title_raw: title.to_string(),
        pane_tty: format!("/dev/pts/{pid}"),
        pane_current_path: "/tmp/proc-wrapper".to_string(),
        window_name: "ai".to_string(),
        session_id: None,
        window_id: None,
        agent_provider: None,
        agent_label: None,
        agent_cwd: None,
        agent_state: None,
        agent_session_id: None,
    })
}

fn pane_output_status_pane(pid: u32, provider: Provider, title: &str) -> PaneRecord {
    let mut pane = proc_fallback_pane(pid, "node", title);
    pane.provider = Some(provider);
    pane.status = PaneStatus::not_checked();
    pane
}

struct FakeProcessInspector {
    processes_by_pid: std::collections::HashMap<u32, Vec<proc::ProcessEvidence>>,
    foreground_by_tty: std::collections::HashMap<String, Vec<proc::ProcessEvidence>>,
    calls: RefCell<Vec<u32>>,
    foreground_calls: RefCell<Vec<String>>,
}

impl FakeProcessInspector {
    fn new(entries: impl IntoIterator<Item = (u32, Vec<String>)>) -> Self {
        Self {
            processes_by_pid: entries
                .into_iter()
                .map(|(pid, commands)| {
                    (
                        pid,
                        commands
                            .into_iter()
                            .map(|command| proc::ProcessEvidence {
                                pid,
                                command: command.clone(),
                                argv: vec![command],
                                env: Vec::new(),
                            })
                            .collect(),
                    )
                })
                .collect(),
            foreground_by_tty: std::collections::HashMap::new(),
            calls: RefCell::new(Vec::new()),
            foreground_calls: RefCell::new(Vec::new()),
        }
    }

    fn with_processes(
        entries: impl IntoIterator<Item = (u32, Vec<proc::ProcessEvidence>)>,
    ) -> Self {
        Self {
            processes_by_pid: entries.into_iter().collect(),
            foreground_by_tty: std::collections::HashMap::new(),
            calls: RefCell::new(Vec::new()),
            foreground_calls: RefCell::new(Vec::new()),
        }
    }

    fn with_foreground(
        descendants: impl IntoIterator<Item = (u32, Vec<String>)>,
        foreground: impl IntoIterator<Item = (String, Vec<String>)>,
    ) -> Self {
        let mut inspector = Self::new(descendants);
        inspector.foreground_by_tty = foreground
            .into_iter()
            .map(|(tty, commands)| {
                (
                    tty,
                    commands
                        .into_iter()
                        .map(|command| proc::ProcessEvidence {
                            pid: 0,
                            command: command.clone(),
                            argv: vec![command],
                            env: Vec::new(),
                        })
                        .collect(),
                )
            })
            .collect();
        inspector
    }

    fn calls(&self) -> Vec<u32> {
        self.calls.borrow().clone()
    }

    fn foreground_calls(&self) -> Vec<String> {
        self.foreground_calls.borrow().clone()
    }
}

impl proc::ProcessInspector for FakeProcessInspector {
    fn descendant_processes(&self, root_pid: u32) -> anyhow::Result<Vec<proc::ProcessEvidence>> {
        self.calls.borrow_mut().push(root_pid);
        Ok(self
            .processes_by_pid
            .get(&root_pid)
            .cloned()
            .unwrap_or_default())
    }

    fn foreground_processes(&self, pane_tty: &str) -> anyhow::Result<Vec<proc::ProcessEvidence>> {
        self.foreground_calls
            .borrow_mut()
            .push(pane_tty.to_string());
        Ok(self
            .foreground_by_tty
            .get(pane_tty)
            .cloned()
            .unwrap_or_default())
    }
}

fn assert_fixture_codex_cases(panes: &[PaneRecord]) {
    let codex_plain_working = pane_by_id(panes, "%178");
    assert_eq!(codex_plain_working.provider, Some(Provider::Codex));
    assert_eq!(codex_plain_working.status.kind, StatusKind::Busy);
    assert_eq!(codex_plain_working.display.label, "Working");
    assert_eq!(codex_plain_working.display.activity_label, None);

    let codex_working = pane_by_id(panes, "%191");
    assert_eq!(codex_working.provider, Some(Provider::Codex));
    assert_eq!(codex_working.status.kind, StatusKind::Busy);
    assert_eq!(codex_working.display.label, "agentscan");
    assert_eq!(
        codex_working.display.activity_label.as_deref(),
        Some("agentscan")
    );

    let codex_ready = pane_by_id(panes, "%67");
    assert_eq!(codex_ready.status.kind, StatusKind::Idle);
    assert_eq!(codex_ready.display.activity_label, None);

    let codex_waiting = pane_by_id(panes, "%194");
    assert_eq!(codex_waiting.provider, Some(Provider::Codex));
    assert_eq!(codex_waiting.status.kind, StatusKind::Busy);
    assert_eq!(codex_waiting.display.label, "agentscan");
    assert_eq!(
        codex_waiting.display.activity_label.as_deref(),
        Some("agentscan")
    );
}

fn assert_fixture_claude_cases(panes: &[PaneRecord]) {
    let claude_idle = pane_by_id(panes, "%41");
    assert_eq!(claude_idle.provider, Some(Provider::Claude));
    assert_eq!(claude_idle.status.kind, StatusKind::Idle);
    assert_eq!(claude_idle.display.label, "Review and summarize todo list");
    assert_eq!(
        claude_idle.display.activity_label.as_deref(),
        Some("Review and summarize todo list")
    );

    let claude_busy = pane_by_id(panes, "%223");
    assert_eq!(claude_busy.status.kind, StatusKind::Busy);

    let claude_title_busy = pane_by_id(panes, "%224");
    assert_eq!(claude_title_busy.provider, Some(Provider::Claude));
    assert_eq!(claude_title_busy.status.kind, StatusKind::Busy);
    assert_eq!(claude_title_busy.display.label, "Working");
    assert_eq!(claude_title_busy.display.activity_label, None);

    let claude_title_idle = pane_by_id(panes, "%225");
    assert_eq!(claude_title_idle.provider, Some(Provider::Claude));
    assert_eq!(claude_title_idle.status.kind, StatusKind::Idle);
    assert_eq!(claude_title_idle.display.label, "Ready");
    assert_eq!(claude_title_idle.display.activity_label, None);

    let claude_task_idle = pane_by_id(panes, "%275");
    assert_eq!(claude_task_idle.provider, Some(Provider::Claude));
    assert_eq!(claude_task_idle.status.kind, StatusKind::Idle);
    assert_eq!(
        claude_task_idle.display.label,
        "Design GitHub bot with Claude agent dashboard"
    );
    assert_eq!(
        claude_task_idle.display.activity_label.as_deref(),
        Some("Design GitHub bot with Claude agent dashboard")
    );
}

fn assert_fixture_gemini_cases(panes: &[PaneRecord]) {
    let gemini_idle = pane_by_id(panes, "%300");
    assert_eq!(gemini_idle.provider, Some(Provider::Gemini));
    assert_eq!(gemini_idle.status.kind, StatusKind::Idle);
    assert_eq!(gemini_idle.status.source, super::StatusSource::TmuxTitle);
    assert_eq!(gemini_idle.display.label, "Ready");
    assert_eq!(gemini_idle.display.activity_label, None);
    assert_eq!(
        gemini_idle.classification.matched_by,
        Some(super::ClassificationMatchKind::PaneCurrentCommand)
    );

    let gemini_busy = pane_by_id(panes, "%306");
    assert_eq!(gemini_busy.provider, Some(Provider::Gemini));
    assert_eq!(gemini_busy.status.kind, StatusKind::Busy);
    assert_eq!(gemini_busy.status.source, super::StatusSource::TmuxTitle);
    assert_eq!(gemini_busy.display.label, "Working");
    assert_eq!(gemini_busy.display.activity_label, None);

    let gemini_task = pane_by_id(panes, "%307");
    assert_eq!(gemini_task.provider, Some(Provider::Gemini));
    assert_eq!(gemini_task.status.kind, StatusKind::Unknown);
    assert_eq!(gemini_task.status.source, super::StatusSource::NotChecked);
    assert_eq!(gemini_task.display.label, "Plan snapshot cache migration");
    assert_eq!(
        gemini_task.display.activity_label.as_deref(),
        Some("Plan snapshot cache migration")
    );
}

fn assert_fixture_opencode_case(panes: &[PaneRecord]) {
    let opencode = pane_by_id(panes, "%301");
    assert_eq!(opencode.provider, Some(Provider::Opencode));
    assert_eq!(opencode.status.kind, StatusKind::Unknown);
    assert_eq!(opencode.status.source, super::StatusSource::NotChecked);
    assert_eq!(opencode.display.label, "Query planner");
    assert_eq!(opencode.display.activity_label, None);

    let opencode_busy = pane_by_id(panes, "%308");
    assert_eq!(opencode_busy.provider, Some(Provider::Opencode));
    assert_eq!(opencode_busy.status.kind, StatusKind::Unknown);
    assert_eq!(opencode_busy.status.source, super::StatusSource::NotChecked);
    assert_eq!(opencode_busy.display.label, "Working");
    assert_eq!(opencode_busy.display.activity_label, None);

    let opencode_idle = pane_by_id(panes, "%309");
    assert_eq!(opencode_idle.provider, Some(Provider::Opencode));
    assert_eq!(opencode_idle.status.kind, StatusKind::Unknown);
    assert_eq!(opencode_idle.status.source, super::StatusSource::NotChecked);
    assert_eq!(opencode_idle.display.label, "Ready");
    assert_eq!(opencode_idle.display.activity_label, None);

    let opencode_default = pane_by_id(panes, "%314");
    assert_eq!(opencode_default.provider, Some(Provider::Opencode));
    assert_eq!(opencode_default.status.kind, StatusKind::Unknown);
    assert_eq!(
        opencode_default.status.source,
        super::StatusSource::NotChecked
    );
    assert_eq!(opencode_default.display.label, "OpenCode");
    assert_eq!(opencode_default.display.activity_label, None);
}

fn assert_fixture_copilot_case(panes: &[PaneRecord]) {
    let copilot = pane_by_id(panes, "%302");
    assert_eq!(copilot.provider, Some(Provider::Copilot));
    assert_eq!(copilot.status.kind, StatusKind::Busy);
    assert_eq!(copilot.status.source, super::StatusSource::TmuxTitle);
    assert_eq!(copilot.display.label, "Working");
    assert_eq!(copilot.display.activity_label, None);

    let copilot_idle = pane_by_id(panes, "%310");
    assert_eq!(copilot_idle.provider, Some(Provider::Copilot));
    assert_eq!(copilot_idle.status.kind, StatusKind::Idle);
    assert_eq!(copilot_idle.status.source, super::StatusSource::TmuxTitle);
    assert_eq!(copilot_idle.display.label, "Ready");
    assert_eq!(copilot_idle.display.activity_label, None);
    assert_eq!(
        copilot_idle.classification.matched_by,
        Some(super::ClassificationMatchKind::PaneTitle)
    );

    let copilot_task = pane_by_id(panes, "%311");
    assert_eq!(copilot_task.provider, Some(Provider::Copilot));
    assert_eq!(copilot_task.status.kind, StatusKind::Unknown);
    assert_eq!(copilot_task.status.source, super::StatusSource::NotChecked);
    assert_eq!(copilot_task.display.label, "Review patch");
    assert_eq!(
        copilot_task.display.activity_label.as_deref(),
        Some("Review patch")
    );
}

fn assert_fixture_cursor_cli_title_case(panes: &[PaneRecord]) {
    let cursor = pane_by_id(panes, "%303");
    assert_eq!(cursor.provider, Some(Provider::CursorCli));
    assert_eq!(cursor.display.label, "Query planner");
    assert_eq!(cursor.status.kind, StatusKind::Unknown);
    assert_eq!(cursor.status.source, super::StatusSource::NotChecked);
    assert_eq!(
        cursor.display.activity_label.as_deref(),
        Some("Query planner")
    );
}

fn assert_fixture_cursor_cli_command_case(panes: &[PaneRecord]) {
    let cursor = pane_by_id(panes, "%305");
    assert_eq!(cursor.provider, Some(Provider::CursorCli));
    assert_eq!(cursor.display.label, "cursor-agent");
    assert_eq!(cursor.status.kind, StatusKind::Unknown);
    assert_eq!(cursor.display.activity_label, None);
    assert_eq!(
        cursor.classification.matched_by,
        Some(super::ClassificationMatchKind::PaneCurrentCommand)
    );
}

fn assert_fixture_pi_case(panes: &[PaneRecord]) {
    let pi = pane_by_id(panes, "%304");
    assert_eq!(pi.provider, Some(Provider::Pi));
    assert_eq!(pi.display.label, "refactor - pi_proj");
    assert_eq!(pi.status.kind, StatusKind::Unknown);
    assert_eq!(pi.status.source, super::StatusSource::NotChecked);
    assert_eq!(pi.display.activity_label, None);

    let pi_busy = pane_by_id(panes, "%312");
    assert_eq!(pi_busy.provider, Some(Provider::Pi));
    assert_eq!(pi_busy.status.kind, StatusKind::Busy);
    assert_eq!(pi_busy.status.source, super::StatusSource::TmuxTitle);
    assert_eq!(pi_busy.display.label, "refactor - pi_proj");
    assert_eq!(pi_busy.display.activity_label, None);

    let pi_command = pane_by_id(panes, "%313");
    assert_eq!(pi_command.provider, Some(Provider::Pi));
    assert_eq!(pi_command.status.kind, StatusKind::Unknown);
    assert_eq!(pi_command.status.source, super::StatusSource::NotChecked);
    assert_eq!(pi_command.display.label, "ship cache docs - followup");
    assert_eq!(
        pi_command.display.activity_label.as_deref(),
        Some("ship cache docs - followup")
    );
    assert_eq!(
        pi_command.classification.matched_by,
        Some(super::ClassificationMatchKind::PaneCurrentCommand)
    );
}

fn safe_tmux_field() -> impl Strategy<Value = String> {
    string_regex(r"[A-Za-z0-9_./()|: -]{0,32}").expect("safe tmux field regex should compile")
}

fn known_status_glyph() -> impl Strategy<Value = char> {
    prop::sample::select(
        CLAUDE_SPINNER_GLYPHS
            .iter()
            .copied()
            .chain(IDLE_GLYPHS.iter().copied())
            .collect::<Vec<_>>(),
    )
}

fn pane_by_id<'a>(panes: &'a [PaneRecord], pane_id: &str) -> &'a PaneRecord {
    panes
        .iter()
        .find(|pane| pane.pane_id == pane_id)
        .unwrap_or_else(|| panic!("missing pane fixture entry {pane_id}"))
}

fn assert_unresolved_ambiguous_pane(pane: &PaneRecord, expected_label: &str) {
    assert_eq!(pane.provider, None, "pane_id: {}", pane.pane_id);
    assert_eq!(
        pane.status.kind,
        StatusKind::Unknown,
        "pane_id: {}",
        pane.pane_id
    );
    assert_eq!(
        pane.status.source,
        super::StatusSource::NotChecked,
        "pane_id: {}",
        pane.pane_id
    );
    assert_eq!(
        pane.classification.matched_by, None,
        "pane_id: {}",
        pane.pane_id
    );
    assert_eq!(
        pane.classification.confidence, None,
        "pane_id: {}",
        pane.pane_id
    );
    assert!(
        pane.classification.reasons.is_empty(),
        "pane_id: {}",
        pane.pane_id
    );
    assert_eq!(
        pane.display.label, expected_label,
        "pane_id: {}",
        pane.pane_id
    );
    assert_eq!(
        pane.display.activity_label, None,
        "pane_id: {}",
        pane.pane_id
    );
}
