use super::*;

pub(crate) fn pane_output_status_fallback_candidate(pane: &PaneRecord) -> bool {
    matches!(
        pane.provider,
        Some(Provider::Codex)
            | Some(Provider::Claude)
            | Some(Provider::Copilot)
            | Some(Provider::CursorCli)
            | Some(Provider::Gemini)
            | Some(Provider::Grok)
            | Some(Provider::Hermes)
            | Some(Provider::Opencode)
            | Some(Provider::Pi)
    ) && pane.status.kind == StatusKind::Unknown
        && pane.status.source == StatusSource::NotChecked
}

pub(crate) fn apply_pane_output_status_fallback(pane: &mut PaneRecord, output: &str) {
    if !pane_output_status_fallback_candidate(pane) {
        return;
    }

    let status = match pane.provider {
        Some(Provider::Codex) => codex_pane_output_status(output),
        Some(Provider::Claude) => claude_pane_output_status(output),
        Some(Provider::Copilot) => copilot_pane_output_status(output),
        Some(Provider::CursorCli) => cursor_cli_pane_output_status(output),
        Some(Provider::Gemini) => gemini_pane_output_status(output),
        Some(Provider::Grok) => grok_pane_output_status(output),
        Some(Provider::Hermes) => hermes_pane_output_status(output),
        Some(Provider::Opencode) => opencode_pane_output_status(output),
        Some(Provider::Pi) => pi_pane_output_status(output),
        _ => None,
    };

    if let Some(kind) = status {
        pane.status = PaneStatus::pane_output(kind);
    }
}

fn claude_pane_output_status(output: &str) -> Option<StatusKind> {
    let lines: Vec<&str> = output.lines().collect();
    let prompt_index = lines.iter().rposition(|line| claude_prompt_line(line));
    let busy_index = lines
        .iter()
        .rposition(|line| claude_current_busy_marker_line(line));

    if let Some(index) = busy_index
        && prompt_index.is_some_and(|prompt_index| {
            claude_busy_marker_is_near_current_prompt(&lines, index, prompt_index)
        })
    {
        return Some(StatusKind::Busy);
    }

    prompt_index
        .is_some_and(|index| claude_prompt_is_near_current_footer(&lines, index))
        .then_some(StatusKind::Idle)
}

fn claude_prompt_line(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with('❯')
}

fn claude_current_busy_marker_line(line: &str) -> bool {
    let line = line.trim();
    claude_interrupt_hint_line(line) || claude_waiting_permission_line(line)
}

fn claude_interrupt_hint_line(line: &str) -> bool {
    line.contains("esc") && line.contains("interrupt")
}

fn claude_waiting_permission_line(line: &str) -> bool {
    line.contains("Waiting for permission")
}

fn claude_busy_marker_is_near_current_prompt(
    lines: &[&str],
    busy_index: usize,
    prompt_index: usize,
) -> bool {
    let distance = prompt_index.abs_diff(busy_index);
    distance <= 6
        && claude_lines_between_are_status_gap(lines, busy_index, prompt_index)
        && claude_prompt_is_near_current_footer(lines, prompt_index)
}

fn claude_lines_between_are_status_gap(lines: &[&str], first: usize, second: usize) -> bool {
    let start = first.min(second) + 1;
    let end = first.max(second);
    lines[start..end]
        .iter()
        .all(|line| claude_status_gap_line(line))
}

fn claude_status_gap_line(line: &str) -> bool {
    let line = line.trim();
    line.is_empty() || claude_prompt_border_line(line)
}

fn claude_prompt_is_near_current_footer(lines: &[&str], prompt_index: usize) -> bool {
    let tail_len = lines.len().saturating_sub(prompt_index);
    tail_len <= 8
        && lines[prompt_index..]
            .iter()
            .any(|line| claude_current_footer_line(line))
}

fn claude_current_footer_line(line: &str) -> bool {
    let line = line.trim();
    line.contains("? for shortcuts")
        || line.contains("shift+tab") && line.contains("cycle")
        || line.contains("auto on")
        || line.contains("plan on")
        || line.contains("accept edits on")
        || line.contains("bypass permissions on")
        || line.contains("ultraplan on")
        || claude_interrupt_hint_line(line)
}

