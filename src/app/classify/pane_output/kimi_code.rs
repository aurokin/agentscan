use super::{PaneOutputFrame, StatusKind};

// kimi renders a bordered input box (`│ > … │`) at the current prompt in every state.
const INPUT_BOX_MARKER: &str = "│ >";
// While a turn runs, the activity pane renders a spinner before the editor chrome.
// Observed moon-phase glyphs are confident busy signals; other non-ASCII spinner-shaped
// glyphs are guarded as ambiguous so a restyle cannot invert busy to idle.
const MOON_SPINNER_START: char = '\u{1f311}'; // 🌑
const MOON_SPINNER_END: char = '\u{1f318}'; // 🌘
// Kimi 0.29.1 uses these exact braille frames plus the `working...` label while composing.
const BRAILLE_SPINNER_FRAMES: &str = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏";
const COMPOSING_SPINNER_LABEL: &str = "working...";
// Lines to inspect above the current input box when no Todo panel separates it from
// the activity pane: top border, blank spacer, and the spinner line, with slack for
// rotating tip text wrapping in narrow panes. The window stays bounded so old
// spinner-shaped transcript text cannot become current busy evidence.
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
const TODO_PANEL_HEADER: &str = "Todo";
const TODO_COLLAPSED_MAX_VISIBLE: usize = 5;

pub(super) fn status(output: &str) -> Option<StatusKind> {
    let frame = PaneOutputFrame::new(output);
    let box_index = frame.rposition(kimi_input_box_line)?;
    if !frame.is_within_tail(box_index, INPUT_BOX_TAIL) {
        return None;
    }
    let current_prompt_lines = frame.window_ending_at(box_index, SPINNER_WINDOW)?;
    let todo_divider_index = kimi_current_todo_divider_index(&frame, box_index);
    let todo_spinner_line =
        todo_divider_index.and_then(|index| kimi_todo_spinner_line(&frame, index));

    let spinner_busy = todo_spinner_line.is_some_and(kimi_busy_spinner_line)
        || (todo_divider_index.is_none()
            && (current_prompt_lines
                .iter()
                .any(|line| kimi_moon_spinner_line(line))
                || kimi_prompt_attached_composing_spinner(&frame, box_index)));
    if spinner_busy {
        return Some(StatusKind::Busy);
    }

    // Spinnerless busy: the footer below the current box carries the running-task
    // chip or the turn-in-progress hint while a turn executes tools in the
    // background. Scan only past the box's closing border so typed prompt text
    // inside the box can never masquerade as footer chrome; the box tail bound
    // already proves these rows are current UI.
    let footer_busy = frame.lines_from(box_index).is_some_and(|lines| {
        lines
            .iter()
            .position(|line| kimi_box_bottom_border_line(line))
            .is_some_and(|offset| {
                lines[offset + 1..]
                    .iter()
                    .any(|line| kimi_footer_busy_line(line))
            })
    });
    if footer_busy {
        return Some(StatusKind::Busy);
    }

    // A bare moon glyph or an unrecognized non-ASCII `<glyph> · <tip>` line is
    // ambiguous: echoed output, or a future spinner restyle. Withhold status rather
    // than letting a decorative-string or glyph-range mismatch flip busy to idle.
    let ambiguous_spinner = todo_spinner_line.is_some_and(kimi_ambiguous_spinner_line)
        || (todo_divider_index.is_none()
            && current_prompt_lines
                .iter()
                .any(|line| kimi_ambiguous_spinner_line(line)));
    if ambiguous_spinner {
        return None;
    }

    // The current input box is present with no live spinner above it. Other states
    // (approval dialogs, alternate UIs) drop the box and fall through to `None`.
    Some(StatusKind::Idle)
}

fn kimi_input_box_line(line: &str) -> bool {
    line.trim_start().starts_with(INPUT_BOX_MARKER)
}

fn kimi_input_box_top_border_line(line: &str) -> bool {
    line.trim_start().starts_with('╭')
}

fn kimi_box_bottom_border_line(line: &str) -> bool {
    line.trim_start().starts_with('╰')
}

