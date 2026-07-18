use super::{PaneOutputFrame, StatusKind};

// kimi renders a bordered input box (`│ > … │`) at the current prompt in every state.
const INPUT_BOX_MARKER: &str = "│ >";
// While a turn runs, a spinner line (`<glyph> · <tip>`) is rendered directly above the
// input box. Observed moon-phase glyphs are confident busy signals; other non-ASCII
// spinner-shaped glyphs are guarded as ambiguous so a restyle cannot invert busy to idle.
const MOON_SPINNER_START: char = '\u{1f311}'; // 🌑
const MOON_SPINNER_END: char = '\u{1f318}'; // 🌘
// Lines to inspect above the current input box: top border, blank spacer, and the
// spinner line, with slack for the rotating tip text wrapping in narrow panes. Kimi
// erases the spinner on completion, so a live spinner is always bottom-pinned near the
// box; a moon line far above it is echoed output, not UI. Observed tips run ~70 chars,
// so 12 rows covers wrapping down to ~10-column panes — narrower than kimi's box UI
// can render. The window must stay bounded: widening it further trades a pathological
// false-idle for a far likelier false-busy from spinner-shaped text in scrollback.
const SPINNER_WINDOW: usize = 12;
// The current input box sits at most a bottom border plus the model/context footer
// lines above the end of the rendered frame; the bound carries slack for a footer row
// or two appearing in future releases. A box further up is stale scrollback (e.g.
// above an approval dialog that replaced the prompt) and must not drive status.
const INPUT_BOX_TAIL: usize = 8;
// Newer kimi releases render long tool/task turns with a Todo panel above the box and
// NO spinner; the busy evidence moves to the status footer below the box: a bracketed
// `[N task(s) running]` chip and a guidance hint ending "without waiting for the turn
// to finish" (observed live on K3, 2026-07-18). Both are scanned only below the
// current box, where echoed agent output cannot appear.
const TASK_RUNNING_CHIP: &str = "task running";
const TASKS_RUNNING_CHIP: &str = "tasks running";
const TURN_IN_PROGRESS_HINT: &str = "without waiting for the turn to finish";

pub(super) fn status(output: &str) -> Option<StatusKind> {
    let frame = PaneOutputFrame::new(output);
    let box_index = frame.rposition(kimi_input_box_line)?;
    if !frame.is_within_tail(box_index, INPUT_BOX_TAIL) {
        return None;
    }
    let current_prompt_lines = frame.window_ending_at(box_index, SPINNER_WINDOW)?;

    if current_prompt_lines
        .iter()
        .any(|line| kimi_moon_spinner_line(line))
    {
        return Some(StatusKind::Busy);
    }

    // Spinnerless busy: the footer below the current box carries the running-task
    // chip or the turn-in-progress hint while a turn executes tools in the
    // background. The box tail bound already proves these rows are current UI.
    if frame
        .lines_from(box_index)
        .is_some_and(|lines| lines.iter().any(|line| kimi_footer_busy_line(line)))
    {
        return Some(StatusKind::Busy);
    }

    // A bare moon glyph or an unrecognized non-ASCII `<glyph> · <tip>` line is
    // ambiguous: echoed output, or a future spinner restyle. Withhold status rather
    // than letting a decorative-string or glyph-range mismatch flip busy to idle.
    if current_prompt_lines
        .iter()
        .any(|line| kimi_moon_line(line) || kimi_unknown_spinner_shaped_line(line))
    {
        return None;
    }

    // The current input box is present with no live spinner above it. Other states
    // (approval dialogs, alternate UIs) drop the box and fall through to `None`.
    Some(StatusKind::Idle)
}

fn kimi_input_box_line(line: &str) -> bool {
    line.trim_start().starts_with(INPUT_BOX_MARKER)
}

