use super::{PaneOutputFrame, StatusKind, dotted_version_token};

pub(super) fn status(output: &str) -> Option<StatusKind> {
    let frame = PaneOutputFrame::new(output);

    // Bottom-most current idle prompt: either the placeholder input row (older build) near
    // the current footer, or the newer build's command-bar input box. Folding both into one
    // index lets a busy marker only win when it is below the live prompt — a stale
    // approval/question row above the current prompt must not force busy.
    let placeholder_index = frame
        .rposition(opencode_idle_prompt_line)
        .filter(|&index| opencode_prompt_is_near_current_footer(&frame, index));
    let command_bar_index = opencode_current_command_bar_index(&frame);
    let input_box_index = opencode_current_input_box_index(&frame);
    let idle_index = placeholder_index
        .max(command_bar_index)
        .max(input_box_index);

    let busy_index = frame.rposition(opencode_current_busy_marker_line);

    if let Some(index) = busy_index
        && opencode_busy_marker_is_current(
            &frame,
            index,
            idle_index,
            command_bar_index,
            input_box_index,
        )
    {
        return Some(StatusKind::Busy);
    }

    idle_index.map(|_| StatusKind::Idle)
}

/// Whether a busy marker reflects the current bottom frame rather than stale scrollback.
///
/// The capture is the last 30 rows including scrollback, so an old approval/interrupt line
/// can sit above a frame that has since scrolled on. A busy marker is current when it is
/// below the live idle prompt (a new run started under it), pinned in the persistent prompt
/// footer region (`esc interrupt` rendered just above the command bar or the input box border),
/// or — when there is no current idle anchor at all — is itself in the current bottom frame. A
/// stale marker scrolled up with no current prompt below it must stay unknown, not busy.
///
/// Both the command bar (`tab agents …`) and the input box border (`╹▀▀▀`) are valid footer
/// anchors: a used session folds the command bar into the bottom status bar, leaving the box
/// border as the only footer landmark, so a current `esc interrupt` above that box must still
/// win over the input-box idle anchor rather than fall through to idle.
fn opencode_busy_marker_is_current(
    frame: &PaneOutputFrame<'_>,
    busy_index: usize,
    idle_index: Option<usize>,
    command_bar_index: Option<usize>,
    input_box_index: Option<usize>,
) -> bool {
    let in_current_footer =
        opencode_busy_marker_in_current_footer(frame, busy_index, command_bar_index)
            || opencode_busy_marker_in_current_footer(frame, busy_index, input_box_index);
    match idle_index {
        Some(idle_index) => idle_index < busy_index || in_current_footer,
        None => in_current_footer || opencode_prompt_is_near_current_footer(frame, busy_index),
    }
}

fn opencode_busy_marker_in_current_footer(
    frame: &PaneOutputFrame<'_>,
    busy_index: usize,
    command_bar_index: Option<usize>,
) -> bool {
    let Some(command_bar) = command_bar_index else {
        return false;
    };
    frame.forward_gap_before_all(busy_index, command_bar, opencode_prompt_gap_line)
}

fn opencode_prompt_gap_line(line: &str) -> bool {
    let line = line.trim();
    line.is_empty()
        || line.starts_with('┃')
        || line.starts_with('╹')
        || line.starts_with('╭')
        || line.starts_with('│')
        || line.starts_with('╰')
}

fn opencode_idle_prompt_line(line: &str) -> bool {
    let line = line.trim_start();
    line.contains("Ask anything... \"") || line.contains("Run a command... \"")
}

