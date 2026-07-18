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
    let busy_index = gemini_current_busy_marker_index(&frame);

    // A dismissed approval modal stays visible higher on the screen after the
    // turn moves on, so a busy marker only counts when it is anchored to the
    // current bottom frame (same 14-row window as the auth prompt below).
    if let Some(index) = busy_index
        && frame.is_within_tail(index, 14)
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

fn gemini_current_busy_marker_index(frame: &PaneOutputFrame<'_>) -> Option<usize> {
    let mut marker_index = frame.rposition(gemini_current_busy_marker_line);
    while let Some(index) = marker_index {
        let line = frame.line(index)?;
        if gemini_running_agent_marker_line(line)
            || gemini_approval_marker_has_current_chrome(frame, index)
        {
            return Some(index);
        }
        marker_index = frame.rposition_before(index, gemini_current_busy_marker_line);
    }
    None
}

fn gemini_current_busy_marker_line(line: &str) -> bool {
    let line = line.trim();
    line.contains(ACTION_REQUIRED_MARKER)
        || line.contains(APPLY_CHANGE_MARKER)
        || line.contains(ALLOW_EXECUTION_MARKER)
        || gemini_running_agent_marker_line(line)
}

fn gemini_running_agent_marker_line(line: &str) -> bool {
    line.contains(RUNNING_AGENT_MARKER) && line.contains(COLLAPSE_HINT)
}

fn gemini_approval_marker_has_current_chrome(
    frame: &PaneOutputFrame<'_>,
    marker_index: usize,
) -> bool {
    let chrome_start = marker_index.saturating_sub(3);
    let chrome_len = marker_index - chrome_start + 4;
    let has_nearby_box = frame.lines_from(chrome_start).is_some_and(|lines| {
        lines
            .iter()
            .take(chrome_len)
            .any(|line| gemini_modal_box_line(line))
    });
    let glyph_start = marker_index.saturating_sub(1);
    let glyph_len = marker_index - glyph_start + 2;
    let has_adjacent_status_glyph = frame.lines_from(glyph_start).is_some_and(|lines| {
        lines
            .iter()
            .take(glyph_len)
            .any(|line| line.contains('✋') || line.contains('⏲'))
    });

    has_nearby_box || has_adjacent_status_glyph
}

fn gemini_modal_box_line(line: &str) -> bool {
    let line = line.trim();
    (line.starts_with('│') && line.ends_with('│'))
        || ((line.starts_with('╭') || line.starts_with('╰')) && line.chars().any(|ch| ch == '─'))
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
