use super::{PaneOutputFrame, StatusKind, dotted_version_token};

const OPENCODE_TIP_TEXT_INDENT: usize = 6;
const OPENCODE_MIN_BLANK_LINES_AFTER_PIN_TIP_WRAP: usize = 3;
const OPENCODE_PIN_SESSION_TIP_PREFIX: &str =
    "● Tip Press ctrl+f in the session list to pin a session so it stays at the";
const OPENCODE_PIN_SESSION_TIP_CONTINUATION: &str = "top";

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
    let only_trailing_chrome = opencode_trailing_chrome_after(frame, footer_index, false);
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
    let only_trailing_chrome = opencode_trailing_chrome_after(frame, border_index, true);
    only_trailing_chrome.then_some(border_index)
}

fn opencode_trailing_chrome_after(
    frame: &PaneOutputFrame<'_>,
    index: usize,
    allow_command_bar: bool,
) -> bool {
    let mut tip_notice = None;
    let only_chrome = frame.trailing_lines_after_are(index, |_, line, is_last| {
        opencode_trailing_chrome_line(line, is_last, allow_command_bar, &mut tip_notice)
    });
    // Trailing blank rows are trimmed before provider classifiers run, and the observed
    // OpenCode Go splash pins a status bar below the wrapped tip. If the captured tail ends
    // at the continuation itself, keep the pane unknown rather than treating a final `top`
    // output line as current footer chrome.
    only_chrome
        && !matches!(
            tip_notice,
            Some(OpencodeTipNotice {
                saw_observed_continuation: true,
                ..
            })
        )
}

struct OpencodeTipNotice {
    continuation_column: usize,
    allow_observed_top_continuation: bool,
    saw_observed_continuation: bool,
    blank_lines_after_observed_continuation: usize,
}

fn opencode_trailing_chrome_line(
    line: &str,
    is_last: bool,
    allow_command_bar: bool,
    tip_notice: &mut Option<OpencodeTipNotice>,
) -> bool {
    let raw_line = line;
    let trimmed = raw_line.trim();
    if trimmed.is_empty() {
        if let Some(notice) = tip_notice.as_mut()
            && notice.saw_observed_continuation
        {
            notice.blank_lines_after_observed_continuation += 1;
            return true;
        }
        *tip_notice = None;
        return true;
    }

    if let Some((marker_column, allow_observed_top_continuation)) = opencode_tip_notice(raw_line) {
        *tip_notice = Some(OpencodeTipNotice {
            continuation_column: marker_column + OPENCODE_TIP_TEXT_INDENT,
            allow_observed_top_continuation,
            saw_observed_continuation: false,
            blank_lines_after_observed_continuation: 0,
        });
        return true;
    }

    if let Some(notice) = tip_notice.as_mut() {
        if notice.saw_observed_continuation {
            if notice.blank_lines_after_observed_continuation
                < OPENCODE_MIN_BLANK_LINES_AFTER_PIN_TIP_WRAP
            {
                return false;
            }
            *tip_notice = None;
        } else if notice.allow_observed_top_continuation
            && opencode_tip_continuation_line(raw_line, notice.continuation_column)
        {
            notice.saw_observed_continuation = true;
            return true;
        } else {
            *tip_notice = None;
        }
    }

    (allow_command_bar && opencode_command_bar_footer_line(trimmed))
        || (is_last && opencode_bottom_status_bar_line(trimmed))
}

fn opencode_tip_notice(line: &str) -> Option<(usize, bool)> {
    let column = first_nonblank_column(line)?;
    let trimmed = line.trim_start();
    trimmed
        .starts_with("● Tip")
        .then_some((column, trimmed == OPENCODE_PIN_SESSION_TIP_PREFIX))
}

fn opencode_tip_continuation_line(line: &str, continuation_column: usize) -> bool {
    // Plain `capture-pane` text cannot distinguish arbitrary aligned output from wrapped
    // tip prose. Keep support to the observed OpenCode 1.15.11 pin-session wrap; other
    // wraps should stay unknown until there is stronger provider evidence.
    first_nonblank_column(line).is_some_and(|column| column == continuation_column)
        && line.trim() == OPENCODE_PIN_SESSION_TIP_CONTINUATION
}

fn first_nonblank_column(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    (!trimmed.is_empty()).then(|| line.chars().take_while(|ch| ch.is_whitespace()).count())
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
