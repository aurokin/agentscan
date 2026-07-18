use super::{PaneOutputFrame, StatusKind, is_version_like_command};

// opencode idle input placeholders (older placeholder-row build).
const OPENCODE_ASK_PLACEHOLDER: &str = "Ask anything... \"";
const OPENCODE_COMMAND_PLACEHOLDER: &str = "Run a command... \"";
// opencode `● Tip` notice marker rendered below the input box.
const OPENCODE_TIP_MARKER: &str = "● Tip";
// opencode command-bar footer hints (`tab agents  ctrl+p commands`).
const OPENCODE_COMMAND_BAR_TAB_HINT: &str = "tab agents";
const OPENCODE_COMMAND_BAR_COMMANDS_HINT: &str = "ctrl+p commands";
// opencode pinned bottom status-bar brand marker (`• OpenCode <ver>`).
const OPENCODE_BRAND_MARKER: &str = "• OpenCode";
// opencode interrupt hint shown while a turn is running (`esc interrupt`); two substrings.
const OPENCODE_INTERRUPT_ESC_MARKER: &str = "esc";
const OPENCODE_INTERRUPT_VERB_MARKER: &str = "interrupt";
// opencode permission-prompt copy shown while awaiting the user.
const OPENCODE_PERMISSION_REQUIRED_MARKER: &str = "Permission required";
const OPENCODE_REJECT_PERMISSION_MARKER: &str = "Reject permission";
const OPENCODE_ALLOW_ONCE_MARKER: &str = "Allow once";
const OPENCODE_ALLOW_ALWAYS_MARKER: &str = "Allow always";
const OPENCODE_PERMISSION_MARKER: &str = "Permission";
// opencode question-prompt copy shown while awaiting the user.
const OPENCODE_REJECT_QUESTION_MARKER: &str = "Reject question";
const OPENCODE_WAITING_QUESTION_MARKER: &str = "Waiting for question event";
const OPENCODE_QUESTIONS_MARKER: &str = "# Questions";

pub(super) fn status(output: &str) -> Option<StatusKind> {
    let frame = PaneOutputFrame::new(output);

    // Bottom-most current idle prompt: either the placeholder input row (older build) near
    // the current footer, or the newer build's command-bar input box. Folding both into one
    // index lets a busy marker only win when it is below the live prompt — a stale
    // approval/question row above the current prompt must not force busy.
    let placeholder_index = frame
        .rposition(opencode_idle_prompt_line)
        .filter(|&index| opencode_prompt_is_near_current_footer(&frame, index));
    let command_bar_index = opencode_current_command_bar_index(&frame);
    let input_box_index = opencode_current_input_box_index(&frame);
    let idle_index = placeholder_index
        .max(command_bar_index)
        .max(input_box_index);

    let busy_index = frame.rposition(opencode_current_busy_marker_line);

    if let Some(index) = busy_index
        && opencode_busy_marker_is_current(
            &frame,
            index,
            idle_index,
            command_bar_index,
            input_box_index,
        )
    {
        let kind = if frame.line(index).is_some_and(opencode_waiting_marker_line) {
            StatusKind::Waiting
        } else {
            StatusKind::Busy
        };
        return Some(kind);
    }

    idle_index.map(|_| StatusKind::Idle)
}

