use super::{PaneOutputFrame, StatusKind};

pub(super) fn status(output: &str) -> Option<StatusKind> {
    let frame = PaneOutputFrame::new(output);
    let prompt_index = frame.rposition(claude_prompt_line);
    let busy_index = frame.rposition(claude_current_busy_marker_line);

    if let Some(index) = busy_index
        && prompt_index.is_some_and(|prompt_index| {
            claude_busy_marker_is_near_current_prompt(&frame, index, prompt_index)
        })
    {
        return Some(StatusKind::Busy);
    }

    prompt_index
        .is_some_and(|index| claude_prompt_is_near_current_footer(&frame, index))
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
    frame: &PaneOutputFrame<'_>,
    busy_index: usize,
    prompt_index: usize,
) -> bool {
    frame.gap_between_is_within(busy_index, prompt_index, 6, claude_status_gap_line)
        && claude_prompt_is_near_current_footer(frame, prompt_index)
}

fn claude_status_gap_line(line: &str) -> bool {
    let line = line.trim();
    line.is_empty() || claude_prompt_border_line(line)
}

fn claude_prompt_is_near_current_footer(frame: &PaneOutputFrame<'_>, prompt_index: usize) -> bool {
    frame.tail_contains(prompt_index, 8, claude_current_footer_line)
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
