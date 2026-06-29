use super::{PaneOutputFrame, StatusKind};

pub(super) fn status(output: &str) -> Option<StatusKind> {
    let frame = PaneOutputFrame::new(output);
    let idle_index = frame.rposition(gemini_idle_input_prompt_line);
    let busy_index = frame.rposition(gemini_current_busy_marker_line);

    if let Some(index) = busy_index
        && idle_index.is_none_or(|idle_index| idle_index < index)
    {
        return Some(StatusKind::Busy);
    }

    if let Some(index) = gemini_current_auth_prompt_index(&frame)
        && idle_index.is_none_or(|idle_index| idle_index < index)
    {
        return Some(StatusKind::Busy);
    }

    idle_index
        .is_some_and(|index| gemini_prompt_is_near_current_footer(&frame, index))
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

fn gemini_current_auth_prompt_index(frame: &PaneOutputFrame<'_>) -> Option<usize> {
    let index =
        frame.rposition(|line| line.contains("Opening authentication page in your browser"))?;

    if !frame.is_within_tail(index, 14) {
        return None;
    }

    let lines = frame.lines_from(index)?;
    lines
        .iter()
        .any(|line| line.contains("Do you want to continue?"))
        .then_some(())?;
    lines
        .iter()
        .any(|line| line.contains("Enter to select") || line.contains("to navigate"))
        .then_some(index)
}

fn gemini_prompt_is_near_current_footer(frame: &PaneOutputFrame<'_>, prompt_index: usize) -> bool {
    frame.is_within_tail(prompt_index, 8)
}
