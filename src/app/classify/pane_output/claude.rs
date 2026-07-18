use super::{PaneOutputFrame, StatusKind};

// Claude Code interrupt hint shown in the status line while a turn is running.
const INTERRUPT_HINT_TOKENS: [&str; 3] = ["esc", "to", "interrupt"];
// Claude Code permission approval prompt shown while awaiting the user.
const WAITING_PERMISSION_MARKER: &str = "Waiting for permission";
// Claude Code idle input footer hint.
const SHORTCUTS_HINT: &str = "? for shortcuts";
// Claude Code mode-cycle footer hint (`shift+tab to cycle`); probed as two substrings.
const CYCLE_MODE_KEYBIND: &str = "shift+tab";
const CYCLE_MODE_ACTION: &str = "cycle";
// Claude Code footer mode indicators.
const AUTO_MODE_MARKER: &str = "auto on";
const PLAN_MODE_MARKER: &str = "plan on";
const ACCEPT_EDITS_MODE_MARKER: &str = "accept edits on";
const BYPASS_PERMISSIONS_MODE_MARKER: &str = "bypass permissions on";
const ULTRAPLAN_MODE_MARKER: &str = "ultraplan on";

pub(super) fn status(output: &str) -> Option<StatusKind> {
    let frame = PaneOutputFrame::new(output);
    let prompt_index = frame.rposition(claude_prompt_line);
    let busy_index = frame.rposition(claude_current_busy_marker_line);

    if let Some(index) = busy_index
        && prompt_index.is_some_and(|prompt_index| {
            claude_busy_marker_is_near_current_prompt(&frame, index, prompt_index)
        })
    {
        let kind = if frame
            .line(index)
            .is_some_and(claude_waiting_permission_line)
        {
            StatusKind::Waiting
        } else {
            StatusKind::Busy
        };
        return Some(kind);
    }

    prompt_index
        .is_some_and(|index| claude_prompt_is_near_current_footer(&frame, index))
        .then_some(StatusKind::Idle)
}

fn claude_prompt_line(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with('❯')
}

fn claude_current_busy_marker_line(line: &str) -> bool {
    let line = line.trim();
    claude_interrupt_hint_line(line) || claude_waiting_permission_line(line)
}

fn claude_interrupt_hint_line(line: &str) -> bool {
    line.split_whitespace()
        .map(|token| token.trim_matches(|ch: char| !ch.is_alphanumeric()))
        .collect::<Vec<_>>()
        .windows(INTERRUPT_HINT_TOKENS.len())
        .any(|tokens| tokens == INTERRUPT_HINT_TOKENS)
}

fn claude_waiting_permission_line(line: &str) -> bool {
    line.contains(WAITING_PERMISSION_MARKER)
}

fn claude_busy_marker_is_near_current_prompt(
    frame: &PaneOutputFrame<'_>,
    busy_index: usize,
    prompt_index: usize,
) -> bool {
    frame.gap_between_is_within(busy_index, prompt_index, 6, claude_status_gap_line)
        && claude_prompt_is_near_current_footer(frame, prompt_index)
}

fn claude_status_gap_line(line: &str) -> bool {
    let line = line.trim();
    line.is_empty() || claude_prompt_border_line(line)
}

fn claude_prompt_is_near_current_footer(frame: &PaneOutputFrame<'_>, prompt_index: usize) -> bool {
    frame.tail_contains(prompt_index, 8, claude_current_footer_line)
}

fn claude_current_footer_line(line: &str) -> bool {
    let line = line.trim();
    line.contains(SHORTCUTS_HINT)
        || line.contains(CYCLE_MODE_KEYBIND) && line.contains(CYCLE_MODE_ACTION)
        || line.contains(AUTO_MODE_MARKER)
        || line.contains(PLAN_MODE_MARKER)
        || line.contains(ACCEPT_EDITS_MODE_MARKER)
        || line.contains(BYPASS_PERMISSIONS_MODE_MARKER)
        || line.contains(ULTRAPLAN_MODE_MARKER)
        // Real busy frames can render this as their only footer-ish line. Only the strict,
        // token-delimited sequence is accepted here, never a loose `esc`/`interrupt` pair.
        || claude_interrupt_hint_line(line)
}

fn claude_prompt_border_line(line: &str) -> bool {
    let line = line.trim();
    line.chars().count() >= 8
        && line
            .chars()
            .all(|ch| matches!(ch, '─' | '╭' | '╮' | '╰' | '╯'))
}

#[cfg(test)]
mod tests {
    use crate::app::classify;
    use crate::app::tests::{pane_output_status_pane, proc_fallback_pane};
    use crate::app::{Provider, StatusKind};