fn kimi_current_todo_divider_index(frame: &PaneOutputFrame<'_>, box_index: usize) -> Option<usize> {
    // Upstream mounts Todo directly before the editor. Prove that relationship by
    // walking upward from the input box through the exact Todo-owned row indentation,
    // then requiring its header and divider. A historical Todo block followed by
    // transcript output cannot be selected by an unbounded search.
    let box_top_index = box_index.checked_sub(1)?;
    let box_top_line = frame.line(box_top_index)?;
    if !kimi_input_box_top_border_line(box_top_line) {
        return None;
    }

    let lines_before_box = frame.lines_before(box_top_index)?;
    let panel_indent = leading_spaces(box_top_line);
    let body_indent = panel_indent.checked_add(2)?;
    let mut cursor = lines_before_box.len();
    while cursor > 0 && kimi_todo_panel_body_line(lines_before_box[cursor - 1], body_indent) {
        cursor -= 1;
    }
    if cursor == lines_before_box.len() {
        return None;
    }
    if !kimi_todo_panel_body_is_complete(&lines_before_box[cursor..], body_indent) {
        return None;
    }

    let header_index = cursor.checked_sub(1)?;
    let header = lines_before_box[header_index];
    if leading_spaces(header) != body_indent || header.trim() != TODO_PANEL_HEADER {
        return None;
    }

    let divider_index = header_index.checked_sub(1)?;
    let divider = lines_before_box[divider_index];
    (leading_spaces(divider) == panel_indent && kimi_todo_panel_divider_line(divider))
        .then_some(divider_index)
}

fn kimi_todo_spinner_line<'a>(
    frame: &PaneOutputFrame<'a>,
    divider_index: usize,
) -> Option<&'a str> {
    // The activity pane is immediately before Todo. Search its bounded tail so an
    // older wrapped tip remains visible, but require every continuation through the
    // divider to be nonblank. Idle renders a blank spacer here, which rejects stale
    // spinner-shaped transcript content above it.
    let activity_tail = frame.window_before(divider_index, SPINNER_WINDOW)?;
    let spinner_offset = activity_tail
        .iter()
        .rposition(|line| kimi_ambiguous_spinner_line(line))?;
    activity_tail[spinner_offset + 1..]
        .iter()
        .all(|line| !line.trim().is_empty())
        .then_some(activity_tail[spinner_offset])
}

fn kimi_todo_panel_divider_line(line: &str) -> bool {
    let line = line.trim();
    line.chars().count() >= 10 && line.chars().all(|ch| ch == '─')
}

fn kimi_todo_panel_body_line(line: &str, expected_indent: usize) -> bool {
    // Upstream maps every Todo row through `truncateToWidth`; item titles never
    // produce continuation rows. Accepting arbitrary indented text here would let
    // transcript output bridge a stale Todo block to the current editor.
    if leading_spaces(line) != expected_indent {
        return false;
    }
    let line = line.trim_start();
    line.starts_with("● ")
        || line.starts_with("✓ ")
        || line.starts_with("○ ")
        || (line.starts_with('…') && line.contains("ctrl+t to expand"))
        || (line.starts_with("all ") && line.contains("ctrl+t to collapse"))
}

fn kimi_todo_panel_body_is_complete(lines: &[&str], expected_indent: usize) -> bool {
    let Some((last, preceding)) = lines.split_last() else {
        return false;
    };
    let last = last.trim_start();

    if last.starts_with("all ") && last.contains("ctrl+t to collapse") {
        let Some(total) = last
            .strip_prefix("all ")
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|count| count.parse::<usize>().ok())
        else {
            return false;
        };
        return total > TODO_COLLAPSED_MAX_VISIBLE
            && preceding.len() == total
            && preceding
                .iter()
                .all(|line| kimi_todo_item_line(line, expected_indent));
    }

    if last.starts_with('…') && last.contains("ctrl+t to expand") {
        return preceding.len() == TODO_COLLAPSED_MAX_VISIBLE
            && preceding
                .iter()
                .all(|line| kimi_todo_item_line(line, expected_indent));
    }

    lines.len() <= TODO_COLLAPSED_MAX_VISIBLE
        && lines
            .iter()
            .all(|line| kimi_todo_item_line(line, expected_indent))
}

fn kimi_todo_item_line(line: &str, expected_indent: usize) -> bool {
    if leading_spaces(line) != expected_indent {
        return false;
    }
    let line = line.trim_start();
    line.starts_with("● ") || line.starts_with("✓ ") || line.starts_with("○ ")
}