fn claude_prompt_border_line(line: &str) -> bool {
    let line = line.trim();
    line.chars().count() >= 8
        && line
            .chars()
            .all(|ch| matches!(ch, '─' | '╭' | '╮' | '╰' | '╯'))
}

fn codex_pane_output_status(output: &str) -> Option<StatusKind> {
    let lines: Vec<&str> = output.lines().collect();
    let idle_index = lines.iter().rposition(|line| codex_idle_prompt_line(line));
    let busy_index = lines
        .iter()
        .rposition(|line| codex_current_busy_marker_line(line));

    if let Some(index) = busy_index
        && idle_index.is_none_or(|idle_index| {
            idle_index < index
                || codex_busy_marker_is_near_current_prompt(&lines, index, idle_index)
        })
    {
        return Some(StatusKind::Busy);
    }

    idle_index
        .is_some_and(|index| codex_prompt_is_near_current_footer(&lines, index))
        .then_some(StatusKind::Idle)
}

fn codex_idle_prompt_line(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with('›') && line.contains("Ask Codex to do anything")
}

fn codex_current_busy_marker_line(line: &str) -> bool {
    let line = line.trim();
    codex_interrupt_status_line(line) || codex_approval_prompt_line(line)
}

fn codex_interrupt_status_line(line: &str) -> bool {
    line.contains("esc to interrupt)") && line.contains('(')
}

fn codex_approval_prompt_line(line: &str) -> bool {
    line.contains("Press enter to confirm or esc to cancel")
        || line.contains("Yes, proceed")
        || line.contains("Reviewing ") && line.contains("approval request")
}

fn codex_busy_marker_is_near_current_prompt(
    lines: &[&str],
    busy_index: usize,
    idle_index: usize,
) -> bool {
    idle_index.saturating_sub(busy_index) <= 4
        && lines[busy_index + 1..idle_index]
            .iter()
            .all(|line| codex_status_gap_line(line))
        && codex_prompt_is_near_current_footer(lines, idle_index)
}

fn codex_status_gap_line(line: &str) -> bool {
    let line = line.trim();
    line.is_empty() || line.starts_with('└')
}

fn codex_prompt_is_near_current_footer(lines: &[&str], prompt_index: usize) -> bool {
    let tail_len = lines.len().saturating_sub(prompt_index);
    tail_len <= 6
        && lines[prompt_index..]
            .iter()
            .any(|line| codex_footer_line(line))
}

fn codex_footer_line(line: &str) -> bool {
    let line = line.trim();
    line.contains("context left")
        || (line.contains("Context ") && line.contains(" used"))
        || line.contains("Fast on")
        || line.contains("tab to queue message")
        || codex_model_path_footer_line(line)
}

fn codex_model_path_footer_line(line: &str) -> bool {
    line.contains(" · ")
        && codex_model_footer_token(line)
        && (codex_footer_has_path_context(line) || codex_footer_has_mode_context(line))
}

fn codex_model_footer_token(line: &str) -> bool {
    line.split_whitespace()
        .any(|token| token.starts_with("gpt-") || token.starts_with("o"))
}

fn codex_footer_has_path_context(line: &str) -> bool {
    line.split(" · ").any(|part| {
        let part = part.trim();
        part.starts_with('/')
            || part.starts_with("~/")
            || part.starts_with("./")
            || part.contains("/Users/")
            || part.contains("/tmp/")
    })
}

fn codex_footer_has_mode_context(line: &str) -> bool {
    line.contains("Plan mode")
        || line.contains("Default mode")
        || line.contains("Shell mode")
        || line.contains("Side from ")
        || line.contains("Goal ")
}

fn copilot_pane_output_indicates_busy(output: &str) -> bool {
    copilot_current_status_line(output).is_some_and(|line| line.contains("Thinking (Esc to cancel"))
        || copilot_current_trust_prompt_visible(output)
}