// Busy evidence in the status footer: the turn-in-progress hint, or a
// `[N task(s) running]` chip. The chip match is bracket-scoped so bracketed
// prose elsewhere in the footer row cannot trip it accidentally.
fn kimi_footer_busy_line(line: &str) -> bool {
    if line.contains(TURN_IN_PROGRESS_HINT) {
        return true;
    }

    let mut rest = line;
    while let Some(start) = rest.find('[') {
        let after = &rest[start + 1..];
        let Some(end) = after.find(']') else {
            break;
        };
        let inside = &after[..end];
        if inside.contains(TASK_RUNNING_CHIP) || inside.contains(TASKS_RUNNING_CHIP) {
            return true;
        }
        rest = &after[end + 1..];
    }

    false
}

fn kimi_moon_spinner_line(line: &str) -> bool {
    // The live spinner renders as `<moon> · <tip text>`. The separator upgrades a moon
    // line to a confident busy signal; moon lines missing it fall to the ambiguity
    // check in `status` (unknown), never to idle, so a future separator restyle
    // degrades safely instead of inverting status.
    // Accepted residual risk: response text that literally echoes the full spinner shape
    // in its last lines is indistinguishable here — in this inline-scroll TUI the end of
    // a response occupies the same rows a live spinner would, so no positional check can
    // separate the two, and the glyph range, separator, and bounded tail-anchored window
    // already reflect every observed frame. Matches droid's spinner-anchor posture.
    let line = line.trim_start();
    let mut chars = line.chars();
    chars
        .next()
        .is_some_and(|ch| (MOON_SPINNER_START..=MOON_SPINNER_END).contains(&ch))
        && chars.as_str().starts_with(" · ")
}

fn kimi_moon_line(line: &str) -> bool {
    line.trim_start()
        .chars()
        .next()
        .is_some_and(|ch| (MOON_SPINNER_START..=MOON_SPINNER_END).contains(&ch))
}

fn kimi_unknown_spinner_shaped_line(line: &str) -> bool {
    let line = line.trim_start();
    let mut chars = line.chars();
    chars.next().is_some_and(|ch| {
        !ch.is_ascii()
            && !('\u{2500}'..='\u{257f}').contains(&ch)
            && !('\u{2580}'..='\u{259f}').contains(&ch)
    }) && chars.as_str().starts_with(" · ")
}

#[cfg(test)]
mod tests {
    use crate::app::classify;
    use crate::app::tests::{
        assert_pane_output_status, assert_unprovidered_pane_output_unchanged,
        pane_output_status_pane,
    };
    use crate::app::{Provider, StatusKind};

