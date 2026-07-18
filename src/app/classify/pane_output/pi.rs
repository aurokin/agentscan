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

#[cfg(test)]
mod tests {
    use crate::app::classify;
    use crate::app::tests::{pane_output_status_pane, proc_fallback_pane};
    use crate::app::{Provider, StatusKind};

    #[test]
    fn pi_pane_output_marks_current_editor_footer_idle_only_after_provider_is_known() {
        let mut pi = pane_output_status_pane(787, Provider::Pi, "π - agentscan");

        classify::apply_pane_output_status_fallback(
            &mut pi,
            "Completed prior turn.\n\
         ────────────────────────────────\n\
                                         \n\
         ────────────────────────────────\n\
         ~/code/app\n\
         0.0%/200k                                      claude-sonnet\n",
        );

        assert_eq!(pi.status.kind, StatusKind::Idle);
        assert_eq!(pi.status.source, crate::app::StatusSource::PaneOutput);

        let mut unknown = proc_fallback_pane(788, "zsh", "custom title");
        classify::apply_pane_output_status_fallback(
            &mut unknown,
            "Completed prior turn.\n\
         ────────────────────────────────\n\
                                         \n\
         ────────────────────────────────\n\
         ~/code/app\n\
         0.0%/200k                                      claude-sonnet\n",
        );

        assert_eq!(unknown.status.kind, StatusKind::Unknown);
        assert_eq!(unknown.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn pi_pane_output_marks_current_working_loader_busy() {
        let mut pi = pane_output_status_pane(789, Provider::Pi, "π - agentscan");

        classify::apply_pane_output_status_fallback(
            &mut pi,
            "⠋ Working... (ctrl+c to interrupt)\n\
         ────────────────────────────────\n\
                                        \n\
         ────────────────────────────────\n\
         ~/code/app\n\
         0.0%/200k                                      claude-sonnet\n",
        );

        assert_eq!(pi.status.kind, StatusKind::Busy);
        assert_eq!(pi.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn pi_pane_output_marks_current_retry_loader_busy() {
        let mut pi = pane_output_status_pane(790, Provider::Pi, "π - agentscan");

        classify::apply_pane_output_status_fallback(
            &mut pi,
            // Busy paths require the same live `%/` footer anchor as idle paths.
            "Retrying (2/3) in 4s... (ctrl+c to cancel)\n\
         0.0%/200k                                      claude-sonnet\n",
        );

        assert_eq!(pi.status.kind, StatusKind::Busy);
        assert_eq!(pi.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn pi_pane_output_does_not_treat_working_prose_after_footer_as_busy() {
        let mut pi = pane_output_status_pane(793, Provider::Pi, "π - agentscan");

        classify::apply_pane_output_status_fallback(
            &mut pi,
            "────────────────────────────────\n\
         \n\
         ────────────────────────────────\n\
         ~/code/app\n\
         0.0%/200k                                      claude-sonnet\n\
         The Working... label is described in this documentation.\n",
        );

        assert_eq!(pi.status.kind, StatusKind::Unknown);
        assert_eq!(pi.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn pi_pane_output_unanchored_working_loader_stays_unknown() {
        let mut pi = pane_output_status_pane(794, Provider::Pi, "π - agentscan");

        classify::apply_pane_output_status_fallback(
            &mut pi,
            "⠋ Working... (ctrl+c to interrupt)\n",
        );

        assert_eq!(pi.status.kind, StatusKind::Unknown);
        assert_eq!(pi.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn pi_pane_output_uses_current_idle_footer_over_stale_busy_loader() {
        let mut pi = pane_output_status_pane(791, Provider::Pi, "π - agentscan");

        classify::apply_pane_output_status_fallback(
            &mut pi,
            "⠋ Working... (ctrl+c to interrupt)\n\
         Finished.\n\
         ────────────────────────────────\n\
                                         \n\
         ────────────────────────────────\n\
         ~/code/app\n\
         ?/200k                                      claude-sonnet\n",
        );

        assert_eq!(pi.status.kind, StatusKind::Idle);
        assert_eq!(pi.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn pi_pane_output_does_not_infer_idle_from_stale_editor_frame() {
        let mut pi = pane_output_status_pane(792, Provider::Pi, "π - agentscan");

        classify::apply_pane_output_status_fallback(
            &mut pi,
            "────────────────────────────────\n\
                                        \n\
         ────────────────────────────────\n\
         ~/code/app\n\
         0.0%/200k                                      claude-sonnet\n\
         \n\
         Planning edits\n\
         Reading files\n\
         Updating code\n\
         Running tests\n\
         Collecting output\n\
         Current line\n",
        );

        assert_eq!(pi.status.kind, StatusKind::Unknown);
        assert_eq!(pi.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn pi_pane_output_marks_idle_through_trailing_blank_padding() {
        // Real-world regression: a freshly started pi renders its editor frame and footer at
        // the top, leaving the rest of the taller pane blank. The trailing blank rows must not
        // push the current footer out of the "near the bottom" window.
        let mut pi = pane_output_status_pane(799, Provider::Pi, "π - agentscan");

        classify::apply_pane_output_status_fallback(
            &mut pi,
            "────────────────────────────────\n\
         \n\
         ────────────────────────────────\n\
         ~/code/agentscan (main)\n\
         $0.000 (sub) 0.0%/272k (auto)              (openai-codex) gpt-5.5 • medium\n\
         \n\
         \n\
         \n\
         \n\
         \n\
         \n\
         \n\
         \n",
        );

        assert_eq!(pi.status.kind, StatusKind::Idle);
        assert_eq!(pi.status.source, crate::app::StatusSource::PaneOutput);
    }
}
