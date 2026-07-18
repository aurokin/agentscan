use super::{PaneOutputFrame, StatusKind};

// droid idle footer hint shown at the bottom of the input box.
const HELP_HINT: &str = "? for help";
// droid footer shown after an IDE/TMUX integration reload (`… ready … restart to apply`).
const READY_MARKER: &str = " ready ";
const RESTART_MARKER: &str = "restart to apply";
// droid busy prompt shown while a turn can be steered.
const STEER_PROMPT_MARKER: &str = "> Enter to steer";
const STEER_HINT: &str = "Enter to steer";
// droid streaming status stop hint (rendered after a leading spinner glyph).
const STOP_HINT: &str = "Press ESC to stop";

pub(super) fn status(output: &str) -> Option<StatusKind> {
    let frame = PaneOutputFrame::new(output);
    let footer_index = frame.rposition(droid_current_footer_line);
    let input_box_index = frame
        .rposition(droid_input_box_row)
        .filter(|&index| frame.is_within_tail(index, 8));
    let current_frame_index = footer_index.max(input_box_index)?;
    let current_prompt_lines = frame.window_ending_at(current_frame_index, 8)?;

    if current_prompt_lines
        .iter()
        .any(|line| droid_current_busy_prompt_line(line))
    {
        return Some(StatusKind::Busy);
    }

    if current_prompt_lines
        .iter()
        .any(|line| droid_current_idle_prompt_line(line))
    {
        return Some(StatusKind::Idle);
    }

    current_prompt_lines
        .iter()
        .any(|line| droid_current_streaming_line(line))
        .then_some(StatusKind::Busy)
}

fn droid_input_box_row(line: &str) -> bool {
    line.trim_start().starts_with("│ >")
}

fn droid_current_footer_line(line: &str) -> bool {
    let line = line.trim();
    (line.contains(HELP_HINT)
        && line
            .split_whitespace()
            .any(|token| matches!(token, "IDE" | "TMUX")))
        || (line.contains(READY_MARKER)
            && line.contains(RESTART_MARKER)
            && line.split_whitespace().any(|token| token == "TMUX"))
}

fn droid_current_busy_prompt_line(line: &str) -> bool {
    let line = line.trim();
    line.contains(STEER_PROMPT_MARKER)
}

fn droid_current_streaming_line(line: &str) -> bool {
    // droid's streaming status line is `<spinner> <verb>…  (Press ESC to stop)`, where the verb
    // varies across a turn (`Streaming…`, `Invoking tools…`, `Thinking…`). Anchor on the live
    // braille spinner glyph plus the verb-agnostic stop hint, so prose that merely contains
    // "Press ESC to stop" (without the leading spinner) is not mistaken for an active turn.
    let line = line.trim_start();
    line.chars()
        .next()
        .is_some_and(|ch| ('\u{2800}'..='\u{28ff}').contains(&ch))
        && line.contains(STOP_HINT)
}

fn droid_current_idle_prompt_line(line: &str) -> bool {
    let line = line.trim();
    droid_input_box_row(line) && !line.contains(STEER_HINT)
}