/// Whether a busy marker reflects the current bottom frame rather than stale scrollback.
///
/// The capture is the visible screen, and inline-scroll UIs keep prior turns on screen, so
/// an old approval/interrupt line can sit above a frame that has since scrolled on. A busy marker is current when it is
/// below the live idle prompt (a new run started under it), pinned in the persistent prompt
/// footer region (`esc interrupt` rendered just above the command bar or the input box border),
/// or — when there is no current idle anchor at all — is itself in the current bottom frame. A
/// stale marker scrolled up with no current prompt below it must stay unknown, not busy.
///
/// Both the command bar (`tab agents …`) and the input box border (`╹▀▀▀`) are valid footer
/// anchors: a used session folds the command bar into the bottom status bar, leaving the box
/// border as the only footer landmark, so a current `esc interrupt` above that box must still
/// win over the input-box idle anchor rather than fall through to idle.
fn opencode_busy_marker_is_current(
    frame: &PaneOutputFrame<'_>,
    busy_index: usize,
    idle_index: Option<usize>,
    command_bar_index: Option<usize>,
    input_box_index: Option<usize>,
) -> bool {
    let in_current_footer =
        opencode_busy_marker_in_current_footer(frame, busy_index, command_bar_index)
            || opencode_busy_marker_in_current_footer(frame, busy_index, input_box_index);
    match idle_index {
        Some(idle_index) => idle_index < busy_index || in_current_footer,
        None => in_current_footer || opencode_prompt_is_near_current_footer(frame, busy_index),
    }
}

fn opencode_busy_marker_in_current_footer(
    frame: &PaneOutputFrame<'_>,
    busy_index: usize,
    command_bar_index: Option<usize>,
) -> bool {
    let Some(command_bar) = command_bar_index else {
        return false;
    };
    frame.forward_gap_before_all(busy_index, command_bar, opencode_prompt_gap_line)
}

fn opencode_prompt_gap_line(line: &str) -> bool {
    let line = line.trim();
    line.is_empty()
        || line.starts_with('┃')
        || line.starts_with('╹')
        || line.starts_with('╭')
        || line.starts_with('│')
        || line.starts_with('╰')
}

fn opencode_idle_prompt_line(line: &str) -> bool {
    let line = line.trim_start();
    line.contains(OPENCODE_ASK_PLACEHOLDER) || line.contains(OPENCODE_COMMAND_PLACEHOLDER)
}

/// Index of the newer build's command bar when its input box is the current prompt.
///
/// The bordered input box sits directly above a `tab agents  ctrl+p commands` command bar.
/// Prior turns stay visible on screen above the prompt, so only opencode's own trailing
/// chrome (blank rows, notices, the bottom status bar) may follow it; a command bar trailed
/// by real agent output is a stale frame, not the current prompt.
fn opencode_current_command_bar_index(frame: &PaneOutputFrame<'_>) -> Option<usize> {
    let footer_index = frame.rposition(opencode_command_bar_footer_line)?;
    let has_input_box = frame
        .window_before(footer_index, 2)?
        .iter()
        .any(|line| opencode_input_box_bottom_border(line));

    // Only opencode's own chrome may follow the command bar: blank rows, a `● Tip` notice,
    // or the pinned bottom status bar as the final row. Anything else means the command bar
    // is a stale frame in the scrollback capture, not the current prompt.
    let only_trailing_chrome = opencode_trailing_chrome_after(frame, footer_index, false);
    (has_input_box && only_trailing_chrome).then_some(footer_index)
}

/// Index of the bordered input box's bottom border when that box is the current idle prompt.
///
/// `opencode_current_command_bar_index` anchors on the `tab agents  ctrl+p commands` hint, but
/// the live build drops that hint once a session has activity, folding the command bar into the
/// bottom status bar (`<tokens> (<pct>) · $<cost>  ctrl+p commands  • OpenCode <ver>`). The stable
/// element across fresh and used sessions is the input box's `╹▀▀▀` bottom border, so anchor on
/// it: it is the current prompt when only opencode's own trailing chrome follows (blank rows, the
/// `tab agents` command bar, a `● Tip` notice, or the pinned bottom status bar as the final row).
/// A border trailed by real agent output is a stale frame in the scrollback capture, not the
/// current prompt. Busy markers still win — this only contributes to the idle anchor.
fn opencode_current_input_box_index(frame: &PaneOutputFrame<'_>) -> Option<usize> {
    let border_index = frame.rposition(opencode_input_box_bottom_border)?;
    let only_trailing_chrome = opencode_trailing_chrome_after(frame, border_index, true);
    only_trailing_chrome.then_some(border_index)
}

