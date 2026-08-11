use super::{PaneOutputFrame, StatusKind};

// Claude Code interrupt hint shown in the status line while a turn is running.
const INTERRUPT_HINT_TOKENS: [&str; 3] = ["esc", "to", "interrupt"];
// Claude Code permission approval prompt shown while awaiting the user.
const WAITING_PERMISSION_MARKER: &str = "Waiting for permission";
const PERMISSION_QUESTION: &str = "Do you want to proceed?";
const CANCEL_HINT: &str = "Esc to cancel";
// AskUserQuestion choice and review markers observed in Claude Code 2.1.228.
const ASK_CHAT_OPTION: &str = "Chat about this";
const ASK_SELECT_HINT: &str = "Enter to select";
const REVIEW_HEADING: &str = "Review your answers";
const REVIEW_READY_QUESTION: &str = "Ready to submit your answers?";
const REVIEW_SUBMIT_OPTION: &str = "Submit answers";
const REVIEW_CANCEL_OPTION: &str = "Cancel";
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
    // Dialog checks can run before busy detection because each one proves it is
    // current: choice and permission views reject a newer `❯` prompt, while
    // review requires its Cancel option to be the last rendered line. Claude's
    // busy matcher below requires that newer prompt, so stale dialogs cannot
    // override current busy evidence.
    if claude_current_waiting_dialog(&frame) {
        return Some(StatusKind::Waiting);
    }

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

fn claude_current_waiting_dialog(frame: &PaneOutputFrame<'_>) -> bool {
    claude_ask_question_dialog(frame)
        || claude_ask_review_dialog(frame)
        || claude_permission_dialog(frame)
}

fn claude_ask_question_dialog(frame: &PaneOutputFrame<'_>) -> bool {
    let Some(chat_index) = frame.rposition(claude_ask_chat_option_line) else {
        return false;
    };
    let Some(cancel_index) = frame.rposition(|line| line.contains(CANCEL_HINT)) else {
        return false;
    };
    let Some(dialog_tail) = frame.lines_from(chat_index) else {
        return false;
    };

    chat_index < cancel_index
        && frame.is_within_tail(chat_index, 12)
        && frame.is_within_tail(cancel_index, 4)
        && dialog_tail.iter().any(|line| line.contains(ASK_SELECT_HINT))
        // A completed dialog can remain in scrollback above a live prompt. The
        // current question view has no main prompt after its Chat option.
        && !dialog_tail.iter().skip(1).any(|line| claude_prompt_line(line))
}

fn claude_ask_review_dialog(frame: &PaneOutputFrame<'_>) -> bool {
    let Some(review_index) = frame.rposition(|line| line.trim() == REVIEW_HEADING) else {
        return false;
    };
    let Some(ready_index) = frame.rposition(|line| line.trim() == REVIEW_READY_QUESTION) else {
        return false;
    };
    let Some(submit_index) =
        frame.rposition(|line| claude_numbered_option_line(line, REVIEW_SUBMIT_OPTION))
    else {
        return false;
    };

    review_index < ready_index
        && ready_index < submit_index
        && frame.is_within_tail(ready_index, 10)
        && frame.is_within_tail(submit_index, 6)
        && frame
            .last_nonblank()
            .is_some_and(|line| claude_numbered_option_line(line, REVIEW_CANCEL_OPTION))
}

fn claude_permission_dialog(frame: &PaneOutputFrame<'_>) -> bool {
    let Some(question_index) = frame.rposition(|line| line.trim() == PERMISSION_QUESTION) else {
        return false;
    };
    let Some(cancel_index) = frame.rposition(|line| line.contains(CANCEL_HINT)) else {
        return false;
    };
    let Some(permission_tail) = frame.lines_from(cancel_index) else {
        return false;
    };

    question_index < cancel_index
        && frame.is_within_tail(question_index, 12)
        && frame.is_within_tail(cancel_index, 4)
        && !permission_tail
            .iter()
            .skip(1)
            .any(|line| claude_prompt_line(line))
}

fn claude_ask_chat_option_line(line: &str) -> bool {
    line.trim_end().ends_with(ASK_CHAT_OPTION)
}