fn copilot_pane_output_status(output: &str) -> Option<StatusKind> {
    if copilot_pane_output_indicates_busy(output) {
        return Some(StatusKind::Busy);
    }

    copilot_current_prompt_visible(output).then_some(StatusKind::Idle)
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
    (line.starts_with('/') || line.starts_with("~/")) && !line.starts_with("/ commands")
}

fn copilot_current_prompt_visible(output: &str) -> bool {
    let lines: Vec<&str> = output.lines().collect();
    let Some(prompt_index) = lines.iter().rposition(|line| line.trim() == "❯") else {
        return false;
    };
    let Some(context_index) = lines[..prompt_index]
        .iter()
        .rposition(|line| copilot_prompt_context_line(line))
    else {
        return false;
    };

    lines[prompt_index..]
        .iter()
        .any(|line| line.contains("/ commands") && line.contains("? help"))
        && lines[context_index..prompt_index]
            .iter()
            .any(|line| copilot_separator_line(line))
}

fn copilot_separator_line(line: &str) -> bool {
    let line = line.trim();
    line.len() >= 8 && line.chars().all(|ch| ch == '─')
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

fn cursor_cli_pane_output_status(output: &str) -> Option<StatusKind> {
    let lines: Vec<&str> = output.lines().collect();
    let footer_top_index = lines
        .iter()
        .rposition(|line| cursor_cli_footer_top_border(line))?;

    let current_footer = lines[footer_top_index..]
        .iter()
        .map(|line| line.trim())
        .find(|line| line.starts_with('→'));

    if current_footer.is_some_and(|line| line.contains("ctrl+c to stop"))
        || cursor_cli_current_status_line(&lines, footer_top_index)
            .is_some_and(cursor_cli_status_line_indicates_running)
    {
        return Some(StatusKind::Busy);
    }

    current_footer
        .is_some_and(cursor_cli_footer_indicates_idle)
        .then_some(StatusKind::Idle)
}

fn cursor_cli_current_status_line<'a>(
    lines: &'a [&'a str],
    footer_top_index: usize,
) -> Option<&'a str> {
    lines[..footer_top_index]
        .iter()
        .rev()
        .map(|line| line.trim())
        .find(|line| !line.is_empty())
}

fn cursor_cli_status_line_indicates_running(line: &str) -> bool {
    let mut parts = line.split_whitespace();
    let Some(spinner) = parts.next() else {
        return false;
    };
    spinner
        .chars()
        .all(|ch| ('\u{2800}'..='\u{28ff}').contains(&ch))
        && parts.any(|part| part == "Running")
}

fn cursor_cli_footer_indicates_idle(line: &str) -> bool {
    let line = line.trim_start_matches('→').trim();
    line == "Add a follow-up" || line == "Plan, search, build anything"
}

fn cursor_cli_footer_top_border(line: &str) -> bool {
    line.trim_start().starts_with("▄▄▄▄▄▄")
}

fn gemini_pane_output_status(output: &str) -> Option<StatusKind> {
    let lines: Vec<&str> = output.lines().collect();
    let idle_index = lines
        .iter()
        .rposition(|line| gemini_idle_input_prompt_line(line));
    let busy_index = lines
        .iter()
        .rposition(|line| gemini_current_busy_marker_line(line));

    if let Some(index) = busy_index
        && idle_index.is_none_or(|idle_index| idle_index < index)
    {
        return Some(StatusKind::Busy);
    }

    idle_index
        .is_some_and(|index| gemini_prompt_is_near_current_footer(&lines, index))
        .then_some(StatusKind::Idle)
}

fn gemini_idle_input_prompt_line(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with('>') && line.contains("Type your message")
}

fn gemini_current_busy_marker_line(line: &str) -> bool {
    let line = line.trim();
    line.contains("Action Required")
        || line.contains("Apply this change?")
        || line.contains("Allow execution of")
        || (line.contains("Running Agent") && line.contains("ctrl+o to collapse"))
}

fn gemini_prompt_is_near_current_footer(lines: &[&str], prompt_index: usize) -> bool {
    let tail_len = lines.len().saturating_sub(prompt_index);
    tail_len <= 8
}

