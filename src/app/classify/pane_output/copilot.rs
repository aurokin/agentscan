use super::{PaneOutputFrame, StatusKind};

// Copilot busy status line shown while a turn is running.
const THINKING_CANCEL_HINT: &str = "Thinking (Esc to cancel";
// Copilot working footer probes (`Working — esc to cancel`).
const WORKING_MARKER: &str = "Working";
const ESC_MARKER: &str = "esc";
const CANCEL_MARKER: &str = "cancel";
// Copilot idle footer command hints.
const COMMANDS_HINT: &str = "/ commands";
const HELP_HINT: &str = "? help";
const FILES_HINT: &str = "@ files";
const ISSUES_HINT: &str = "# issues";
// Copilot folder-trust modal copy.
const TRUST_MODAL_TITLE: &str = "Confirm folder trust";
const TRUST_MODAL_QUESTION: &str = "Do you trust the files in this folder?";

pub(super) fn status(output: &str) -> Option<StatusKind> {
    let frame = PaneOutputFrame::new(output);
    if copilot_pane_output_indicates_busy(&frame) {
        return Some(StatusKind::Busy);
    }

    copilot_current_prompt_visible(&frame).then_some(StatusKind::Idle)
}

fn copilot_pane_output_indicates_busy(frame: &PaneOutputFrame<'_>) -> bool {
    copilot_current_status_line(frame).is_some_and(|line| line.contains(THINKING_CANCEL_HINT))
        || copilot_current_working_footer_visible(frame)
        || copilot_current_bordered_prompt_footer(frame).is_some_and(copilot_working_footer_line)
        || copilot_current_trust_prompt_visible(frame)
}

fn copilot_current_status_line<'a>(frame: &'a PaneOutputFrame<'a>) -> Option<&'a str> {
    let prompt_index = frame.rposition(|line| line.trim() == "❯")?;
    let context_index = frame
        .lines_before(prompt_index)?
        .iter()
        .rposition(|line| copilot_prompt_context_line(line))?;

    let status_line = frame.line(context_index.checked_sub(1)?)?.trim();
    (!status_line.is_empty()).then_some(status_line)
}

fn copilot_prompt_context_line(line: &str) -> bool {
    let line = line.trim();
    (line.starts_with('/') || line.starts_with("~/")) && !line.starts_with(COMMANDS_HINT)
}

fn copilot_current_prompt_visible(frame: &PaneOutputFrame<'_>) -> bool {
    if copilot_current_bordered_prompt_footer(frame).is_some_and(copilot_idle_footer_line) {
        return true;
    }

    let Some(prompt_index) = frame.rposition(|line| line.trim() == "❯") else {
        return false;
    };
    let Some(context_index) = frame.lines_before(prompt_index).and_then(|lines| {
        lines
            .iter()
            .rposition(|line| copilot_prompt_context_line(line))
    }) else {
        return false;
    };

    frame
        .lines_from(prompt_index)
        .is_some_and(|lines| lines.iter().any(|line| copilot_idle_footer_line(line)))
        && frame
            .range(context_index, prompt_index)
            .is_some_and(|lines| lines.iter().any(|line| copilot_separator_line(line)))
}

fn copilot_current_bordered_prompt_footer<'a>(frame: &'a PaneOutputFrame<'a>) -> Option<&'a str> {
    let bottom_index = frame.rposition(copilot_bordered_prompt_bottom_line)?;
    let top_index = frame.rposition_before(bottom_index, copilot_bordered_prompt_top_line)?;
    let context_line = frame.previous_nonblank_before(top_index)?;
    if !copilot_prompt_context_line(context_line) {
        return None;
    }

    let input_lines = frame.range(top_index + 1, bottom_index)?;
    if input_lines.is_empty()
        || !input_lines
            .iter()
            .all(|line| copilot_bordered_prompt_input_line(line))
    {
        return None;
    }

    if !frame.trailing_lines_after_are(bottom_index, |_, line, _| {
        let line = line.trim();
        line.is_empty() || copilot_idle_footer_line(line) || copilot_working_footer_line(line)
    }) {
        return None;
    }

    frame
        .lines_from(bottom_index)?
        .iter()
        .skip(1)
        .copied()
        .map(str::trim)
        .find(|line| !line.is_empty())
}

fn copilot_current_working_footer_visible(frame: &PaneOutputFrame<'_>) -> bool {
    let Some(prompt_index) = frame.rposition(|line| line.trim() == "❯") else {
        return false;
    };
    let Some(context_index) = frame.lines_before(prompt_index).and_then(|lines| {
        lines
            .iter()
            .rposition(|line| copilot_prompt_context_line(line))
    }) else {
        return false;
    };

    frame
        .range(context_index, prompt_index)
        .is_some_and(|lines| lines.iter().any(|line| copilot_separator_line(line)))
        && frame
            .lines_from(prompt_index)
            .is_some_and(|lines| lines.iter().any(|line| copilot_working_footer_line(line)))
}

fn copilot_working_footer_line(line: &str) -> bool {
    let line = line.trim();
    line.contains(WORKING_MARKER) && line.contains(ESC_MARKER) && line.contains(CANCEL_MARKER)
}

fn copilot_idle_footer_line(line: &str) -> bool {
    let line = line.trim();
    (line.contains(COMMANDS_HINT) && line.contains(HELP_HINT))
        || (line.contains(FILES_HINT) && line.contains(ISSUES_HINT))
}

fn copilot_bordered_prompt_top_line(line: &str) -> bool {
    let line = line.trim();
    line.starts_with('╻') && line.chars().skip(1).all(|ch| ch == '▄')
}

fn copilot_bordered_prompt_input_line(line: &str) -> bool {
    line.trim_start().starts_with('┃')
}

fn copilot_bordered_prompt_bottom_line(line: &str) -> bool {
    let line = line.trim();
    line.starts_with('╹') && line.chars().skip(1).all(|ch| ch == '▀')
}

fn copilot_separator_line(line: &str) -> bool {
    let line = line.trim();
    line.len() >= 8 && line.chars().all(|ch| ch == '─')
}

fn copilot_current_trust_prompt_visible(frame: &PaneOutputFrame<'_>) -> bool {
    let Some(modal_index) = frame.rposition(|line| line.contains(TRUST_MODAL_TITLE)) else {
        return false;
    };

    let Some(modal_lines) = frame.lines_from(modal_index) else {
        return false;
    };
    let normal_prompt_after_modal = modal_lines.iter().any(|line| line.trim() == "❯");
    !normal_prompt_after_modal
        && modal_lines
            .iter()
            .any(|line| line.contains(TRUST_MODAL_QUESTION))
}