fn opencode_trailing_chrome_after(
    frame: &PaneOutputFrame<'_>,
    index: usize,
    allow_command_bar: bool,
) -> bool {
    frame.trailing_lines_after_are(index, |_, line, is_last| {
        opencode_trailing_chrome_line(line, is_last, allow_command_bar)
    })
}

fn opencode_trailing_chrome_line(line: &str, is_last: bool, allow_command_bar: bool) -> bool {
    let trimmed = line.trim();
    trimmed.is_empty()
        || trimmed.starts_with(OPENCODE_TIP_MARKER)
        || (allow_command_bar && opencode_command_bar_footer_line(trimmed))
        || (is_last && opencode_bottom_status_bar_line(trimmed))
}

fn opencode_command_bar_footer_line(line: &str) -> bool {
    let line = line.trim();
    line.contains(OPENCODE_COMMAND_BAR_TAB_HINT)
        && line.contains(OPENCODE_COMMAND_BAR_COMMANDS_HINT)
}

fn opencode_input_box_bottom_border(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with('╹') && line.contains('▀')
}

/// opencode's pinned bottom status bar, e.g. `~/path:branch    1.15.11` or
/// `… • OpenCode 1.15.11`. Matched by its actual shape — the `• OpenCode` brand followed by
/// a version, or a `path:branch` line ending in a version token — so arbitrary agent output
/// that merely mentions a semver/IP (`Updated SDK to 1.2.3`, `See RFC 192.168.1.1`) or a
/// bare file path is not mistaken for chrome.
fn opencode_bottom_status_bar_line(line: &str) -> bool {
    if line.contains(OPENCODE_BRAND_MARKER) {
        return line.split_whitespace().any(is_version_like_command);
    }
    (line.starts_with("~/") || line.starts_with('/'))
        && line.contains(':')
        && line
            .split_whitespace()
            .next_back()
            .is_some_and(is_version_like_command)
}

fn opencode_current_busy_marker_line(line: &str) -> bool {
    let line = line.trim();
    opencode_interrupt_hint_line(line)
        || opencode_permission_prompt_line(line)
        || opencode_question_prompt_line(line)
}

fn opencode_waiting_marker_line(line: &str) -> bool {
    let line = line.trim();
    opencode_permission_prompt_line(line) || line.contains(OPENCODE_WAITING_QUESTION_MARKER)
}

fn opencode_interrupt_hint_line(line: &str) -> bool {
    line.contains(OPENCODE_INTERRUPT_ESC_MARKER) && line.contains(OPENCODE_INTERRUPT_VERB_MARKER)
}

fn opencode_permission_prompt_line(line: &str) -> bool {
    line.contains(OPENCODE_PERMISSION_REQUIRED_MARKER)
        || line.contains(OPENCODE_REJECT_PERMISSION_MARKER)
        || line.contains(OPENCODE_ALLOW_ONCE_MARKER)
        || line.contains(OPENCODE_ALLOW_ALWAYS_MARKER)
        || (line.contains('△') && line.contains(OPENCODE_PERMISSION_MARKER))
}

fn opencode_question_prompt_line(line: &str) -> bool {
    line.contains(OPENCODE_REJECT_QUESTION_MARKER)
        || line.contains(OPENCODE_WAITING_QUESTION_MARKER)
        || line.contains(OPENCODE_QUESTIONS_MARKER)
}

fn opencode_prompt_is_near_current_footer(
    frame: &PaneOutputFrame<'_>,
    prompt_index: usize,
) -> bool {
    frame.is_within_tail(prompt_index, 8)
}

#[cfg(test)]
mod tests {
    use crate::app::classify;
    use crate::app::tests::{pane_output_status_pane, proc_fallback_pane};
    use crate::app::{Provider, StatusKind};

