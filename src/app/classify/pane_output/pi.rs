use super::{PaneOutputFrame, StatusKind};

// pi working loader shown while a turn is running (`Working… (… to interrupt)`).
const WORKING_MARKER: &str = "Working...";
const INTERRUPT_HINT: &str = "to interrupt";
// pi retry loader shown while retrying a request.
const RETRYING_MARKER: &str = "Retrying (";
// pi cancel hint shared by the retry/compaction/bash loaders (`(… to cancel)`).
const CANCEL_HINT: &str = " to cancel)";
// pi context-compaction loaders.
const COMPACTING_MARKER: &str = "Compacting context...";
const AUTO_COMPACTING_MARKER: &str = "Auto-compacting...";
// pi bash-execution loader.
const RUNNING_MARKER: &str = "Running...";

pub(super) fn status(output: &str) -> Option<StatusKind> {
    let frame = PaneOutputFrame::new(output);
    let idle_index = frame.rposition(pi_editor_border_line);
    let busy_index = frame.rposition(pi_current_busy_marker_line);
    let footer_index = frame.rposition(pi_footer_context_line);

    if let Some(index) = busy_index
        && footer_index.is_some_and(|footer_index| {
            index <= footer_index && pi_footer_is_current(&frame, footer_index)
        })
        && idle_index.is_none_or(|idle_index| {
            idle_index < index || pi_busy_marker_is_near_current_editor(&frame, index, idle_index)
        })
    {
        return Some(StatusKind::Busy);
    }

    idle_index
        .is_some_and(|index| pi_editor_frame_is_near_current_footer(&frame, index))
        .then_some(StatusKind::Idle)
}

fn pi_current_busy_marker_line(line: &str) -> bool {
    let line = line.trim();
    pi_working_loader_line(line)
        || pi_retry_loader_line(line)
        || pi_compaction_loader_line(line)
        || pi_running_bash_line(line)
}

fn pi_busy_marker_is_near_current_editor(
    frame: &PaneOutputFrame<'_>,
    busy_index: usize,
    idle_index: usize,
) -> bool {
    frame.forward_gap_before_is_within(busy_index, idle_index, 4, pi_editor_gap_line)
        && pi_editor_frame_is_near_current_footer(frame, idle_index)
}

fn pi_editor_gap_line(line: &str) -> bool {
    let line = line.trim();
    line.is_empty() || pi_editor_border_line(line)
}

fn pi_working_loader_line(line: &str) -> bool {
    let line = line.trim_start();
    let Some(spinner) = line.chars().next() else {
        return false;
    };
    if !('\u{2800}'..='\u{28ff}').contains(&spinner) {
        return false;
    }
    let status = line[spinner.len_utf8()..].trim_start();
    pi_working_marker_at_word_boundary(status) || pi_parenthesized_interrupt_status(status)
}

fn pi_working_marker_at_word_boundary(status: &str) -> bool {
    status.strip_prefix(WORKING_MARKER).is_some_and(|suffix| {
        suffix.is_empty()
            || suffix
                .chars()
                .next()
                .is_some_and(|ch| ch.is_whitespace() || ch == '(')
    })
}

fn pi_parenthesized_interrupt_status(status: &str) -> bool {
    let Some((_, parenthesized)) = status.rsplit_once('(') else {
        return false;
    };
    parenthesized
        .strip_suffix(')')
        .and_then(|inner| inner.strip_suffix(INTERRUPT_HINT))
        .is_some_and(|control| !control.trim().is_empty())
}

fn pi_retry_loader_line(line: &str) -> bool {
    line.contains(RETRYING_MARKER) && line.contains(CANCEL_HINT)
}

fn pi_compaction_loader_line(line: &str) -> bool {
    (line.contains(COMPACTING_MARKER) || line.contains(AUTO_COMPACTING_MARKER))
        && line.contains(CANCEL_HINT)
}

fn pi_running_bash_line(line: &str) -> bool {
    line.contains(RUNNING_MARKER) && line.contains(CANCEL_HINT)
}

fn pi_editor_border_line(line: &str) -> bool {
    let line = line.trim();
    line.chars().count() >= 8 && line.chars().all(|ch| ch == '─')
}

fn pi_editor_frame_is_near_current_footer(
    frame: &PaneOutputFrame<'_>,
    border_index: usize,
) -> bool {
    frame.is_within_tail(border_index, 6)
        && frame.last_nonblank().is_some_and(pi_footer_context_line)
}

fn pi_footer_is_current(frame: &PaneOutputFrame<'_>, footer_index: usize) -> bool {
    frame.is_within_tail(footer_index, 6)
        && frame.last_nonblank().is_some_and(pi_footer_context_line)
}

fn pi_footer_context_line(line: &str) -> bool {
    let line = line.trim();
    pi_footer_context_token(line)
        && !pi_current_busy_marker_line(line)
        && !line.contains("Error:")
        && !line.contains("Warning:")
}

fn pi_footer_context_token(line: &str) -> bool {
    line.split_whitespace().any(|token| {
        token.contains("%/")
            || (token.starts_with("?/") && token.chars().skip(2).any(|ch| ch.is_ascii_digit()))
    })
}