fn grok_pane_output_status(output: &str) -> Option<StatusKind> {
    let lines: Vec<&str> = output.lines().collect();
    if let Some(footer) = lines
        .iter()
        .rev()
        .map(|line| line.trim())
        .find(|line| grok_keybind_footer_line(line))
    {
        return grok_footer_indicates_idle(footer).then_some(StatusKind::Idle);
    }

    let completed_index = lines
        .iter()
        .rposition(|line| line.trim_start().starts_with("Turn completed in "));
    let running_index = lines
        .iter()
        .rposition(|line| grok_running_status_line(line));

    completed_index
        .is_some_and(|index| running_index.is_none_or(|running| running < index))
        .then_some(StatusKind::Idle)
}

fn grok_keybind_footer_line(line: &str) -> bool {
    line.contains("Shift+Tab:mode") && line.contains("Ctrl+.:shortcuts")
}

fn grok_footer_indicates_idle(line: &str) -> bool {
    !grok_footer_has_working_binds(line) && !grok_footer_has_approval_text(line)
}

fn grok_footer_has_working_binds(line: &str) -> bool {
    line.contains("Ctrl+c:cancel") || line.contains("Ctrl+Enter:interject")
}

fn grok_footer_has_approval_text(line: &str) -> bool {
    let line = line.to_ascii_lowercase();
    ["approve", "reject", "allow", "deny", "confirm"]
        .iter()
        .any(|word| line.contains(word))
}

fn grok_running_status_line(line: &str) -> bool {
    let line = line.trim();
    line.chars()
        .next()
        .is_some_and(|ch| ('\u{2800}'..='\u{28ff}').contains(&ch))
        && (line.contains("Running:")
            || (line.contains(" … ⇣")
                && line.contains("[✗]")
                && line.split_whitespace().any(grok_elapsed_token)))
}

fn grok_elapsed_token(token: &str) -> bool {
    let Some(value) = token.strip_suffix('s') else {
        return false;
    };

    !value.is_empty()
        && value.chars().all(|ch| ch.is_ascii_digit() || ch == '.')
        && value.chars().any(|ch| ch.is_ascii_digit())
}

fn hermes_pane_output_status(output: &str) -> Option<StatusKind> {
    let lines: Vec<&str> = output.lines().collect();
    let busy_index = lines.iter().rposition(|line| hermes_busy_prompt_line(line));
    let idle_index = lines.iter().rposition(|line| line.trim() == "❯");

    if let Some(index) = busy_index
        && hermes_status_bar_before(&lines, index).is_some()
        && idle_index.is_none_or(|idle_index| idle_index < index)
    {
        return Some(StatusKind::Busy);
    }

    if let Some(index) = idle_index
        && hermes_status_bar_before(&lines, index).is_some()
        && busy_index.is_none_or(|busy_index| busy_index < index)
    {
        return Some(StatusKind::Idle);
    }

    None
}

