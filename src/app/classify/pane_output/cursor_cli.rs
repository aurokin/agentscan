use super::StatusKind;

pub(super) fn status(output: &str) -> Option<StatusKind> {
    let lines: Vec<&str> = output.lines().collect();
    let footer_top_index = lines
        .iter()
        .rposition(|line| cursor_cli_footer_top_border(line))?;

    let current_footer = lines[footer_top_index..]
        .iter()
        .map(|line| line.trim())
        .find(|line| line.starts_with('→'));

    if current_footer.is_some_and(|line| line.contains("ctrl+c to stop"))
        || cursor_cli_current_status_line(&lines, footer_top_index)
            .is_some_and(cursor_cli_status_line_indicates_running)
    {
        return Some(StatusKind::Busy);
    }

    current_footer
        .is_some_and(cursor_cli_footer_indicates_idle)
        .then_some(StatusKind::Idle)
}

fn cursor_cli_current_status_line<'a>(
    lines: &'a [&'a str],
    footer_top_index: usize,
) -> Option<&'a str> {
    lines[..footer_top_index]
        .iter()
        .rev()
        .map(|line| line.trim())
        .find(|line| !line.is_empty())
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

fn cursor_cli_footer_top_border(line: &str) -> bool {
    line.trim_start().starts_with("▄▄▄▄▄▄")
}
