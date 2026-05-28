use super::StatusKind;

pub(super) fn status(output: &str) -> Option<StatusKind> {
    let lines: Vec<&str> = output.lines().collect();

    // Antigravity (closed-source) flips its own footer below the bordered `>` input box between
    // an idle prompt (`? for shortcuts`) and an active turn (`esc to cancel`, shown with a
    // `… Generating…`/`… Loading…` spinner above), so the footer is the current-frame busy/idle
    // anchor. Require the `>` box just above and only blank rows after it, so a stale scrollback
    // line carrying the phrase is not mistaken for the live footer; anything else stays unknown
    // rather than risk a guessed state.
    let footer_index = lines.iter().rposition(|line| {
        antigravity_idle_footer_line(line) || antigravity_busy_footer_line(line)
    })?;
    let only_blank_after = lines[footer_index + 1..]
        .iter()
        .all(|line| line.trim().is_empty());
    if !(only_blank_after && antigravity_prompt_above_footer(&lines, footer_index)) {
        return None;
    }

    Some(if antigravity_busy_footer_line(lines[footer_index]) {
        StatusKind::Busy
    } else {
        StatusKind::Idle
    })
}

fn antigravity_idle_footer_line(line: &str) -> bool {
    line.trim_start().starts_with("? for shortcuts")
}

fn antigravity_busy_footer_line(line: &str) -> bool {
    line.trim_start().starts_with("esc to cancel")
}

fn antigravity_prompt_above_footer(lines: &[&str], footer_index: usize) -> bool {
    let start = footer_index.saturating_sub(6);
    lines[start..footer_index].iter().any(|line| {
        let line = line.trim();
        line == ">" || line.starts_with("> ")
    })
}