    #[test]
    fn claude_pane_output_marks_current_prompt_idle_only_after_provider_is_known() {
        let mut claude = pane_output_status_pane(804, Provider::Claude, "Claude Code");

        classify::apply_pane_output_status_fallback(
            &mut claude,
            "╭────────────────────────────────────────╮\n\
         ❯ Try \"fix the failing test\"\n\
         ╰────────────────────────────────────────╯\n\
         ? for shortcuts\n",
        );

        assert_eq!(claude.status.kind, StatusKind::Idle);
        assert_eq!(claude.status.source, crate::app::StatusSource::PaneOutput);

        let mut unknown = proc_fallback_pane(805, "zsh", "custom title");
        classify::apply_pane_output_status_fallback(
            &mut unknown,
            "╭────────────────────────────────────────╮\n\
         ❯ Try \"fix the failing test\"\n\
         ╰────────────────────────────────────────╯\n\
         ? for shortcuts\n",
        );

        assert_eq!(unknown.status.kind, StatusKind::Unknown);
        assert_eq!(unknown.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn claude_pane_output_marks_current_interrupt_hint_busy() {
        let mut claude = pane_output_status_pane(806, Provider::Claude, "Claude Code");

        classify::apply_pane_output_status_fallback(
            &mut claude,
            "╭────────────────────────────────────────╮\n\
         ❯ \n\
         ╰────────────────────────────────────────╯\n\
         esc to interrupt\n",
        );

        assert_eq!(claude.status.kind, StatusKind::Busy);
        assert_eq!(claude.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn claude_pane_output_does_not_treat_interrupt_prose_as_busy() {
        let mut claude = pane_output_status_pane(810, Provider::Claude, "Claude Code");

        classify::apply_pane_output_status_fallback(
            &mut claude,
            "describe how to interrupt the running task\n\
         ╭──────────────────────────────────────╮\n\
         ❯ \n\
         ╰──────────────────────────────────────╯\n\
         ? for shortcuts\n",
        );

        assert_eq!(claude.status.kind, StatusKind::Idle);
        assert_eq!(claude.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn claude_pane_output_leaves_loose_interrupt_words_unknown() {
        let mut claude = pane_output_status_pane(811, Provider::Claude, "Claude Code");

        classify::apply_pane_output_status_fallback(
            &mut claude,
            "esc can describe how to interrupt safely\n\
         ╭──────────────────────────────────────╮\n\
         ❯ \n\
         ╰──────────────────────────────────────╯\n",
        );

        assert_eq!(claude.status.kind, StatusKind::Unknown);
        assert_eq!(claude.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn claude_pane_output_marks_current_permission_wait_waiting() {
        let mut claude = pane_output_status_pane(807, Provider::Claude, "Claude Code");

        classify::apply_pane_output_status_fallback(
            &mut claude,
            "Waiting for permission…\n\
         \n\
         ╭────────────────────────────────────────╮\n\
         ❯ \n\
         ╰────────────────────────────────────────╯\n\
         ? for shortcuts\n",
        );

        assert_eq!(claude.status.kind, StatusKind::Waiting);
        assert_eq!(claude.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn claude_pane_output_upgrades_title_busy_to_waiting_on_permission_prompt() {
        // A spinner-glyph title reads busy, but the screen shows a permission
        // prompt: the waiting refinement must upgrade busy → waiting.
        let mut claude = pane_output_status_pane(817, Provider::Claude, "Claude Code");
        claude.status = crate::app::PaneStatus::title(StatusKind::Busy);

        classify::apply_pane_output_status_fallback(
            &mut claude,
            "Waiting for permission…\n\
         \n\
         ╭────────────────────────────────────────╮\n\
         ❯ \n\
         ╰────────────────────────────────────────╯\n\
         ? for shortcuts\n",
        );

        assert_eq!(claude.status.kind, StatusKind::Waiting);
        assert_eq!(claude.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn claude_pane_output_keeps_title_busy_when_screen_reads_busy_or_idle() {
        // Refinement of a busy title accepts only a waiting read: a busy read
        // must not churn provenance, and an idle read must not invert status.
        for output in [
            // Busy: current interrupt hint above the prompt box.
            "(esc to interrupt)\n\
             ╭────────────────────────────────────────╮\n\
             ❯ \n\
             ╰────────────────────────────────────────╯\n",
            // Idle: plain current prompt with shortcuts footer.
            "╭────────────────────────────────────────╮\n\
             ❯ \n\
             ╰────────────────────────────────────────╯\n\
             ? for shortcuts\n",
        ] {
            let mut claude = pane_output_status_pane(818, Provider::Claude, "Claude Code");
            claude.status = crate::app::PaneStatus::title(StatusKind::Busy);

            classify::apply_pane_output_status_fallback(&mut claude, output);

            assert_eq!(claude.status.kind, StatusKind::Busy);
            assert_eq!(claude.status.source, crate::app::StatusSource::TmuxTitle);
        }
    }

    #[test]
    fn claude_pane_output_ignores_stale_prompt_without_current_footer() {
        let mut claude = pane_output_status_pane(808, Provider::Claude, "Claude Code");

        classify::apply_pane_output_status_fallback(
            &mut claude,
            "╭────────────────────────────────────────╮\n\
         ❯ Try \"fix the failing test\"\n\
         ╰────────────────────────────────────────╯\n\
         older transcript output\n\
         command result\n\
         done\n\
         shell prompt\n",
        );

        assert_eq!(claude.status.kind, StatusKind::Unknown);
        assert_eq!(claude.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn claude_pane_output_ignores_ascii_angle_output_near_footer() {
        let mut claude = pane_output_status_pane(809, Provider::Claude, "Claude Code");

        classify::apply_pane_output_status_fallback(
            &mut claude,
            "> quoted transcript output\n\
         ? for shortcuts\n",
        );

        assert_eq!(claude.status.kind, StatusKind::Unknown);
        assert_eq!(claude.status.source, crate::app::StatusSource::NotChecked);
    }
}
