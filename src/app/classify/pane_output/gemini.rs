use super::{PaneOutputFrame, StatusKind};

// Gemini idle input prompt placeholder.
const IDLE_PROMPT_MARKER: &str = "Type your message";
// Gemini busy/approval markers shown while a turn is running.
const ACTION_REQUIRED_MARKER: &str = "Action Required";
const APPLY_CHANGE_MARKER: &str = "Apply this change?";
const ALLOW_EXECUTION_MARKER: &str = "Allow execution of";
const RUNNING_AGENT_MARKER: &str = "Running Agent";
const COLLAPSE_HINT: &str = "ctrl+o to collapse";
// Gemini auth-flow prompt copy.
const AUTH_OPENING_MARKER: &str = "Opening authentication page in your browser";
const AUTH_CONTINUE_MARKER: &str = "Do you want to continue?";
const AUTH_SELECT_HINT: &str = "Enter to select";
const AUTH_NAVIGATE_HINT: &str = "to navigate";

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
    line.starts_with('>') && line.contains(IDLE_PROMPT_MARKER)
}

fn gemini_current_busy_marker_line(line: &str) -> bool {
    let line = line.trim();
    line.contains(ACTION_REQUIRED_MARKER)
        || line.contains(APPLY_CHANGE_MARKER)
        || line.contains(ALLOW_EXECUTION_MARKER)
        || (line.contains(RUNNING_AGENT_MARKER) && line.contains(COLLAPSE_HINT))
}

fn gemini_current_auth_prompt_index(frame: &PaneOutputFrame<'_>) -> Option<usize> {
    let index = frame.rposition(|line| line.contains(AUTH_OPENING_MARKER))?;

    if !frame.is_within_tail(index, 14) {
        return None;
    }

    let lines = frame.lines_from(index)?;
    lines
        .iter()
        .any(|line| line.contains(AUTH_CONTINUE_MARKER))
        .then_some(())?;
    lines
        .iter()
        .any(|line| line.contains(AUTH_SELECT_HINT) || line.contains(AUTH_NAVIGATE_HINT))
        .then_some(index)
}

fn gemini_prompt_is_near_current_footer(frame: &PaneOutputFrame<'_>, prompt_index: usize) -> bool {
    frame.is_within_tail(prompt_index, 8)
}
