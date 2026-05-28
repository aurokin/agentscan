use super::StatusKind;

pub(super) fn status(output: &str) -> Option<StatusKind> {
    if copilot_pane_output_indicates_busy(output) {
        return Some(StatusKind::Busy);
    }

    copilot_current_prompt_visible(output).then_some(StatusKind::Idle)
}

fn copilot_pane_output_indicates_busy(output: &str) -> bool {
    copilot_current_status_line(output).is_some_and(|line| line.contains("Thinking (Esc to cancel"))
        || copilot_current_trust_prompt_visible(output)
}

fn copilot_current_status_line(output: &str) -> Option<&str> {
    let lines: Vec<&str> = output.lines().collect();
    let prompt_index = lines.iter().rposition(|line| line.trim() == "❯")?;
    let context_index = lines[..prompt_index]
        .iter()
        .rposition(|line| copilot_prompt_context_line(line))?;

    let status_line = lines[..context_index].last()?.trim();
    (!status_line.is_empty()).then_some(status_line)
}

fn copilot_prompt_context_line(line: &str) -> bool {
    let line = line.trim();
    (line.starts_with('/') || line.starts_with("~/")) && !line.starts_with("/ commands")
}

fn copilot_current_prompt_visible(output: &str) -> bool {
    let lines: Vec<&str> = output.lines().collect();
    let Some(prompt_index) = lines.iter().rposition(|line| line.trim() == "❯") else {
        return false;
    };
    let Some(context_index) = lines[..prompt_index]
        .iter()
        .rposition(|line| copilot_prompt_context_line(line))
    else {
        return false;
    };

    lines[prompt_index..]
        .iter()
        .any(|line| line.contains("/ commands") && line.contains("? help"))
        && lines[context_index..prompt_index]
            .iter()
            .any(|line| copilot_separator_line(line))
}

fn copilot_separator_line(line: &str) -> bool {
    let line = line.trim();
    line.len() >= 8 && line.chars().all(|ch| ch == '─')
}

fn copilot_current_trust_prompt_visible(output: &str) -> bool {
    let lines: Vec<&str> = output.lines().collect();
    let Some(modal_index) = lines
        .iter()
        .rposition(|line| line.contains("Confirm folder trust"))
    else {
        return false;
    };

    let modal_lines = &lines[modal_index..];
    let normal_prompt_after_modal = modal_lines.iter().any(|line| line.trim() == "❯");
    !normal_prompt_after_modal
        && modal_lines
            .iter()
            .any(|line| line.contains("Do you trust the files in this folder?"))
}
