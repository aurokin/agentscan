use super::*;

pub(crate) fn pane_output_status_fallback_candidate(pane: &PaneRecord) -> bool {
    matches!(
        pane.provider,
        Some(Provider::Copilot)
            | Some(Provider::CursorCli)
            | Some(Provider::Gemini)
            | Some(Provider::Grok)
            | Some(Provider::Hermes)
            | Some(Provider::Opencode)
    ) && pane.status.kind == StatusKind::Unknown
        && pane.status.source == StatusSource::NotChecked
}

pub(crate) fn apply_pane_output_status_fallback(pane: &mut PaneRecord, output: &str) {
    if !pane_output_status_fallback_candidate(pane) {
        return;
    }

    let status = match pane.provider {
        Some(Provider::Copilot) => copilot_pane_output_status(output),
        Some(Provider::CursorCli) => cursor_cli_pane_output_status(output),
        Some(Provider::Gemini) => gemini_pane_output_status(output),
        Some(Provider::Grok) => grok_pane_output_status(output),
        Some(Provider::Hermes) => hermes_pane_output_status(output),
        Some(Provider::Opencode) => opencode_pane_output_status(output),
        _ => None,
    };

    if let Some(kind) = status {
        pane.status = PaneStatus::pane_output(kind);
    }
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
