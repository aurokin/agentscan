use super::{PaneOutputFrame, StatusKind};

pub(super) fn status(output: &str) -> Option<StatusKind> {
    let frame = PaneOutputFrame::new(output);
    let footer_index = frame.rposition(droid_current_footer_line)?;
    let current_prompt_lines = frame.window_ending_at(footer_index, 8)?;

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

fn droid_current_footer_line(line: &str) -> bool {
    let line = line.trim();
    line.contains("? for help") && line.contains("IDE")
}

fn droid_current_busy_prompt_line(line: &str) -> bool {
    let line = line.trim();
    line.contains("> Enter to steer")
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
        && line.contains("Press ESC to stop")
}

fn droid_current_idle_prompt_line(line: &str) -> bool {
    let line = line.trim();
    line.starts_with("│ >") && !line.contains("Enter to steer")
}
