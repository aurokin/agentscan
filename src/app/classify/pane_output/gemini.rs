use super::*;

pub(super) fn status(output: &str) -> Option<StatusKind> {
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