    #[test]
    fn kimi_code_pane_output_marks_spinnerless_task_running_footer_busy() {
        // Mirrors a real K3 frame (luma, 2026-07-18): a long tool turn renders a Todo
        // panel above the box with NO spinner; busy evidence is the footer's
        // `[1 task running]` chip and turn-in-progress hint below the box.
        let mut kimi = pane_output_status_pane(833, Provider::KimiCode, "Go through this");

        classify::apply_pane_output_status_fallback(
            &mut kimi,
            "  Todo\n\
         ✓ Item 3: dedupe rows\n\
         ● Item 5: adoption run, 70 min\n\
         ╭──────────────────────────────────────────────╮\n\
         │ >                                            │\n\
         ╰──────────────────────────────────────────────╯\n\
         yolo  K3 thinking: max  [1 task running]  ~/code/proj  main\n",
        );

        assert_eq!(kimi.status.kind, StatusKind::Busy);
        assert_eq!(kimi.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn kimi_code_pane_output_marks_turn_in_progress_hint_busy() {
        let mut kimi = pane_output_status_pane(834, Provider::KimiCode, "Go through this");

        classify::apply_pane_output_status_fallback(
            &mut kimi,
            "╭──────────────────────────────────────────────╮\n\
         │ >                                            │\n\
         ╰──────────────────────────────────────────────╯\n\
         ctrl-s to add guidance without waiting for the turn to finish\n",
        );

        assert_eq!(kimi.status.kind, StatusKind::Busy);
    }

    #[test]
    fn kimi_code_pane_output_ignores_task_running_prose_above_the_box() {
        // Bracketed chip text echoed in agent output above the box is content,
        // not footer chrome; the pane stays idle.
        let mut kimi = pane_output_status_pane(835, Provider::KimiCode, "notes");

        classify::apply_pane_output_status_fallback(
            &mut kimi,
            "● The dashboard shows [1 task running] when the worker is live.\n\n\
         ╭──────────────────────────────────────────────╮\n\
         │ >                                            │\n\
         ╰──────────────────────────────────────────────╯\n\
         K3 thinking: max  ~/code/proj  main\n",
        );

        assert_eq!(kimi.status.kind, StatusKind::Idle);
    }

    #[test]
    fn kimi_code_pane_output_marks_current_streaming_prompt_busy() {
        // Mirrors a real busy kimi frame (v0.27.0): a moon-phase spinner line with rotating tip
        // text sits directly above the input box while a turn runs.
        let mut kimi = pane_output_status_pane(830, Provider::KimiCode, "reply with exactly OK");

        classify::apply_pane_output_status_fallback(
            &mut kimi,
            "● OK\n\n\
         🌓 · Tip: ! to run a shell command\n\n\
         ╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ >                                                                            │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n\
         K2.7 Coding thinking  ~/code/agentscan  main                    ctrl+c: cancel\n\
         context: 9% (22k/256k)\n",
        );

        assert_eq!(kimi.status.kind, StatusKind::Busy);
        assert_eq!(kimi.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn kimi_code_pane_output_marks_wrapped_spinner_tip_busy_in_narrow_pane() {
        // In a narrow pane the rotating tip text wraps below the moon glyph, pushing the
        // spinner line several rows above the input box. The widened window must still see it.
        let mut kimi = pane_output_status_pane(837, Provider::KimiCode, "reply with exactly OK");

        classify::apply_pane_output_status_fallback(
            &mut kimi,
            "● OK\n\n\
         🌒 · Tip: ask Kimi to schedule\n\
         tasks, e.g. \"remind me at\n\
         5pm\"\n\n\
         ╭──────────────────────────────╮\n\
         │ >                            │\n\
         ╰──────────────────────────────╯\n\
         K2.7 Coding thinking  ~/code\n\
         context: 9% (22k/256k)\n",
        );

        assert_eq!(kimi.status.kind, StatusKind::Busy);
        assert_eq!(kimi.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn kimi_code_pane_output_marks_current_prompt_idle_only_after_provider_is_known() {
        let output = "╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │  Welcome to Kimi Code!                                                       │\n\
         │  Model:     K2.7 Coding                                                      │\n\
         │  Version:   0.27.0                                                           │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n\n\
         ● OK\n\n\
         ╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ >                                                                            │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n\
         K2.7 Coding thinking  ~/code/agentscan  main                    ctrl+c: cancel\n\
         context: 0% (0/256k)\n";
        assert_pane_output_status(
            831,
            Provider::KimiCode,
            "Kimi Code",
            output,
            StatusKind::Idle,
            crate::app::StatusSource::PaneOutput,
        );
        assert_unprovidered_pane_output_unchanged(832, "zsh", "custom title", output);
    }

    #[test]
    fn kimi_code_pane_output_ignores_stale_streaming_above_current_prompt() {
        // A moon glyph fossilized in scrollback (a spinner line that scrolled into history
        // during a long turn, with the turn's output below it) must not mark the fresh idle
        // prompt busy: the spinner window is anchored to the current input box.
        let mut kimi = pane_output_status_pane(833, Provider::KimiCode, "reply with exactly OK");

        classify::apply_pane_output_status_fallback(
            &mut kimi,
            "🌑 · Tip: ask Kimi to schedule tasks, e.g. \"remind me at 5pm\"\n\n\
         ● Done. The refactor is complete.\n\n\
         ● Updated src/lib.rs and src/main.rs.\n\n\
         ● Added the new classifier module.\n\n\
         ● Wired the provider registry entry.\n\n\
         ● Ran cargo fmt: no changes.\n\n\
         ● Ran cargo test: 42 passed.\n\n\
         ● Ran cargo clippy: clean.\n\n\
         ● All tests pass.\n\n\
         ╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ >                                                                            │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n\
         K2.7 Coding thinking  ~/code/agentscan  main                    ctrl+c: cancel\n\
         context: 9% (22k/256k)\n",
        );

        assert_eq!(kimi.status.kind, StatusKind::Idle);
        assert_eq!(kimi.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn kimi_code_pane_output_leaves_status_unknown_without_current_input_box() {
        // Unprobed UI states (approval dialogs, alternate screens) drop the input box; the
        // classifier must leave those Unknown rather than guessing.
        let mut kimi = pane_output_status_pane(834, Provider::KimiCode, "reply with exactly OK");

        classify::apply_pane_output_status_fallback(
            &mut kimi,
            "● Working through the plan.\n\n\
         K2.7 Coding thinking  ~/code/agentscan  main                    ctrl+c: cancel\n\
         context: 9% (22k/256k)\n",
        );

        assert_eq!(kimi.status.kind, StatusKind::Unknown);
        assert_eq!(kimi.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn kimi_code_pane_output_withholds_status_from_bare_moon_line_near_prompt() {
        // A moon-glyph line without the ` · ` separator is ambiguous: echoed output, or a
        // future release restyling the spinner tip. It must not report busy, and it must
        // not silently flip to idle either — status stays unknown.
        let mut kimi = pane_output_status_pane(836, Provider::KimiCode, "reply with exactly OK");

        classify::apply_pane_output_status_fallback(
            &mut kimi,
            "🌑 marks a new moon in the schedule output.\n\n\
         ╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ >                                                                            │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n\
         K2.7 Coding thinking  ~/code/agentscan  main                    ctrl+c: cancel\n\
         context: 9% (22k/256k)\n",
        );

        assert_eq!(kimi.status.kind, StatusKind::Unknown);
        assert_eq!(kimi.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn kimi_code_pane_output_withholds_idle_from_braille_spinner_shape() {
        let mut kimi = pane_output_status_pane(838, Provider::KimiCode, "reply with exactly OK");

        classify::apply_pane_output_status_fallback(
            &mut kimi,
            "● Still working.\n\n\
         ⠋ · Tip: use @ to include files\n\n\
         ╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ >                                                                            │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n\
         K2.7 Coding thinking  ~/code/agentscan  main                    ctrl+c: cancel\n\
         context: 9% (22k/256k)\n",
        );

        assert_eq!(kimi.status.kind, StatusKind::Unknown);
        assert_eq!(kimi.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn kimi_code_pane_output_withholds_status_from_unknown_spinner_glyph() {
        let mut kimi = pane_output_status_pane(839, Provider::KimiCode, "reply with exactly OK");

        classify::apply_pane_output_status_fallback(
            &mut kimi,
            "◆ · Tip: future spinner style\n\n\
         ╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ >                                                                            │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n\
         K2.7 Coding thinking  ~/code/agentscan  main                    ctrl+c: cancel\n\
         context: 9% (22k/256k)\n",
        );

        assert_eq!(kimi.status.kind, StatusKind::Unknown);
        assert_eq!(kimi.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn kimi_code_pane_output_ignores_stale_input_box_above_replacement_ui() {
        // When an approval dialog (or another alternate UI) replaces the prompt, the last
        // input box survives in scrollback well above the bottom of the frame. The stale box
        // must not be classified as the current prompt; status stays unknown.
        let mut kimi = pane_output_status_pane(835, Provider::KimiCode, "reply with exactly OK");

        classify::apply_pane_output_status_fallback(
            &mut kimi,
            "╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ >                                                                            │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n\n\
         ● Running the migration now.\n\n\
         Allow Kimi to run this command?\n\n\
         rm -rf build/\n\n\
         1. Yes\n\
         2. No, tell Kimi what to do differently\n",
        );

        assert_eq!(kimi.status.kind, StatusKind::Unknown);
        assert_eq!(kimi.status.source, crate::app::StatusSource::NotChecked);
    }
}
