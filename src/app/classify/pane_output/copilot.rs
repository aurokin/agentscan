use super::{PaneOutputFrame, StatusKind};

pub(super) fn status(output: &str) -> Option<StatusKind> {
    let frame = PaneOutputFrame::new(output);
    if copilot_pane_output_indicates_busy(&frame) {
        return Some(StatusKind::Busy);
    }

    copilot_current_prompt_visible(&frame).then_some(StatusKind::Idle)
}

fn copilot_pane_output_indicates_busy(frame: &PaneOutputFrame<'_>) -> bool {
    copilot_current_status_line(frame).is_some_and(|line| line.contains("Thinking (Esc to cancel"))
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
    (line.starts_with('/') || line.starts_with("~/")) && !line.starts_with("/ commands")
}

fn copilot_current_prompt_visible(frame: &PaneOutputFrame<'_>) -> bool {
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

    frame.lines_from(prompt_index).is_some_and(|lines| {
        lines
            .iter()
            .any(|line| line.contains("/ commands") && line.contains("? help"))
    }) && frame
        .range(context_index, prompt_index)
        .is_some_and(|lines| lines.iter().any(|line| copilot_separator_line(line)))
}

fn copilot_separator_line(line: &str) -> bool {
    let line = line.trim();
    line.len() >= 8 && line.chars().all(|ch| ch == '─')
}

fn copilot_current_trust_prompt_visible(frame: &PaneOutputFrame<'_>) -> bool {
    let Some(modal_index) = frame.rposition(|line| line.contains("Confirm folder trust")) else {
        return false;
    };

    let Some(modal_lines) = frame.lines_from(modal_index) else {
        return false;
    };
    let normal_prompt_after_modal = modal_lines.iter().any(|line| line.trim() == "❯");
    !normal_prompt_after_modal
        && modal_lines
            .iter()
            .any(|line| line.contains("Do you trust the files in this folder?"))
}
