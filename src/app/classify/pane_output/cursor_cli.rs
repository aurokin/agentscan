use super::{PaneOutputFrame, StatusKind};

pub(super) fn status(output: &str) -> Option<StatusKind> {
    let frame = PaneOutputFrame::new(output);
    let Some(footer_top_index) = frame.rposition(cursor_cli_footer_top_border) else {
        return cursor_cli_borderless_prompt_status(&frame);
    };

    let current_footer = frame
        .lines_from(footer_top_index)?
        .iter()
        .map(|line| line.trim())
        .find(|line| line.starts_with('→'));

    if current_footer.is_some_and(|line| line.contains("ctrl+c to stop"))
        || cursor_cli_current_status_line(&frame, footer_top_index)
            .is_some_and(cursor_cli_status_line_indicates_running)
    {
        return Some(StatusKind::Busy);
    }

    current_footer
        .is_some_and(cursor_cli_footer_indicates_idle)
        .then_some(StatusKind::Idle)
}

fn cursor_cli_borderless_prompt_status(frame: &PaneOutputFrame<'_>) -> Option<StatusKind> {
    let prompt_index = frame.rposition(cursor_cli_borderless_prompt_line)?;
    let prompt = frame.line(prompt_index)?.trim();
    if cursor_cli_borderless_prompt_indicates_busy(prompt) {
        return cursor_cli_borderless_prompt_has_current_chrome(frame, prompt_index, true)
            .then_some(StatusKind::Busy);
    }
    // `cursor_cli_footer_indicates_idle` strips the leading arrow, so the same predicate
    // covers bordered footer rows and borderless prompt rows.
    if !cursor_cli_footer_indicates_idle(prompt) {
        return None;
    }

    cursor_cli_borderless_prompt_has_current_chrome(frame, prompt_index, false)
        .then_some(StatusKind::Idle)
}

fn cursor_cli_borderless_prompt_has_current_chrome(
    frame: &PaneOutputFrame<'_>,
    prompt_index: usize,
    allow_task_count: bool,
) -> bool {
    let Some(lines_after) = frame.lines_from(prompt_index) else {
        return false;
    };
    let has_composer_footer = lines_after
        .iter()
        .any(|line| cursor_cli_composer_footer_line(line.trim()));
    let only_cursor_chrome_after = frame.trailing_lines_after_are(prompt_index, |_, line, _| {
        let line = line.trim();
        line.is_empty()
            || cursor_cli_composer_footer_line(line)
            || cursor_cli_path_footer_line(line)
            || (allow_task_count && cursor_cli_task_count_line(line))
    });

    has_composer_footer && only_cursor_chrome_after
}

fn cursor_cli_current_status_line<'a>(
    frame: &'a PaneOutputFrame<'a>,
    footer_top_index: usize,
) -> Option<&'a str> {
    frame
        .previous_nonblank_before(footer_top_index)
        .map(str::trim)
}

fn cursor_cli_status_line_indicates_running(line: &str) -> bool {
    let mut parts = line.split_whitespace();
    let Some(spinner) = parts.next() else {
        return false;
    };
    spinner
        .chars()
        .all(|ch| ('\u{2800}'..='\u{28ff}').contains(&ch))
        && parts.any(|part| part == "Running")
}

fn cursor_cli_footer_indicates_idle(line: &str) -> bool {
    let line = line.trim_start_matches('→').trim();
    line == "Add a follow-up" || line == "Plan, search, build anything"
}

fn cursor_cli_borderless_prompt_line(line: &str) -> bool {
    line.trim_start().starts_with('→')
}

fn cursor_cli_borderless_prompt_indicates_busy(line: &str) -> bool {
    line.contains("ctrl+c to stop")
}

fn cursor_cli_composer_footer_line(line: &str) -> bool {
    line.contains("Composer") && (line.contains("Auto-run") || line.contains("Run Everything"))
}

fn cursor_cli_path_footer_line(line: &str) -> bool {
    (line.starts_with('/') || line.starts_with("~/")) && line.contains(" · ")
}

fn cursor_cli_task_count_line(line: &str) -> bool {
    let line = line.trim();
    let Some(count) = line
        .strip_suffix(" task")
        .or_else(|| line.strip_suffix(" tasks"))
    else {
        return false;
    };

    count.trim().parse::<u32>().is_ok()
}

fn cursor_cli_footer_top_border(line: &str) -> bool {
    line.trim_start().starts_with("▄▄▄▄▄▄")
}
