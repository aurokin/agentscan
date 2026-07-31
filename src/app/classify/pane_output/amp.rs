use super::{PaneOutputFrame, StatusKind};

const MAX_COMPOSER_TAIL_LINES: usize = 8;
const MAX_COMPOSER_HEIGHT: usize = 7;
const MODE_SUFFIXES: &[&str] = &[" low ─╮", " medium ─╮", " high ─╮", " ultra ─╮"];

pub(super) fn status(output: &str) -> Option<StatusKind> {
    let frame = PaneOutputFrame::new(output);
    let bottom_index = frame
        .rposition(amp_composer_bottom)
        .filter(|&index| frame.is_within_tail(index, MAX_COMPOSER_TAIL_LINES))?;
    if frame.line(bottom_index)? != frame.last_nonblank()? {
        return None;
    }
    let top_index = frame.rposition_before(bottom_index, amp_composer_top)?;

    if bottom_index.saturating_sub(top_index) > MAX_COMPOSER_HEIGHT {
        return None;
    }

    let body = frame.range(top_index + 1, bottom_index)?;
    if body.is_empty() || !body.iter().all(|line| amp_composer_body(line)) {
        return None;
    }

    let bottom = frame.line(bottom_index)?.trim_start();
    let left_edge = bottom.strip_prefix('╰')?.trim_start();
    let marker = left_edge.chars().next()?;

    if marker == '─' {
        return Some(StatusKind::Idle);
    }

    matches!(marker, '≈' | '≋' | '∼').then_some(StatusKind::Busy)
}

fn amp_composer_top(line: &str) -> bool {
    let line = line.trim();
    line.starts_with('╭') && MODE_SUFFIXES.iter().any(|suffix| line.ends_with(suffix))
}

fn amp_composer_body(line: &str) -> bool {
    let line = line.trim();
    line.starts_with('│') && line.ends_with('│')
}

fn amp_composer_bottom(line: &str) -> bool {
    let line = line.trim();
    line.starts_with('╰') && line.ends_with("─╯")
}

#[cfg(test)]
mod tests {
    use crate::app::classify;
    use crate::app::tests::{
        assert_pane_output_status, assert_unprovidered_pane_output_unchanged,
        pane_output_status_pane,
    };
    use crate::app::{Provider, StatusKind};

    const IDLE: &str = "╭──────────────────────────────────────────────────────── medium ─╮\n\
                        │                                                                  │\n\
                        │                                                                  │\n\
                        │                                                                  │\n\
                        ╰────────────────────────────── ~/code/agentscan (main) ─╯\n";

    #[test]
    fn amp_pane_output_marks_current_composer_idle_only_after_provider_is_known() {
        assert_pane_output_status(
            850,
            Provider::Amp,
            "Terminal state machines - amp - ~/code/agentscan",
            IDLE,
            StatusKind::Idle,
            crate::app::StatusSource::PaneOutput,
        );
        assert_unprovidered_pane_output_unchanged(851, "zsh", "custom title", IDLE);
    }

    #[test]
    fn amp_pane_output_marks_observed_activity_glyphs_busy() {
        for (index, status_line) in [
            "≈ Connecting",
            "≋ Sending",
            "≋ Strting",
            "∼ Streaming",
            "∼ Streaming 196 tok",
        ]
        .iter()
        .enumerate()
        {
            let output = format!(
                "╭────────────────────────────────────────── $0.002 ─ medium ─╮\n\
                 │                                                                  │\n\
                 │                                                                  │\n\
                 │                                                                  │\n\
                 ╰ {status_line} ─────────────────── ~/code/agentscan (main) ─╯\n"
            );
            assert_pane_output_status(
                852 + index as u32,
                Provider::Amp,
                "⠍ Terminal state machines - amp - ~/code/agentscan",
                &output,
                StatusKind::Busy,
                crate::app::StatusSource::PaneOutput,
            );
        }
    }

    #[test]
    fn amp_pane_output_prefers_returned_idle_composer_over_stale_busy_frame() {
        let output = format!(
            "╭──────────────────────────────────────────── medium ─╮\n\
             │                                                              │\n\
             ╰ ∼ Streaming 196 tok ─────────────── ~/code/agentscan (main) ─╯\n\
             assistant response\n\
             {IDLE}"
        );
        assert_pane_output_status(
            860,
            Provider::Amp,
            "Terminal state machines - amp - ~/code/agentscan",
            &output,
            StatusKind::Idle,
            crate::app::StatusSource::PaneOutput,
        );
    }

    #[test]
    fn amp_pane_output_degrades_unobserved_or_incomplete_composers_to_unknown() {
        for output in [
            "╭──────────────────────────────────────────── medium ─╮\n\
             │                                                              │\n\
             ╰ ◇ Paused ─────────────────────── ~/code/agentscan (main) ─╯\n",
            "╭─────────────────────────────────────────────────────╮\n\
             │                                                              │\n\
             ╰────────────────────────────── ~/code/agentscan (main) ─╯\n",
            "╭──────────────────────────────────────────── medium ─╮\n\
             not a composer body row\n\
             ╰────────────────────────────── ~/code/agentscan (main) ─╯\n",
            "Prose about ∼ Streaming 196 tok without a current composer.\n",
            "╭──────────────────────────────────────────── medium ─╮\n\
             │                                                              │\n\
             ╰────────────────────────────── ~/code/agentscan (main) ─╯\n\
             Custom policy confirmation\n",
            "╭──────────────────────────────────────────── medium ─╮\n\
             │                                                              │\n\
             │                                                              │\n\
             │                                                              │\n\
             │                                                              │\n\
             │                                                              │\n\
             │                                                              │\n\
             │                                                              │\n\
             ╰────────────────────────────── ~/code/agentscan (main) ─╯\n",
        ] {
            let mut amp = pane_output_status_pane(
                861,
                Provider::Amp,
                "Terminal state machines - amp - ~/code/agentscan",
            );
            classify::apply_pane_output_status_fallback(&mut amp, output);
            assert_eq!(amp.status.kind, StatusKind::Unknown, "output: {output}");
            assert_eq!(
                amp.status.source,
                crate::app::StatusSource::NotChecked,
                "output: {output}"
            );
        }
    }
}