/// Index of the newer build's command bar when its input box is the current prompt.
///
/// The bordered input box sits directly above a `tab agents  ctrl+p commands` command bar.
/// The capture is the last 30 rows including scrollback, so only opencode's own trailing
/// chrome (blank rows, notices, the bottom status bar) may follow it; a command bar trailed
/// by real agent output is a stale frame, not the current prompt.
fn opencode_current_command_bar_index(frame: &PaneOutputFrame<'_>) -> Option<usize> {
    let footer_index = frame.rposition(opencode_command_bar_footer_line)?;
    let has_input_box = frame
        .window_before(footer_index, 2)?
        .iter()
        .any(|line| opencode_input_box_bottom_border(line));

    // Only opencode's own chrome may follow the command bar: blank rows, a `● Tip` notice,
    // or the pinned bottom status bar as the final row. Anything else means the command bar
    // is a stale frame in the scrollback capture, not the current prompt.
    let only_trailing_chrome = frame.trailing_lines_after_are(footer_index, |_, line, is_last| {
        let line = line.trim();
        line.is_empty()
            || line.starts_with("● Tip")
            || (is_last && opencode_bottom_status_bar_line(line))
    });
    (has_input_box && only_trailing_chrome).then_some(footer_index)
}

/// Index of the bordered input box's bottom border when that box is the current idle prompt.
///
/// `opencode_current_command_bar_index` anchors on the `tab agents  ctrl+p commands` hint, but
/// the live build drops that hint once a session has activity, folding the command bar into the
/// bottom status bar (`<tokens> (<pct>) · $<cost>  ctrl+p commands  • OpenCode <ver>`). The stable
/// element across fresh and used sessions is the input box's `╹▀▀▀` bottom border, so anchor on
/// it: it is the current prompt when only opencode's own trailing chrome follows (blank rows, the
/// `tab agents` command bar, a `● Tip` notice, or the pinned bottom status bar as the final row).
/// A border trailed by real agent output is a stale frame in the scrollback capture, not the
/// current prompt. Busy markers still win — this only contributes to the idle anchor.
fn opencode_current_input_box_index(frame: &PaneOutputFrame<'_>) -> Option<usize> {
    let border_index = frame.rposition(opencode_input_box_bottom_border)?;
    let only_trailing_chrome = frame.trailing_lines_after_are(border_index, |_, line, is_last| {
        let line = line.trim();
        line.is_empty()
            || line.starts_with("● Tip")
            || opencode_command_bar_footer_line(line)
            || (is_last && opencode_bottom_status_bar_line(line))
    });
    only_trailing_chrome.then_some(border_index)
}

fn opencode_command_bar_footer_line(line: &str) -> bool {
    let line = line.trim();
    line.contains("tab agents") && line.contains("ctrl+p commands")
}

fn opencode_input_box_bottom_border(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with('╹') && line.contains('▀')
}

/// opencode's pinned bottom status bar, e.g. `~/path:branch    1.15.11` or
/// `… • OpenCode 1.15.11`. Matched by its actual shape — the `• OpenCode` brand followed by
/// a version, or a `path:branch` line ending in a version token — so arbitrary agent output
/// that merely mentions a semver/IP (`Updated SDK to 1.2.3`, `See RFC 192.168.1.1`) or a
/// bare file path is not mistaken for chrome.
fn opencode_bottom_status_bar_line(line: &str) -> bool {
    if line.contains("• OpenCode") {
        return line.split_whitespace().any(dotted_version_token);
    }
    (line.starts_with("~/") || line.starts_with('/'))
        && line.contains(':')
        && line
            .split_whitespace()
            .next_back()
            .is_some_and(dotted_version_token)
}

fn opencode_current_busy_marker_line(line: &str) -> bool {
    let line = line.trim();
    opencode_interrupt_hint_line(line)
        || opencode_permission_prompt_line(line)
        || opencode_question_prompt_line(line)
}

fn opencode_interrupt_hint_line(line: &str) -> bool {
    line.contains("esc") && line.contains("interrupt")
}

fn opencode_permission_prompt_line(line: &str) -> bool {
    line.contains("Permission required")
        || line.contains("Reject permission")
        || line.contains("Allow once")
        || line.contains("Allow always")
        || (line.contains('△') && line.contains("Permission"))
}

fn opencode_question_prompt_line(line: &str) -> bool {
    line.contains("Reject question")
        || line.contains("Waiting for question event")
        || line.contains("# Questions")
}

fn opencode_prompt_is_near_current_footer(
    frame: &PaneOutputFrame<'_>,
    prompt_index: usize,
) -> bool {
    frame.is_within_tail(prompt_index, 8)
}
