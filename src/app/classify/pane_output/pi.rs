use super::{PaneOutputFrame, StatusKind};

pub(super) fn status(output: &str) -> Option<StatusKind> {
    let frame = PaneOutputFrame::new(output);
    let idle_index = frame.rposition(pi_editor_border_line);
    let busy_index = frame.rposition(pi_current_busy_marker_line);

    if let Some(index) = busy_index
        && idle_index.is_none_or(|idle_index| {
            idle_index < index || pi_busy_marker_is_near_current_editor(&frame, index, idle_index)
        })
    {
        return Some(StatusKind::Busy);
    }

    idle_index
        .is_some_and(|index| pi_editor_frame_is_near_current_footer(&frame, index))
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
    frame: &PaneOutputFrame<'_>,
    busy_index: usize,
    idle_index: usize,
) -> bool {
    frame.forward_gap_before_is_within(busy_index, idle_index, 4, pi_editor_gap_line)
        && pi_editor_frame_is_near_current_footer(frame, idle_index)
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

fn pi_editor_frame_is_near_current_footer(
    frame: &PaneOutputFrame<'_>,
    border_index: usize,
) -> bool {
    frame.tail_contains(border_index, 6, pi_footer_context_line)
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
