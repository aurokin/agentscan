use super::{PaneOutputFrame, StatusKind};

pub(super) fn status(output: &str) -> Option<StatusKind> {
    let frame = PaneOutputFrame::new(output);

    // Antigravity (closed-source) flips its own footer below the bordered `>` input box between
    // an idle prompt (`? for shortcuts`) and an active turn (`esc to cancel`, shown with a
    // `… Generating…`/`… Loading…` spinner above), so the footer is the current-frame busy/idle
    // anchor. Require the `>` box just above and only blank rows after it, so a stale scrollback
    // line carrying the phrase is not mistaken for the live footer; anything else stays unknown
    // rather than risk a guessed state.
    let footer_index = frame.rposition(|line| {
        antigravity_idle_footer_line(line) || antigravity_busy_footer_line(line)
    })?;
    let only_blank_after =
        frame.trailing_lines_after_are(footer_index, |_, line, _| line.trim().is_empty());
    if !(only_blank_after && antigravity_prompt_above_footer(&frame, footer_index)) {
        return None;
    }

    Some(
        if frame
            .line(footer_index)
            .is_some_and(antigravity_busy_footer_line)
        {
            StatusKind::Busy
        } else {
            StatusKind::Idle
        },
    )
}

fn antigravity_idle_footer_line(line: &str) -> bool {
    line.trim_start().starts_with("? for shortcuts")
}

fn antigravity_busy_footer_line(line: &str) -> bool {
    line.trim_start().starts_with("esc to cancel")
}

fn antigravity_prompt_above_footer(frame: &PaneOutputFrame<'_>, footer_index: usize) -> bool {
    frame.window_before(footer_index, 6).is_some_and(|lines| {
        lines.iter().any(|line| {
            let line = line.trim();
            line == ">" || line.starts_with("> ")
        })
    })
}
