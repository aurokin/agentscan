use super::{PaneOutputFrame, StatusKind};

// hermes busy input prompt command hints shown while a turn is running
// (`⚕ ❯ … msg=interrupt … /queue … Ctrl+C cancel`).
const INTERRUPT_MARKER: &str = "msg=interrupt";
const QUEUE_MARKER: &str = "/queue";
const CANCEL_HINT: &str = "Ctrl+C cancel";
// hermes turn-startup status line shown while the agent boots a turn.
const INITIALIZING_MARKER: &str = "Initializing agent...";

pub(super) fn status(output: &str) -> Option<StatusKind> {
    let frame = PaneOutputFrame::new(output);
    let busy_index = frame.rposition(hermes_busy_prompt_line);
    let turn_busy_index = frame.rposition(hermes_current_turn_busy_line);
    let idle_index = frame.rposition(hermes_idle_prompt_line);

    if let Some(index) = busy_index
        && hermes_status_bar_directly_above(&frame, index)
        && idle_index.is_none_or(|idle_index| idle_index < index)
        && hermes_prompt_is_current_frame(&frame, index)
    {
        return Some(StatusKind::Busy);
    }

    if let Some(index) = turn_busy_index
        && idle_index.is_none_or(|idle_index| idle_index < index)
        && hermes_turn_busy_marker_is_current(&frame, index)
    {
        return Some(StatusKind::Busy);
    }

    if let Some(index) = idle_index
        && hermes_status_bar_directly_above(&frame, index)
        && busy_index.is_none_or(|busy_index| busy_index < index)
        && hermes_prompt_is_current_frame(&frame, index)
    {
        return Some(StatusKind::Idle);
    }

    None
}

/// Whether a hermes prompt line reflects the current bottom frame rather than stale scrollback.
///
/// The live input box renders its `❯`/`⚕ ❯` prompt directly above the box's closing `────` rule
/// at the bottom of what hermes has drawn. The idle matcher accepts any `❯ <draft>` line, so a
/// submitted prompt or agent output that merely contains a `❯ …` line could otherwise sit deep in
/// scrollback with later output below it and be misread as the live prompt. Require that only box
/// rules and blank rows follow the prompt: any real content below it (output from a turn that has
/// since run) marks it stale. A multi-line draft conservatively reads as unknown rather than risk
/// resurrecting a ghost prompt.
fn hermes_prompt_is_current_frame(frame: &PaneOutputFrame<'_>, prompt_index: usize) -> bool {
    frame.trailing_lines_after_are(prompt_index, |_, line, _| {
        let line = line.trim();
        line.is_empty() || hermes_box_rule_line(line)
    })
}

fn hermes_box_rule_line(line: &str) -> bool {
    line.chars().count() >= 8 && line.chars().all(|ch| ch == '─' || ch == '━')
}

/// Whether the live hermes status bar sits directly above this prompt index.
///
/// The live input box renders as `<status bar>` → optional `────` rule → `❯`/`⚕ ❯` prompt, so
/// the status bar is at most a couple of rows above the prompt and only a box rule or blank may
/// sit between them. Requiring both proximity AND a clean intervening gap prevents an unrelated
/// `❯ <text>` line — e.g. a quoted shell prompt like `❯ npm test` in agent output, possibly with
/// prose like `Run this:` between it and an older status bar — from being classified idle.
fn hermes_status_bar_directly_above(frame: &PaneOutputFrame<'_>, prompt_index: usize) -> bool {
    let Some(window) = frame.window_before(prompt_index, 3) else {
        return false;
    };
    let start = prompt_index.saturating_sub(window.len());
    window
        .iter()
        .enumerate()
        .rev()
        .find(|(_, line)| hermes_status_bar_line(line.trim()))
        .is_some_and(|(rel_index, _)| {
            let status_index = start + rel_index;
            frame.forward_gap_before_all(status_index, prompt_index, |line| {
                let line = line.trim();
                line.is_empty() || hermes_box_rule_line(line)
            })
        })
}

fn hermes_status_bar_line(line: &str) -> bool {
    line.starts_with("⚕ ") && line.contains('│') && (line.contains("ctx") || line.contains("K/"))
}

/// Hermes' live input prompt while idle: a bare `❯`, or `❯ <draft>` when the user has typed but
/// not yet submitted (the agent is still not running a turn). The busy prompt is `⚕ ❯ …`, which
/// starts with `⚕`, so this stays unambiguous against it.
fn hermes_idle_prompt_line(line: &str) -> bool {
    let line = line.trim();
    line == "❯" || line.starts_with("❯ ")
}

fn hermes_busy_prompt_line(line: &str) -> bool {
    let line = line.trim();
    line.starts_with("⚕ ❯")
        && line.contains(INTERRUPT_MARKER)
        && line.contains(QUEUE_MARKER)
        && line.contains(CANCEL_HINT)
}

fn hermes_current_turn_busy_line(line: &str) -> bool {
    line.trim() == INITIALIZING_MARKER
}

fn hermes_turn_busy_marker_is_current(frame: &PaneOutputFrame<'_>, busy_index: usize) -> bool {
    frame.is_within_tail(busy_index, 12)
        && frame
            .lines_from(busy_index)
            .is_some_and(|lines| lines.iter().any(|line| hermes_box_rule_line(line.trim())))
}
