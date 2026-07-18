use super::{PaneOutputFrame, StatusKind};

// droid idle footer hint shown at the bottom of the input box.
const HELP_HINT: &str = "? for help";
// droid footer shown after an IDE/TMUX integration reload (`… ready … restart to apply`).
const READY_MARKER: &str = " ready ";
const RESTART_MARKER: &str = "restart to apply";
// droid busy prompt shown while a turn can be steered.
const STEER_PROMPT_MARKER: &str = "> Enter to steer";
const STEER_HINT: &str = "Enter to steer";
// droid streaming status stop hint (rendered after a leading spinner glyph).
const STOP_HINT: &str = "Press ESC to stop";

pub(super) fn status(output: &str) -> Option<StatusKind> {
    let frame = PaneOutputFrame::new(output);
    let footer_index = frame.rposition(droid_current_footer_line);
    let input_box_index = frame
        .rposition(droid_input_box_row)
        .filter(|&index| frame.is_within_tail(index, 8));
    let current_frame_index = footer_index.max(input_box_index)?;
    let current_prompt_lines = frame.window_ending_at(current_frame_index, 8)?;

    if current_prompt_lines
        .iter()
        .any(|line| droid_current_busy_prompt_line(line))
    {
        return Some(StatusKind::Busy);
    }

    if current_prompt_lines
        .iter()
        .any(|line| droid_current_idle_prompt_line(line))
    {
        return Some(StatusKind::Idle);
    }

    current_prompt_lines
        .iter()
        .any(|line| droid_current_streaming_line(line))
        .then_some(StatusKind::Busy)
}

fn droid_input_box_row(line: &str) -> bool {
    line.trim_start().starts_with("│ >")
}

fn droid_current_footer_line(line: &str) -> bool {
    let line = line.trim();
    (line.contains(HELP_HINT)
        && line
            .split_whitespace()
            .any(|token| matches!(token, "IDE" | "TMUX")))
        || (line.contains(READY_MARKER)
            && line.contains(RESTART_MARKER)
            && line.split_whitespace().any(|token| token == "TMUX"))
}

fn droid_current_busy_prompt_line(line: &str) -> bool {
    let line = line.trim();
    line.contains(STEER_PROMPT_MARKER)
}

fn droid_current_streaming_line(line: &str) -> bool {
    // droid's streaming status line is `<spinner> <verb>…  (Press ESC to stop)`, where the verb
    // varies across a turn (`Streaming…`, `Invoking tools…`, `Thinking…`). Anchor on the live
    // braille spinner glyph plus the verb-agnostic stop hint, so prose that merely contains
    // "Press ESC to stop" (without the leading spinner) is not mistaken for an active turn.
    let line = line.trim_start();
    line.chars()
        .next()
        .is_some_and(|ch| ('\u{2800}'..='\u{28ff}').contains(&ch))
        && line.contains(STOP_HINT)
}

