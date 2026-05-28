use super::*;

pub(super) fn status(output: &str) -> Option<StatusKind> {
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
