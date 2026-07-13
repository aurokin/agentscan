use super::{PaneOutputFrame, StatusKind};

pub(super) fn status(output: &str) -> Option<StatusKind> {
    let frame = PaneOutputFrame::new(output);
    let idle_index = frame.rposition(codex_idle_prompt_line);
    let busy_index = frame.rposition(codex_current_busy_marker_line);

    if let Some(index) = busy_index
        && idle_index.is_none_or(|idle_index| {
            idle_index < index
                || codex_busy_marker_is_near_current_prompt(&frame, index, idle_index)
        })
    {
        return Some(StatusKind::Busy);
    }

    idle_index
        .is_some_and(|index| codex_prompt_is_near_current_footer(&frame, index))
        .then_some(StatusKind::Idle)
}

fn codex_idle_prompt_line(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with('›') && line.contains("Ask Codex to do anything")
}

fn codex_current_busy_marker_line(line: &str) -> bool {
    let line = line.trim();
    codex_interrupt_status_line(line) || codex_approval_prompt_line(line)
}

fn codex_interrupt_status_line(line: &str) -> bool {
    line.contains("esc to interrupt)") && line.contains('(')
}

fn codex_approval_prompt_line(line: &str) -> bool {
    line.contains("Press enter to confirm or esc to cancel")
        || line.contains("Yes, proceed")
        || line.contains("Reviewing ") && line.contains("approval request")
}

fn codex_busy_marker_is_near_current_prompt(
    frame: &PaneOutputFrame<'_>,
    busy_index: usize,
    idle_index: usize,
) -> bool {
    frame.forward_gap_before_is_within(busy_index, idle_index, 4, codex_status_gap_line)
        && codex_prompt_is_near_current_footer(frame, idle_index)
}

fn codex_status_gap_line(line: &str) -> bool {
    let line = line.trim();
    line.is_empty() || line.starts_with('└')
}

fn codex_prompt_is_near_current_footer(frame: &PaneOutputFrame<'_>, prompt_index: usize) -> bool {
    frame.tail_contains(prompt_index, 6, codex_footer_line)
}

fn codex_footer_line(line: &str) -> bool {
    let line = line.trim();
    line.contains("context left")
        || (line.contains("Context ") && line.contains(" used"))
        || line.contains("Fast on")
        || line.contains("tab to queue message")
        || codex_model_path_footer_line(line)
}

fn codex_model_path_footer_line(line: &str) -> bool {
    line.contains(" · ")
        && codex_model_footer_token(line)
        && (codex_footer_has_path_context(line) || codex_footer_has_mode_context(line))
}

fn codex_model_footer_token(line: &str) -> bool {
    line.split_whitespace()
        .any(|token| token.starts_with("gpt-") || is_openai_o_series_token(token))
}

/// OpenAI "o"-series model tokens (`o3`, `o4-mini`) are an `o` followed by a
/// digit. Anchoring on that digit keeps bare English words like `on`/`or`/`of`/
/// `output` from being mistaken for a model name in the footer.
fn is_openai_o_series_token(token: &str) -> bool {
    token
        .strip_prefix('o')
        .and_then(|rest| rest.chars().next())
        .is_some_and(|next| next.is_ascii_digit())
}

fn codex_footer_has_path_context(line: &str) -> bool {
    line.split(" · ").any(|part| {
        let part = part.trim();
        part.starts_with('/')
            || part.starts_with("~/")
            || part.starts_with("./")
            || part.contains("/Users/")
            || part.contains("/tmp/")
    })
}

fn codex_footer_has_mode_context(line: &str) -> bool {
    line.contains("Plan mode")
        || line.contains("Default mode")
        || line.contains("Shell mode")
        || line.contains("Side from ")
        || line.contains("Goal ")
}

#[cfg(test)]
mod tests {
    use super::codex_model_footer_token;

    #[test]
    fn accepts_openai_model_tokens() {
        assert!(codex_model_footer_token("o3 default · /tmp/project"));
        assert!(codex_model_footer_token("o4-mini high · /tmp/project"));
        assert!(codex_model_footer_token("gpt-5.2 default · /tmp/project"));
    }

    #[test]
    fn rejects_bare_english_o_words() {
        assert!(!codex_model_footer_token("on"));
        assert!(!codex_model_footer_token("or"));
        assert!(!codex_model_footer_token("of"));
        assert!(!codex_model_footer_token("output"));
    }
}