    #[test]
    fn opencode_pane_output_marks_current_tui_prompt_idle_only_after_provider_is_known() {
        let mut opencode = pane_output_status_pane(780, Provider::Opencode, "OpenCode");

        classify::apply_pane_output_status_fallback(
            &mut opencode,
            "│ Build finished\n\
         ╹  Ask anything... \"Fix a TODO in the codebase\"\n\
            Build · gpt-5.1 OpenAI\n\
         ~/code/app                                                    /status\n",
        );

        assert_eq!(opencode.status.kind, StatusKind::Idle);
        assert_eq!(opencode.status.source, crate::app::StatusSource::PaneOutput);

        let mut unknown = proc_fallback_pane(781, "zsh", "custom title");
        classify::apply_pane_output_status_fallback(
            &mut unknown,
            "│ Build finished\n\
         ╹  Ask anything... \"Fix a TODO in the codebase\"\n\
            Build · gpt-5.1 OpenAI\n\
         ~/code/app                                                    /status\n",
        );

        assert_eq!(unknown.status.kind, StatusKind::Unknown);
        assert_eq!(unknown.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn opencode_pane_output_marks_shell_prompt_idle() {
        let mut opencode = pane_output_status_pane(782, Provider::Opencode, "OC | Shell");

        classify::apply_pane_output_status_fallback(
            &mut opencode,
            "╹  Run a command... \"git status\"\n\
            Shell\n\
         ~/code/app                                                    /status\n",
        );

        assert_eq!(opencode.status.kind, StatusKind::Idle);
        assert_eq!(opencode.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn opencode_pane_output_marks_running_prompt_busy() {
        let mut opencode = pane_output_status_pane(783, Provider::Opencode, "OC | Working");

        classify::apply_pane_output_status_fallback(
            &mut opencode,
            "╹  Ask anything... \"Fix a TODO in the codebase\"\n\
            Build · gpt-5.1 OpenAI\n\
         ⠹ Reading files\n\
         esc interrupt\n",
        );

        assert_eq!(opencode.status.kind, StatusKind::Busy);
        assert_eq!(opencode.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn opencode_pane_output_marks_permission_prompt_waiting() {
        let mut opencode = pane_output_status_pane(784, Provider::Opencode, "OC | Permission");

        classify::apply_pane_output_status_fallback(
            &mut opencode,
            "Permission required\n\
         → Edit src/app.rs\n\
         Allow once   Allow always   Reject\n\
         esc reject\n",
        );

        assert_eq!(opencode.status.kind, StatusKind::Waiting);
        assert_eq!(opencode.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn opencode_pane_output_uses_current_busy_marker_over_stale_idle_prompt() {
        let mut opencode = pane_output_status_pane(785, Provider::Opencode, "OC | Working");

        classify::apply_pane_output_status_fallback(
            &mut opencode,
            "╹  Ask anything... \"Fix a TODO in the codebase\"\n\
            Build · gpt-5.1 OpenAI\n\
         ~/code/app                                                    /status\n\
         \n\
         Permission required\n\
         → Bash sleep 10\n\
         Allow once   Allow always   Reject\n",
        );

        assert_eq!(opencode.status.kind, StatusKind::Waiting);
        assert_eq!(opencode.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn opencode_pane_output_marks_question_event_waiting() {
        let mut opencode = pane_output_status_pane(813, Provider::Opencode, "OC | Question");

        classify::apply_pane_output_status_fallback(
            &mut opencode,
            "# Questions\n\
         Reject question\n\
         Waiting for question event\n",
        );

        assert_eq!(opencode.status.kind, StatusKind::Waiting);
        assert_eq!(opencode.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn opencode_pane_output_does_not_force_busy_from_stale_approval_without_current_anchor() {
        // The screen capture holds an old approval prompt near the top, but the current bottom
        // frame is plain agent output with no live idle prompt or command bar below it. With no
        // current anchor the stale approval must not force busy — the honest answer is unknown.
        let mut opencode = pane_output_status_pane(795, Provider::Opencode, "OC | Working");

        classify::apply_pane_output_status_fallback(
            &mut opencode,
            "Permission required\n\
         → Bash sleep 10\n\
         Allow once   Allow always   Reject\n\
         \n\
         Reading files\n\
         Planning edits\n\
         Updating code\n\
         Running tests\n\
         Collecting results\n\
         Preparing response\n\
         Current line\n",
        );

        assert_eq!(opencode.status.kind, StatusKind::Unknown);
        assert_eq!(opencode.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn opencode_pane_output_does_not_infer_idle_from_stale_prompt() {
        let mut opencode = pane_output_status_pane(786, Provider::Opencode, "OC | Working");

        classify::apply_pane_output_status_fallback(
            &mut opencode,
            "╹  Ask anything... \"Fix a TODO in the codebase\"\n\
            Build · gpt-5.1 OpenAI\n\
         ~/code/app                                                    /status\n\
         \n\
         Planning edits\n\
         Reading files\n\
         Updating code\n\
         Running tests\n\
         Collecting results\n\
         Preparing response\n\
         Still working\n\
         Current line\n",
        );

        assert_eq!(opencode.status.kind, StatusKind::Unknown);
        assert_eq!(opencode.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn opencode_pane_output_marks_new_build_splash_idle() {
        // Newer "OpenCode Go" splash: the input box is centered with the command bar below it,
        // and the bottom status bar (path + version) sits far below at the true pane bottom.
        let mut opencode = pane_output_status_pane(801, Provider::Opencode, "OpenCode");

        classify::apply_pane_output_status_fallback(
            &mut opencode,
            "┃\n\
         ┃  Ask anything... \"Fix a TODO in the codebase\"\n\
         ┃\n\
         ┃  Build · Kimi K2.6 OpenCode Go\n\
         ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
         tab agents  ctrl+p commands\n\
         \n\
         ● Tip Use opencode run -f file.ts to attach files via CLI\n\
         \n\
         \n\
         ~/code/agentscan:main                                  1.15.11\n",
        );

        assert_eq!(opencode.status.kind, StatusKind::Idle);
        assert_eq!(opencode.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn opencode_pane_output_leaves_literal_wrapped_tip_splash_unknown() {
        // The old v1.15.11 sentence-specific exception over-classified aligned output as chrome.
        // Wrapped tips now deliberately degrade to unknown instead of matching prose.
        let mut opencode = pane_output_status_pane(813, Provider::Opencode, "OpenCode");

        classify::apply_pane_output_status_fallback(
            &mut opencode,
            concat!(
                "┃\n",
                "┃  Ask anything... \"What is the tech stack of this project?\"\n",
                "┃\n",
                "┃  Build · Kimi K2.6 OpenCode Go\n",
                "╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n",
                "tab agents  ctrl+p commands\n",
                "\n",
                "● Tip Press ctrl+f in the session list to pin a session so it stays at the\n",
                "      top\n",
                "\n",
                "\n",
                "\n",
                "~/code/agentscan:main                                  1.15.11\n",
            ),
        );

        assert_eq!(opencode.status.kind, StatusKind::Unknown);
        assert_eq!(opencode.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn opencode_pane_output_leaves_reworded_wrapped_tip_unknown() {
        let mut opencode = pane_output_status_pane(818, Provider::Opencode, "OpenCode");

        classify::apply_pane_output_status_fallback(
            &mut opencode,
            concat!(
                "┃\n",
                "┃  Ask anything... \"Review this project\"\n",
                "┃\n",
                "┃  Build · Kimi K2.6 OpenCode Go\n",
                "╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n",
                "tab agents  ctrl+p commands\n",
                "\n",
                "● Tip Pin important sessions from the list so they remain at the\n",
                "      top after restarting\n",
                "\n",
                "~/code/agentscan:main                                  1.16.0\n",
            ),
        );

        assert_eq!(opencode.status.kind, StatusKind::Unknown);
        assert_eq!(opencode.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn opencode_pane_output_does_not_treat_tip_followed_by_output_as_chrome() {
        let mut opencode = pane_output_status_pane(814, Provider::Opencode, "OpenCode");

        classify::apply_pane_output_status_fallback(
            &mut opencode,
            concat!(
                "┃\n",
                "┃  Ask anything... \"What is the tech stack of this project?\"\n",
                "┃\n",
                "┃  Build · Kimi K2.6 OpenCode Go\n",
                "╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n",
                "tab agents  ctrl+p commands\n",
                "\n",
                "● Tip Press ctrl+f in the session list to pin a session so it stays at the\n",
                "      cargo test\n",
                "\n",
                "~/code/agentscan:main                                  1.15.11\n",
            ),
        );

        assert_eq!(opencode.status.kind, StatusKind::Unknown);
        assert_eq!(opencode.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn opencode_pane_output_does_not_treat_ambiguous_tip_continuation_as_chrome() {
        let mut opencode = pane_output_status_pane(815, Provider::Opencode, "OpenCode");

        classify::apply_pane_output_status_fallback(
            &mut opencode,
            concat!(
                "┃\n",
                "┃  Ask anything... \"What is the tech stack of this project?\"\n",
                "┃\n",
                "┃  Build · Kimi K2.6 OpenCode Go\n",
                "╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n",
                "tab agents  ctrl+p commands\n",
                "\n",
                "\n",
                "\n",
                "\n",
                "\n",
                "\n",
                "● Tip Press ctrl+f in the session list to pin a session so it stays at the\n",
                "      cargo test\n",
            ),
        );

        assert_eq!(opencode.status.kind, StatusKind::Unknown);
        assert_eq!(opencode.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn opencode_pane_output_does_not_treat_top_after_other_tip_as_chrome() {
        let mut opencode = pane_output_status_pane(816, Provider::Opencode, "OpenCode");

        classify::apply_pane_output_status_fallback(
            &mut opencode,
            concat!(
                "┃\n",
                "┃  Ask anything... \"What is the tech stack of this project?\"\n",
                "┃\n",
                "┃  Build · Kimi K2.6 OpenCode Go\n",
                "╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n",
                "tab agents  ctrl+p commands\n",
                "\n",
                "● Tip Read the project notes before changing behavior\n",
                "      top\n",
                "~/code/agentscan:main                                  1.15.11\n",
            ),
        );

        assert_eq!(opencode.status.kind, StatusKind::Unknown);
        assert_eq!(opencode.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn opencode_pane_output_does_not_treat_top_without_spacer_as_chrome() {
        let mut opencode = pane_output_status_pane(817, Provider::Opencode, "OpenCode");

        classify::apply_pane_output_status_fallback(
            &mut opencode,
            concat!(
                "┃\n",
                "┃  Ask anything... \"What is the tech stack of this project?\"\n",
                "┃\n",
                "┃  Build · Kimi K2.6 OpenCode Go\n",
                "╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n",
                "tab agents  ctrl+p commands\n",
                "\n",
                "● Tip Press ctrl+f in the session list to pin a session so it stays at the\n",
                "      top\n",
                "~/code/agentscan:main                                  1.15.11\n",
            ),
        );

        assert_eq!(opencode.status.kind, StatusKind::Unknown);
        assert_eq!(opencode.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn opencode_pane_output_marks_new_build_active_session_idle() {
        // After a turn completes the placeholder is gone AND the live build drops the `tab agents`
        // hint, folding the command bar into the bottom status bar with token/cost usage stats. The
        // bordered input box's `╹▀▀▀` border is the stable anchor that keeps this the current idle
        // prompt — anchoring on `tab agents` alone would miss every used session.
        let mut opencode = pane_output_status_pane(802, Provider::Opencode, "OC | Greeting");

        classify::apply_pane_output_status_fallback(
            &mut opencode,
            "Hello! How can I help you today?\n\
         ▣  Build · Kimi K2.6 · 4.0s\n\
         ┃\n\
         ┃\n\
         ┃\n\
         ┃  Build · Kimi K2.6 OpenCode Go\n\
         ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
         11.8K (4%) · $0.01  ctrl+p commands    • OpenCode 1.15.11\n",
        );

        assert_eq!(opencode.status.kind, StatusKind::Idle);
        assert_eq!(opencode.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn opencode_pane_output_marks_live_build_used_session_idle() {
        // Real capture (build 1.15.11): a used session sits idle with the input box centered, a wide
        // blank gap below it, and the merged command/status bar (`<stats> ctrl+p commands · OpenCode`)
        // pinned at the true pane bottom. No placeholder, no `tab agents` — only the `╹▀▀▀` border
        // anchors it.
        let mut opencode = pane_output_status_pane(810, Provider::Opencode, "OC | Greeting");

        classify::apply_pane_output_status_fallback(
            &mut opencode,
            "  ┃  hi\n\
         \n\
         \n\
         \n\
         \n\
         \n\
            Hello! How can I help you today?\n\
            ▣  Build · Kimi K2.6 · 4.3s\n\
         \n\
         \n\
         \n\
           ┃\n\
           ┃\n\
           ┃  Build · Kimi K2.6 OpenCode Go                              ~/code/agentscan:main\n\
           ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
         \n\
         \n\
            11.8K (4%) · $0.01  ctrl+p commands    • OpenCode 1.15.11\n",
        );

        assert_eq!(opencode.status.kind, StatusKind::Idle);
        assert_eq!(opencode.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn opencode_pane_output_live_build_marks_busy_when_interrupt_hint_in_merged_bottom_bar() {
        // Real capture: the live build renders `esc interrupt` plus the braille run spinner in the
        // merged command/status bar directly *below* the input box border, not above it. The current
        // busy marker must win over the input box that the idle anchor now also recognizes.
        let mut opencode = pane_output_status_pane(811, Provider::Opencode, "OC | Repo review");

        classify::apply_pane_output_status_fallback(
            &mut opencode,
            "  ┃\n\
           ┃  Build · Kimi K2.6 OpenCode Go                              ~/code/agentscan:main\n\
           ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
            ⬝⬝⬝⬝⬝⬝⬝⬝  esc interrupt    139.2K (53%) · $0.23  ctrl+p commands    • OpenCode 1.15.11\n",
        );

        assert_eq!(opencode.status.kind, StatusKind::Busy);
        assert_eq!(opencode.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn opencode_pane_output_live_build_marks_busy_when_interrupt_hint_above_box_without_command_bar()
     {
        // Used session (no `tab agents`, so `command_bar_index` is None) with `esc interrupt`
        // rendered just *above* the input box. The box border is the only footer anchor, so the
        // current busy marker must still win over the input-box idle anchor rather than fall
        // through to idle.
        let mut opencode = pane_output_status_pane(812, Provider::Opencode, "OC | Greeting");

        classify::apply_pane_output_status_fallback(
            &mut opencode,
            "  Reading the codebase\n\
            esc interrupt\n\
           ┃\n\
           ┃  Build · Kimi K2.6 OpenCode Go\n\
           ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
            11.8K (4%) · $0.01  ctrl+p commands    • OpenCode 1.15.11\n",
        );

        assert_eq!(opencode.status.kind, StatusKind::Busy);
        assert_eq!(opencode.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn opencode_pane_output_new_build_yields_to_current_busy_marker() {
        // A waiting marker still wins over the persistent command-bar input box.
        let mut opencode = pane_output_status_pane(803, Provider::Opencode, "OC | Working");

        classify::apply_pane_output_status_fallback(
            &mut opencode,
            "┃\n\
         ┃  Build · Kimi K2.6 OpenCode Go\n\
         ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
         tab agents  ctrl+p commands    • OpenCode 1.15.11\n\
         Permission required\n\
         Allow once   Allow always   Reject\n",
        );

        assert_eq!(opencode.status.kind, StatusKind::Waiting);
        assert_eq!(opencode.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn opencode_pane_output_does_not_infer_new_build_idle_with_output_below_command_bar() {
        // A stale command bar + input box sit in the scrollback capture, but newer agent output
        // scrolled below them with no recognized busy marker. The command bar is no longer the
        // current prompt, so the pane must stay unknown rather than be reported idle.
        let mut opencode = pane_output_status_pane(805, Provider::Opencode, "OpenCode");

        classify::apply_pane_output_status_fallback(
            &mut opencode,
            "╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
         tab agents  ctrl+p commands    • OpenCode 1.15.11\n\
         Reading files\n\
         Planning edits\n\
         Updating code\n\
         Current line\n",
        );

        assert_eq!(opencode.status.kind, StatusKind::Unknown);
        assert_eq!(opencode.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn opencode_pane_output_new_build_marks_busy_when_interrupt_hint_above_persistent_command_bar()
    {
        // The newer build keeps its command bar pinned during a run, with the `esc interrupt`
        // status rendered just above the input box. The persistent command bar must not be read
        // as idle while that current interrupt hint is in the prompt footer.
        let mut opencode = pane_output_status_pane(809, Provider::Opencode, "OC | Greeting");

        classify::apply_pane_output_status_fallback(
            &mut opencode,
            "Reading the codebase\n\
         esc interrupt\n\
         ┃\n\
         ┃  Build · Kimi K2.6 OpenCode Go\n\
         ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
         tab agents  ctrl+p commands    • OpenCode 1.15.11\n",
        );

        assert_eq!(opencode.status.kind, StatusKind::Busy);
        assert_eq!(opencode.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn opencode_pane_output_new_build_idle_survives_stale_approval_in_scrollback() {
        // A resolved permission prompt is still in the scrollback capture above the current
        // command-bar input box. Because the live prompt sits below it, the pane is idle — the
        // stale approval must not preempt to busy.
        let mut opencode = pane_output_status_pane(806, Provider::Opencode, "OC | Greeting");

        classify::apply_pane_output_status_fallback(
            &mut opencode,
            "Permission required\n\
         Allow once   Allow always   Reject\n\
         Done. Applied the edit.\n\
         ┃\n\
         ┃  Build · Kimi K2.6 OpenCode Go\n\
         ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
         tab agents  ctrl+p commands    • OpenCode 1.15.11\n",
        );

        assert_eq!(opencode.status.kind, StatusKind::Idle);
        assert_eq!(opencode.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn opencode_pane_output_does_not_treat_path_output_below_command_bar_as_chrome() {
        // A stale command bar sits in the scrollback capture with only file-path agent output
        // below it (common in coding output). Paths are not opencode chrome, so the stale
        // command bar is not the current prompt and the pane stays unknown.
        let mut opencode = pane_output_status_pane(807, Provider::Opencode, "OpenCode");

        classify::apply_pane_output_status_fallback(
            &mut opencode,
            "╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
         tab agents  ctrl+p commands\n\
         ~/code/agentscan/src/app/classify/pane_output.rs\n\
         /Users/auro/code/agentscan/src/app/scanner.rs\n",
        );

        assert_eq!(opencode.status.kind, StatusKind::Unknown);
        assert_eq!(opencode.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn opencode_pane_output_does_not_treat_semver_prose_below_command_bar_as_chrome() {
        // Agent output that merely mentions a semver or IP is not the pinned status bar, so a
        // stale command bar above such output must not be read as the current idle prompt.
        let mut opencode = pane_output_status_pane(808, Provider::Opencode, "OpenCode");

        classify::apply_pane_output_status_fallback(
            &mut opencode,
            "╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
         tab agents  ctrl+p commands\n\
         Updated SDK to 1.2.3 in the lockfile\n\
         See RFC 192.168.1.1 for details\n",
        );

        assert_eq!(opencode.status.kind, StatusKind::Unknown);
        assert_eq!(opencode.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn opencode_pane_output_does_not_infer_new_build_idle_without_input_box() {
        // The command bar alone (no bordered input box above it) is not enough to call idle.
        let mut opencode = pane_output_status_pane(804, Provider::Opencode, "OpenCode");

        classify::apply_pane_output_status_fallback(
            &mut opencode,
            "Reading files\n\
         Planning edits\n\
         tab agents  ctrl+p commands    • OpenCode 1.15.11\n",
        );

        assert_eq!(opencode.status.kind, StatusKind::Unknown);
        assert_eq!(opencode.status.source, crate::app::StatusSource::NotChecked);
    }
}