fn leading_spaces(line: &str) -> usize {
    line.chars().take_while(|ch| *ch == ' ').count()
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

fn kimi_busy_spinner_line(line: &str) -> bool {
    kimi_moon_spinner_line(line) || kimi_composing_spinner_line(line)
}

fn kimi_prompt_attached_composing_spinner(frame: &PaneOutputFrame<'_>, box_index: usize) -> bool {
    box_index
        .checked_sub(2)
        .and_then(|index| frame.line(index))
        .is_some_and(kimi_composing_spinner_line)
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

fn kimi_composing_spinner_line(line: &str) -> bool {
    let line = line.trim_start();
    let mut chars = line.chars();
    chars.next().is_some_and(kimi_braille_spinner_glyph)
        && chars
            .as_str()
            .strip_prefix(' ')
            .is_some_and(|label| label.starts_with(COMPOSING_SPINNER_LABEL))
}

fn kimi_moon_line(line: &str) -> bool {
    line.trim_start()
        .chars()
        .next()
        .is_some_and(|ch| (MOON_SPINNER_START..=MOON_SPINNER_END).contains(&ch))
}

fn kimi_braille_line(line: &str) -> bool {
    line.trim_start()
        .chars()
        .next()
        .is_some_and(kimi_braille_spinner_glyph)
}

fn kimi_braille_spinner_glyph(ch: char) -> bool {
    BRAILLE_SPINNER_FRAMES.contains(ch)
}

fn kimi_ambiguous_spinner_line(line: &str) -> bool {
    kimi_moon_line(line) || kimi_braille_line(line) || kimi_unknown_spinner_shaped_line(line)
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
    fn kimi_code_pane_output_ignores_task_running_text_typed_into_the_box() {
        // Chip/hint text inside the input box is user input, not footer chrome;
        // the footer scan must start past the closing border.
        let mut kimi = pane_output_status_pane(836, Provider::KimiCode, "notes");

        classify::apply_pane_output_status_fallback(
            &mut kimi,
            "╭──────────────────────────────────────────────────────────╮\n\
         │ > why does it say [1 task running] without waiting for   │\n\
         │   the turn to finish?                                    │\n\
         ╰──────────────────────────────────────────────────────────╯\n\
         K3 thinking: max  ~/code/proj  main\n",
        );

        assert_eq!(kimi.status.kind, StatusKind::Idle);
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
    fn kimi_code_pane_output_marks_v029_composing_spinner_busy() {
        // Kimi 0.29.1 uses a braille `working...` spinner while composing instead of
        // the moon spinner used for waiting/tool activity.
        let mut kimi = pane_output_status_pane(840, Provider::KimiCode, "write the report");

        classify::apply_pane_output_status_fallback(
            &mut kimi,
            "⠼ working... · Tip: /sessions to browse earlier sessions\n\
         ╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ >                                                                            │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n\
         yolo  K3 thinking: high  ~/code/agentscan  main\n\
         context: 9% (83.8k/1M)\n",
        );

        assert_eq!(kimi.status.kind, StatusKind::Busy);
        assert_eq!(kimi.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn kimi_code_pane_output_withholds_busy_from_detached_composing_text() {
        // Idle activity leaves a blank spacer before the editor. An exact
        // spinner-shaped transcript line above that spacer is ambiguous, not busy.
        let mut kimi = pane_output_status_pane(845, Provider::KimiCode, "write the report");

        classify::apply_pane_output_status_fallback(
            &mut kimi,
            "⠼ working... was the literal output under test\n\
         \n\
         ╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ >                                                                            │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n\
         yolo  K3 thinking: high  ~/code/agentscan  main\n\
         context: 9% (83.8k/1M)\n",
        );

        assert_eq!(kimi.status.kind, StatusKind::Unknown);
        assert_eq!(kimi.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn kimi_code_pane_output_marks_spinner_above_expanded_todo_busy() {
        // The activity pane precedes the Todo panel. Expanded mode renders every item,
        // so the live spinner can sit arbitrarily farther than SPINNER_WINDOW above the
        // current input box while remaining directly attached to the Todo divider. Both
        // current activity styles share that layout.
        for spinner in [
            "⠼ working... · Tip: /sessions to browse earlier sessions",
            "🌑 · Tip: /goal for multi-step work",
            "🌒 · Tip: ask Kimi to schedule\n           tasks, e.g. \"remind me at\n           5pm\"",
        ] {
            let mut kimi = pane_output_status_pane(841, Provider::KimiCode, "write the report");
            let output = format!(
                "{spinner}\n\
         ────────────────────────────────────────────────────────────────────────────────\n\
         \x20\x20Todo\n\
         \x20\x20● Phase 1\n\
         \x20\x20○ Phase 2\n\
         \x20\x20○ Phase 3\n\
         \x20\x20○ Phase 4\n\
         \x20\x20○ Phase 5\n\
         \x20\x20○ Phase 6\n\
         \x20\x20○ Phase 7\n\
         \x20\x20○ Phase 8\n\
         \x20\x20○ Phase 9\n\
         \x20\x20○ Phase 10\n\
         \x20\x20○ Phase 11\n\
         \x20\x20○ Phase 12\n\
         \x20\x20○ Phase 13\n\
         \x20\x20○ Phase 14\n\
         \x20\x20all 14 items · ctrl+t to collapse\n\
         ╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ >                                                                            │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n\
         yolo  K3 thinking: high  ~/code/agentscan  main\n\
         context: 9% (83.8k/1M)\n"
            );
            classify::apply_pane_output_status_fallback(&mut kimi, &output);

            assert_eq!(kimi.status.kind, StatusKind::Busy, "spinner: {spinner}");
            assert_eq!(
                kimi.status.source,
                crate::app::StatusSource::PaneOutput,
                "spinner: {spinner}"
            );
        }
    }

    #[test]
    fn kimi_code_pane_output_ignores_detached_spinner_before_idle_todo() {
        // Idle activity renders a blank spacer before the persistent Todo panel. A
        // spinner-shaped transcript line above that spacer is stale, not live chrome.
        let mut kimi = pane_output_status_pane(842, Provider::KimiCode, "write the report");

        classify::apply_pane_output_status_fallback(
            &mut kimi,
            "⠼ working... was the literal output under test\n\
         \n\
         ────────────────────────────────────────────────────────────────────────────────\n\
         \x20\x20Todo\n\
         \x20\x20✓ Phase 1\n\
         \x20\x20○ Phase 2\n\
         ╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ >                                                                            │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n\
         yolo  K3 thinking: high  ~/code/agentscan  main\n\
         context: 9% (83.8k/1M)\n",
        );

        assert_eq!(kimi.status.kind, StatusKind::Idle);
        assert_eq!(kimi.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn kimi_code_pane_output_ignores_stale_todo_panel_above_idle_prompt() {
        // A historical Todo block can remain in the visible transcript after the
        // current panel disappears. It is not attached to the current editor when
        // ordinary response rows intervene, even if those bullets share Todo's indent.
        let mut kimi = pane_output_status_pane(843, Provider::KimiCode, "write the report");

        classify::apply_pane_output_status_fallback(
            &mut kimi,
            "⠼ working... · Tip: old activity\n\
         ────────────────────────────────────────────────────────────────────────────────\n\
         \x20\x20Todo\n\
         \x20\x20✓ Old phase\n\
         \x20\x20● The old Todo panel is complete.\n\
         \x20\x20● Added the report.\n\
         \x20\x20● Ran formatting.\n\
         \x20\x20● Ran tests.\n\
         \x20\x20● Checked the output.\n\
         \x20\x20● Verified the snapshot.\n\
         \x20\x20● Updated the summary.\n\
         \x20\x20● Closed the task.\n\
         \x20\x20● Everything is ready.\n\
         \x20\x20● Handing control back now.\n\
         \n\
         ╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ >                                                                            │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n\
         yolo  K3 thinking: high  ~/code/agentscan  main\n\
         context: 9% (83.8k/1M)\n",
        );

        assert_eq!(kimi.status.kind, StatusKind::Idle);
        assert_eq!(kimi.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn kimi_code_pane_output_withholds_busy_from_short_stale_todo_bridge() {
        // Even a short run of Todo-indented transcript bullets is detached from the
        // editor by Kimi's idle activity spacer. It may stay ambiguous, but never busy.
        let mut kimi = pane_output_status_pane(846, Provider::KimiCode, "write the report");

        classify::apply_pane_output_status_fallback(
            &mut kimi,
            "⠼ working... · Tip: old activity\n\
         ────────────────────────────────────────────────────────────────────────────────\n\
         \x20\x20Todo\n\
         \x20\x20✓ Old phase\n\
         \x20\x20● Transcript bullet one\n\
         \x20\x20● Transcript bullet two\n\
         \n\
         ╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ >                                                                            │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n\
         yolo  K3 thinking: high  ~/code/agentscan  main\n\
         context: 9% (83.8k/1M)\n",
        );

        assert_eq!(kimi.status.kind, StatusKind::Unknown);
        assert_eq!(kimi.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn kimi_code_pane_output_uses_live_spinner_below_stale_todo_panel() {
        // A stale Todo-shaped transcript block must not suppress the ordinary bounded
        // scan when a new live spinner is attached directly to the current editor.
        let mut kimi = pane_output_status_pane(844, Provider::KimiCode, "write the report");

        classify::apply_pane_output_status_fallback(
            &mut kimi,
            "⠼ working... · Tip: old activity\n\
         ────────────────────────────────────────────────────────────────────────────────\n\
         \x20\x20Todo\n\
         \x20\x20✓ Old phase\n\
         ● The old panel is complete.\n\
         \n\
         🌑 · Tip: current activity\n\
         ╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ >                                                                            │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n\
         yolo  K3 thinking: high  ~/code/agentscan  main\n\
         context: 9% (83.8k/1M)\n",
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