fn droid_current_idle_prompt_line(line: &str) -> bool {
    let line = line.trim();
    droid_input_box_row(line) && !line.contains(STEER_HINT)
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
    fn droid_pane_output_marks_current_prompt_idle_only_after_provider_is_known() {
        let output = " Auto (High) - allow all commands            Droid Core (DeepSeek V4 Pro) (Max)\n\
         ╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ >                                                                            │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n\
         [⏱ 5s] ? for help                                                          IDE ◌\n";
        assert_pane_output_status(
            810,
            Provider::Droid,
            "⛬ New Session",
            output,
            StatusKind::Idle,
            crate::app::StatusSource::PaneOutput,
        );
        assert_unprovidered_pane_output_unchanged(811, "zsh", "custom title", output);
    }

    #[test]
    fn droid_pane_output_marks_current_tmux_footer_idle() {
        let output = "                       █████████    █████████     ████████    ███   █████████\n\
         \n\
                                  v0.156.2 (ctrl+j for changelog)\n\
         \n\
                    TIP: Use /context to see your context window usage breakdown\n\
         \n\
                         shift+tab to cycle modes · ctrl+N to cycle models\n\
                              ctrl+L for autonomy · tab for reasoning\n\
         \n\
                               Skills (21) ✓  MCPs (0) ✗  AGENTS.md ✓\n\
         \n\
         \n\
         Auto (High) · allow all commands                                       Droid Core (GLM-5.2) (High)\n\
         ╭──────────────────────────────────────────────────────────────────────────────────────────────────╮\n\
         │ > Try \"Review the changes in my current branch\"                                                  │\n\
         ╰──────────────────────────────────────────────────────────────────────────────────────────────────╯\n\
         ? for help                                                                                    TMUX ⧉\n";

        assert_pane_output_status(
            820,
            Provider::Droid,
            "⛬ New Session",
            output,
            StatusKind::Idle,
            crate::app::StatusSource::PaneOutput,
        );
    }

    #[test]
    fn droid_pane_output_uses_input_box_when_badge_is_renamed() {
        // The input-box row is the durable frame anchor; the integration badge is mutable copy.
        let mut droid = pane_output_status_pane(822, Provider::Droid, "⛬ New Session");

        classify::apply_pane_output_status_fallback(
            &mut droid,
            "⠹ Thinking...  (Press ESC to stop)\n\
         Auto (High) · allow all commands                         Droid Core (GLM-5.2) (High)\n\
         ╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ > Enter to steer                                                             │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n\
         [⏱ 9s] ? for help                                                        SHELL ◌\n",
        );

        assert_eq!(droid.status.kind, StatusKind::Busy);
        assert_eq!(droid.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn droid_pane_output_without_badge_or_input_box_stays_unknown() {
        let mut droid = pane_output_status_pane(823, Provider::Droid, "⛬ New Session");

        classify::apply_pane_output_status_fallback(
            &mut droid,
            "The documentation says > Enter to steer while a request is active.\n\
         [⏱ 9s] ? for help                                                        SHELL ◌\n",
        );

        assert_eq!(droid.status.kind, StatusKind::Unknown);
        assert_eq!(droid.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn droid_pane_output_marks_update_ready_tmux_footer_idle() {
        // Observed from Droid v0.156.2: the current prompt is followed by an update-ready footer
        // rather than the older `? for help` footer. The prompt box still anchors the live frame.
        let output = "Auto (High) · allow all commands                         Droid Core (GLM-5.2) (High)\n\
         ╭──────────────────────────────────────────────────────────────────────────────────────────────────╮\n\
         │ > Try \"How do I handle errors in async functions?\"                                               │\n\
         ╰──────────────────────────────────────────────────────────────────────────────────────────────────╯\n\
         ✓ v0.159.1 ready (restart to apply)                                                          TMUX ⧉\n";

        assert_pane_output_status(
            821,
            Provider::Droid,
            "⛬ New Session",
            output,
            StatusKind::Idle,
            crate::app::StatusSource::PaneOutput,
        );
    }

    #[test]
    fn droid_pane_output_marks_current_steer_prompt_busy() {
        // Mirrors a real busy droid frame (v0.134.0): the input box prompt switches to
        // "Enter to steer" during a turn, with a streaming line above it whose verb varies
        // ("Invoking tools…" here, not "Streaming…").
        let mut droid = pane_output_status_pane(812, Provider::Droid, "⛬ New Session");

        classify::apply_pane_output_status_fallback(
            &mut droid,
            "   Analyze this entire codebase and tell me about it\n\n\
         Plan · 0/7\n\
         ┃ ● Explore project structure and core files\n\n\
         ⠟ Invoking tools...  (Press ESC to stop)\n\n\
         Auto (High) - allow all commands            Droid Core (Kimi K2.6) (High)\n\
         ╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ > Enter to steer                                                             │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n\
         [⏱ 38s] ? for help                                                         IDE ◌\n",
        );

        assert_eq!(droid.status.kind, StatusKind::Busy);
        assert_eq!(droid.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn droid_pane_output_marks_streaming_busy_when_verb_is_not_streaming() {
        // The streaming fallback must recognize droid's varying verbs by the stop hint, not the
        // word "Streaming". Here the current frame has no steer/idle prompt in the box window, so
        // the streaming line is the deciding busy signal.
        let mut droid = pane_output_status_pane(814, Provider::Droid, "⛬ New Session");

        classify::apply_pane_output_status_fallback(
            &mut droid,
            "   Read the source and summarize\n\n\
         ⠹ Thinking...  (Press ESC to stop)\n\n\
         Auto (High) - allow all commands            Droid Core (Kimi K2.6) (High)\n\
         [⏱ 9s] ? for help                                                          IDE ◌\n",
        );

        assert_eq!(droid.status.kind, StatusKind::Busy);
        assert_eq!(droid.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn droid_pane_output_does_not_mark_busy_from_prose_stop_hint_without_spinner() {
        // The streaming fallback requires the live braille spinner glyph, not a bare "Press ESC to
        // stop" substring. Model output that mentions the phrase in prose (no leading spinner) sits
        // in the current frame with no steer/idle prompt, so the pane must not be marked busy.
        let mut droid = pane_output_status_pane(815, Provider::Droid, "⛬ New Session");

        classify::apply_pane_output_status_fallback(
            &mut droid,
            "   To cancel a running job, press ESC. The banner reads: Press ESC to stop\n\n\
         Auto (High) - allow all commands            Droid Core (Kimi K2.6) (High)\n\
         [⏱ 9s] ? for help                                                          IDE ◌\n",
        );

        assert_eq!(droid.status.kind, StatusKind::Unknown);
        assert_eq!(droid.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn droid_pane_output_ignores_stale_streaming_above_current_prompt() {
        let mut droid = pane_output_status_pane(813, Provider::Droid, "⛬ Basic Math Question");

        classify::apply_pane_output_status_fallback(
            &mut droid,
            " ⠄ Streaming...  (Press ESC to stop)\n\n\
         ⛬  2.\n\n\
         Auto (High) - allow all commands            Droid Core (DeepSeek V4 Pro) (Max)\n\
         ╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ >                                                                            │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n\
         [⏱ 5s] ? for help                                                          IDE ◌\n",
        );

        assert_eq!(droid.status.kind, StatusKind::Idle);
        assert_eq!(droid.status.source, crate::app::StatusSource::PaneOutput);
    }
}