fn claude_numbered_option_line(line: &str, expected: &str) -> bool {
    let line = line.trim_start();
    let line = line.strip_prefix('❯').unwrap_or(line).trim_start();
    line.split_once(". ").is_some_and(|(number, label)| {
        !number.is_empty()
            && number.chars().all(|character| character.is_ascii_digit())
            && label.trim() == expected
    })
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
    fn claude_pane_output_marks_current_permission_dialog_waiting() {
        let mut claude = pane_output_status_pane(819, Provider::Claude, "Claude Code");
        claude.status = crate::app::PaneStatus::title(StatusKind::Idle);

        classify::apply_pane_output_status_fallback(
            &mut claude,
            "Bash command\n\
             \n\
               touch /tmp/probe\n\
             \n\
             Do you want to proceed?\n\
             ❯ 1. Yes\n\
               2. Yes, and always allow access to tmp/ from this project\n\
               3. No\n\
             \n\
             Esc to cancel · Tab to amend · ctrl+e to explain\n",
        );

        assert_eq!(claude.status.kind, StatusKind::Waiting);
        assert_eq!(claude.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn claude_pane_output_ignores_stale_permission_dialog_above_live_prompt() {
        let mut claude = pane_output_status_pane(824, Provider::Claude, "Claude Code");
        claude.status = crate::app::PaneStatus::title(StatusKind::Idle);

        classify::apply_pane_output_status_fallback(
            &mut claude,
            "Do you want to proceed?\n\
             ❯ 1. Yes\n\
               2. No\n\
             Esc to cancel\n\
             ❯ Try \"fix the failing test\"\n\
             ? for shortcuts\n",
        );

        assert_eq!(claude.status.kind, StatusKind::Idle);
        assert_eq!(claude.status.source, crate::app::StatusSource::TmuxTitle);
    }

    #[test]
    fn claude_pane_output_marks_ask_user_question_waiting_with_wrapped_footer() {
        let mut claude = pane_output_status_pane(820, Provider::Claude, "Claude Code");
        claude.status = crate::app::PaneStatus::title(StatusKind::Idle);

        classify::apply_pane_output_status_fallback(
            &mut claude,
            "☐ Environment\n\
             \n\
             Which deployment environment should I use?\n\
             \n\
             ❯ 1. Staging\n\
               2. Production\n\
               3. Type something.\n\
             ────────────────────────────────────────\n\
               4. Chat about this\n\
             \n\
             Enter to select · Tab/Arrow keys to\n\
             navigate · Esc to cancel\n",
        );

        assert_eq!(claude.status.kind, StatusKind::Waiting);
        assert_eq!(claude.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn claude_pane_output_marks_ask_user_review_waiting() {
        let mut claude = pane_output_status_pane(821, Provider::Claude, "Claude Code");
        claude.status = crate::app::PaneStatus::title(StatusKind::Idle);

        classify::apply_pane_output_status_fallback(
            &mut claude,
            "←  ☒ Environment  ☒ Region  ✔ Submit  →\n\
             \n\
             Review your answers\n\
             \n\
              ● Which environment?\n\
                → Staging\n\
              ● Which region?\n\
                → US\n\
             \n\
             Ready to submit your answers?\n\
             \n\
             ❯ 1. Submit answers\n\
               2. Cancel\n",
        );

        assert_eq!(claude.status.kind, StatusKind::Waiting);
        assert_eq!(claude.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn claude_pane_output_ignores_stale_ask_dialog_above_live_prompt() {
        let mut claude = pane_output_status_pane(822, Provider::Claude, "Claude Code");
        claude.status = crate::app::PaneStatus::title(StatusKind::Idle);

        classify::apply_pane_output_status_fallback(
            &mut claude,
            "Which environment?\n\
               4. Chat about this\n\
             Enter to select · ↑/↓ to navigate · Esc to cancel\n\
             ╭────────────────────────────────────────╮\n\
             ❯ Try \"fix the failing test\"\n\
             ╰────────────────────────────────────────╯\n\
             ? for shortcuts\n",
        );

        assert_eq!(claude.status.kind, StatusKind::Idle);
        assert_eq!(claude.status.source, crate::app::StatusSource::TmuxTitle);
    }

    #[test]
    fn claude_pane_output_prefers_new_busy_frame_over_stale_ask_dialog() {
        let mut claude = pane_output_status_pane(825, Provider::Claude, "Claude Code");
        claude.status = crate::app::PaneStatus::title(StatusKind::Idle);

        classify::apply_pane_output_status_fallback(
            &mut claude,
            "Which environment?\n\
               4. Chat about this\n\
             Enter to select · ↑/↓ to navigate · Esc to cancel\n\
             ╭────────────────────────────────────────╮\n\
             ❯ \n\
             ╰────────────────────────────────────────╯\n\
             esc to interrupt\n",
        );

        assert_eq!(claude.status.kind, StatusKind::Busy);
        assert_eq!(claude.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn claude_pane_output_does_not_mark_generic_select_menu_waiting() {
        let mut claude = pane_output_status_pane(823, Provider::Claude, "Claude Code");
        claude.status = crate::app::PaneStatus::title(StatusKind::Idle);

        classify::apply_pane_output_status_fallback(
            &mut claude,
            "Choose a theme\n\
             ❯ 1. Dark\n\
               2. Light\n\
             Enter to select · ↑/↓ to navigate · Esc to cancel\n",
        );

        assert_eq!(claude.status.kind, StatusKind::Idle);
        assert_eq!(claude.status.source, crate::app::StatusSource::TmuxTitle);
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
