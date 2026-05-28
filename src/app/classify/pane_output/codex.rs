use super::*;

pub(super) fn status(output: &str) -> Option<StatusKind> {
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
