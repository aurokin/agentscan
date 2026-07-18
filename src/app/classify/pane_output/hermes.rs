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
        && [INTERRUPT_MARKER, QUEUE_MARKER, CANCEL_HINT]
            .iter()
            .any(|marker| line.contains(marker))
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

#[cfg(test)]
mod tests {
    use crate::app::classify;
    use crate::app::tests::{pane_output_status_pane, proc_fallback_pane};
    use crate::app::{Provider, StatusKind};

    #[test]
    fn hermes_pane_output_marks_busy_only_after_provider_is_known() {
        let mut hermes = pane_output_status_pane(765, Provider::Hermes, "agentscan: hermes");

        classify::apply_pane_output_status_fallback(
            &mut hermes,
            "╭─ task ─╮\n\
         │ working │\n\
         ╰─────────╯\n\
         ⚕ gpt-5.5 │ 65.4K/272K │ [██░░░░░░░░] 24% │ 2m │ ⏱ 1m 19s\n\
         ⚕ ❯ msg=interrupt · /queue · /bg · /steer · Ctrl+C cancel\n",
        );

        assert_eq!(hermes.status.kind, StatusKind::Busy);
        assert_eq!(hermes.status.source, crate::app::StatusSource::PaneOutput);

        let mut unknown = proc_fallback_pane(766, "python3.11", "agentscan: hermes");
        classify::apply_pane_output_status_fallback(
            &mut unknown,
            "⚕ gpt-5.5 │ 65.4K/272K │ [██░░░░░░░░] 24% │ 2m │ ⏱ 1m 19s\n\
         ⚕ ❯ msg=interrupt · /queue · /bg · /steer · Ctrl+C cancel\n",
        );

        assert_eq!(unknown.status.kind, StatusKind::Unknown);
        assert_eq!(unknown.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn hermes_pane_output_marks_current_prompt_idle() {
        let mut hermes = pane_output_status_pane(767, Provider::Hermes, "agentscan: hermes");

        classify::apply_pane_output_status_fallback(
            &mut hermes,
            "┌─────────────────────────────────────────────────────────────────────────────┐\n\
         │ Hermes Agent                                                                │\n\
         └─────────────────────────────────────────────────────────────────────────────┘\n\
         ⚕ gpt-5.5 │ ctx -- │ [░░░░░░░░░░] -- │ 2s │ ⏲ 0s\n\
         ❯\n",
        );

        assert_eq!(hermes.status.kind, StatusKind::Idle);
        assert_eq!(hermes.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn hermes_pane_output_accepts_one_busy_hint_in_provider_frame() {
        let mut hermes = pane_output_status_pane(773, Provider::Hermes, "agentscan: hermes");

        classify::apply_pane_output_status_fallback(
            &mut hermes,
            "⚕ gpt-5.5 │ 65.4K/272K │ [██░░░░░░░░] 24% │ 2m │ ⏱ 1m 19s\n\
         ⚕ ❯ /queue\n\
         ────────────────────────────────────────\n",
        );

        assert_eq!(hermes.status.kind, StatusKind::Busy);
        assert_eq!(hermes.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn hermes_pane_output_with_glyph_frame_but_no_busy_hint_stays_unknown() {
        let mut hermes = pane_output_status_pane(774, Provider::Hermes, "agentscan: hermes");

        classify::apply_pane_output_status_fallback(
            &mut hermes,
            "⚕ gpt-5.5 │ 65.4K/272K │ [██░░░░░░░░] 24% │ 2m │ ⏱ 1m 19s\n\
         ⚕ ❯ /bg · /steer\n\
         ────────────────────────────────────────\n",
        );

        assert_eq!(hermes.status.kind, StatusKind::Unknown);
        assert_eq!(hermes.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn hermes_pane_output_marks_idle_with_unsubmitted_draft_prompt() {
        // The user has typed a message but not submitted it: the agent is not running a turn, so the
        // honest label is idle even though the prompt is no longer a bare `❯`. The busy prompt is
        // `⚕ ❯ …` (leading `⚕`), so a `❯ <draft>` line cannot be mistaken for it.
        let mut hermes = pane_output_status_pane(769, Provider::Hermes, "agentscan: hermes");

        classify::apply_pane_output_status_fallback(
            &mut hermes,
            "⚕ gpt-5.5 │ ctx -- │ [░░░░░░░░░░] -- │ 5s │ ⏲ 0s │ ⚠ YOLO\n\
         ─────────────────────────────────────────────────────────────\n\
         ❯ Analyze the entire repo, tell me what you like, tell me what you don't\n\
         ─────────────────────────────────────────────────────────────\n",
        );

        assert_eq!(hermes.status.kind, StatusKind::Idle);
        assert_eq!(hermes.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn hermes_pane_output_marks_initializing_turn_busy() {
        let mut hermes = pane_output_status_pane(771, Provider::Hermes, "agentscan: hermes");

        classify::apply_pane_output_status_fallback(
            &mut hermes,
            "────────────────────────────────────────\n\
         ● Print exactly the marker formed by joining these parts with underscores\n\
         Initializing agent...\n\
         ────────────────────────────────────────\n",
        );

        assert_eq!(hermes.status.kind, StatusKind::Busy);
        assert_eq!(hermes.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn hermes_pane_output_uses_idle_prompt_below_stale_initializing_turn() {
        let mut hermes = pane_output_status_pane(772, Provider::Hermes, "agentscan: hermes");

        classify::apply_pane_output_status_fallback(
            &mut hermes,
            "────────────────────────────────────────\n\
         ● Print exactly the marker formed by joining these parts with underscores\n\
         Initializing agent...\n\
         ────────────────────────────────────────\n\
         ╭─ ⚕ Hermes ────────────────────────────╮\n\
             AGENTSCAN_E2E_DONE_hermes_123\n\
         ╰───────────────────────────────────────╯\n\
         ⚕ gpt-5.5 │ 16.4K/272K │ [█░░░░░░░░░] 6% │ 8s │ ⏲ 4s │ ⚠ YOLO\n\
         ────────────────────────────────────────\n\
         ❯\n\
         ────────────────────────────────────────\n",
        );

        assert_eq!(hermes.status.kind, StatusKind::Idle);
        assert_eq!(hermes.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn hermes_pane_output_does_not_infer_idle_from_stale_draft_prompt_in_scrollback() {
        // A `❯ …` draft prompt (with its status bar) sits in the scrollback capture, but the turn
        // ran and agent output scrolled below it with no current prompt/busy footer at the bottom.
        // The broadened `❯ <draft>` idle match must not resurrect that stale line — the prompt is
        // far from the current footer, so the pane stays unknown.
        let mut hermes = pane_output_status_pane(770, Provider::Hermes, "agentscan: hermes");

        classify::apply_pane_output_status_fallback(
            &mut hermes,
            "⚕ gpt-5.5 │ ctx -- │ [░░░░░░░░░░] -- │ 5s │ ⏲ 0s │ ⚠ YOLO\n\
         ─────────────────────────────────────────────────────────────\n\
         ❯ Analyze the entire repo, tell me what you like\n\
         ─────────────────────────────────────────────────────────────\n\
         ⚕ Reading src/app/classify/pane_output.rs\n\
         ⚕ Reading src/app/classify/provider_match.rs\n\
         ⚕ Grepping for hermes_idle_prompt_line\n\
         Found 3 matches across the classify module.\n\
         Next I will outline the strengths and weaknesses I see.\n\
         Starting with the daemon event loop and classification ladder.\n",
        );

        assert_eq!(hermes.status.kind, StatusKind::Unknown);
        assert_eq!(hermes.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn hermes_pane_output_does_not_infer_idle_from_prompt_like_line_with_prose_above() {
        // A status bar in scrollback followed by prose (`Run this:`) and a `❯ <command>` line is
        // agent output, not the live input box. Proximity alone would accept it; the intervening
        // line between the status bar and the prompt must be a box rule or blank.
        let mut hermes = pane_output_status_pane(772, Provider::Hermes, "agentscan: hermes");

        classify::apply_pane_output_status_fallback(
            &mut hermes,
            "⚕ gpt-5.5 │ ctx -- │ [░░░░░░░░░░] -- │ 5s │ ⏲ 0s │ ⚠ YOLO\n\
         Run this:\n\
         ❯ npm test\n",
        );

        assert_eq!(hermes.status.kind, StatusKind::Unknown);
        assert_eq!(hermes.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn hermes_pane_output_does_not_infer_idle_from_terminal_output_line_at_bottom() {
        // Agent output (e.g. a quoted shell prompt like `❯ npm test`) ends up as the last line of
        // the capture with a hermes status bar still sitting far above in scrollback. Nothing
        // follows the matched line so the current-frame guard trivially passes, but proximity to
        // the status bar must hold — an unrelated bottom line with no adjacent status bar is not
        // the live prompt.
        let mut hermes = pane_output_status_pane(771, Provider::Hermes, "agentscan: hermes");

        classify::apply_pane_output_status_fallback(
            &mut hermes,
            "⚕ gpt-5.5 │ ctx -- │ [░░░░░░░░░░] -- │ 5s │ ⏲ 0s │ ⚠ YOLO\n\
         ─────────────────────────────────────────────────────────────\n\
         ❯ Audit the build scripts\n\
         ─────────────────────────────────────────────────────────────\n\
         ⚕ Reading scripts/build.sh\n\
         ⚕ Reading scripts/test.sh\n\
         Run this to reproduce locally:\n\
         ❯ npm test\n",
        );

        assert_eq!(hermes.status.kind, StatusKind::Unknown);
        assert_eq!(hermes.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn hermes_pane_output_uses_current_prompt_over_stale_busy_footer() {
        let mut hermes = pane_output_status_pane(768, Provider::Hermes, "agentscan: hermes");

        classify::apply_pane_output_status_fallback(
            &mut hermes,
            "⚕ gpt-5.5 │ 65.4K/272K │ [██░░░░░░░░] 24% │ 2m │ ⏱ 1m 19s\n\
         ⚕ ❯ msg=interrupt · /queue · /bg · /steer · Ctrl+C cancel\n\
         \n\
         ⚕ Hermes\n\
         Done.\n\
         ⚕ gpt-5.5 │ 16K/272K │ [█░░░░░░░░░] 6% │ 6s │ ⏲ 3s\n\
         ❯\n",
        );

        assert_eq!(hermes.status.kind, StatusKind::Idle);
        assert_eq!(hermes.status.source, crate::app::StatusSource::PaneOutput);
    }
}