fn hermes_status_bar_before<'a>(lines: &'a [&'a str], index: usize) -> Option<&'a str> {
    lines[..index]
        .iter()
        .rev()
        .map(|line| line.trim())
        .find(|line| hermes_status_bar_line(line))
}

fn hermes_status_bar_line(line: &str) -> bool {
    line.starts_with("⚕ ") && line.contains('│') && (line.contains("ctx") || line.contains("K/"))
}

fn hermes_busy_prompt_line(line: &str) -> bool {
    let line = line.trim();
    line.starts_with("⚕ ❯")
        && line.contains("msg=interrupt")
        && line.contains("/queue")
        && line.contains("Ctrl+C cancel")
}

fn opencode_pane_output_status(output: &str) -> Option<StatusKind> {
    let lines: Vec<&str> = output.lines().collect();
    let idle_index = lines
        .iter()
        .rposition(|line| opencode_idle_prompt_line(line));
    let busy_index = lines
        .iter()
        .rposition(|line| opencode_current_busy_marker_line(line));

    if let Some(index) = busy_index
        && idle_index.is_none_or(|idle_index| idle_index < index)
    {
        return Some(StatusKind::Busy);
    }

    idle_index
        .is_some_and(|index| opencode_prompt_is_near_current_footer(&lines, index))
        .then_some(StatusKind::Idle)
}

fn opencode_idle_prompt_line(line: &str) -> bool {
    let line = line.trim_start();
    line.contains("Ask anything... \"") || line.contains("Run a command... \"")
}

fn opencode_current_busy_marker_line(line: &str) -> bool {
    let line = line.trim();
    opencode_interrupt_hint_line(line)
        || opencode_permission_prompt_line(line)
        || opencode_question_prompt_line(line)
}

fn opencode_interrupt_hint_line(line: &str) -> bool {
    line.contains("esc") && line.contains("interrupt")
}

fn opencode_permission_prompt_line(line: &str) -> bool {
    line.contains("Permission required")
        || line.contains("Reject permission")
        || line.contains("Allow once")
        || line.contains("Allow always")
        || (line.contains('△') && line.contains("Permission"))
}

fn opencode_question_prompt_line(line: &str) -> bool {
    line.contains("Reject question")
        || line.contains("Waiting for question event")
        || line.contains("# Questions")
}

fn opencode_prompt_is_near_current_footer(lines: &[&str], prompt_index: usize) -> bool {
    let tail_len = lines.len().saturating_sub(prompt_index);
    tail_len <= 8
}

fn pi_pane_output_status(output: &str) -> Option<StatusKind> {
    let lines: Vec<&str> = output.lines().collect();
    let idle_index = lines.iter().rposition(|line| pi_editor_border_line(line));
    let busy_index = lines
        .iter()
        .rposition(|line| pi_current_busy_marker_line(line));

    if let Some(index) = busy_index
        && idle_index.is_none_or(|idle_index| {
            idle_index < index || pi_busy_marker_is_near_current_editor(&lines, index, idle_index)
        })
    {
        return Some(StatusKind::Busy);
    }

    idle_index
        .is_some_and(|index| pi_editor_frame_is_near_current_footer(&lines, index))
        .then_some(StatusKind::Idle)
}

fn pi_current_busy_marker_line(line: &str) -> bool {
    let line = line.trim();
    pi_working_loader_line(line)
        || pi_retry_loader_line(line)
        || pi_compaction_loader_line(line)
        || pi_running_bash_line(line)
}

fn pi_busy_marker_is_near_current_editor(
    lines: &[&str],
    busy_index: usize,
    idle_index: usize,
) -> bool {
    idle_index.saturating_sub(busy_index) <= 4
        && lines[busy_index + 1..idle_index]
            .iter()
            .all(|line| pi_editor_gap_line(line))
        && pi_editor_frame_is_near_current_footer(lines, idle_index)
}

fn pi_editor_gap_line(line: &str) -> bool {
    let line = line.trim();
    line.is_empty() || pi_editor_border_line(line)
}

fn pi_working_loader_line(line: &str) -> bool {
    line.contains("Working...") || line.contains(" to interrupt)")
}

fn pi_retry_loader_line(line: &str) -> bool {
    line.contains("Retrying (") && line.contains(" to cancel)")
}

fn pi_compaction_loader_line(line: &str) -> bool {
    (line.contains("Compacting context...") || line.contains("Auto-compacting..."))
        && line.contains(" to cancel)")
}

fn pi_running_bash_line(line: &str) -> bool {
    line.contains("Running...") && line.contains(" to cancel)")
}

fn pi_editor_border_line(line: &str) -> bool {
    let line = line.trim();
    line.chars().count() >= 8 && line.chars().all(|ch| ch == '─')
}

fn pi_editor_frame_is_near_current_footer(lines: &[&str], border_index: usize) -> bool {
    let tail_len = lines.len().saturating_sub(border_index);
    tail_len <= 6
        && lines[border_index..]
            .iter()
            .any(|line| pi_footer_context_line(line))
}

fn pi_footer_context_line(line: &str) -> bool {
    let line = line.trim();
    pi_footer_context_token(line)
        && !pi_current_busy_marker_line(line)
        && !line.contains("Error:")
        && !line.contains("Warning:")
}

fn pi_footer_context_token(line: &str) -> bool {
    line.split_whitespace().any(|token| {
        token.contains("%/")
            || (token.starts_with("?/") && token.chars().skip(2).any(|ch| ch.is_ascii_digit()))
    })
}
