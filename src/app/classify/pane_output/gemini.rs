use super::{PaneOutputFrame, StatusKind};

// Gemini idle input prompt placeholder.
const IDLE_PROMPT_MARKER: &str = "Type your message";
// Gemini busy/approval markers shown while a turn is running.
const ACTION_REQUIRED_MARKER: &str = "Action Required";
const APPLY_CHANGE_MARKER: &str = "Apply this change?";
const ALLOW_EXECUTION_MARKER: &str = "Allow execution of";
const RUNNING_AGENT_MARKER: &str = "Running Agent";
const COLLAPSE_HINT: &str = "ctrl+o to collapse";
// Gemini auth-flow prompt copy.
const AUTH_OPENING_MARKER: &str = "Opening authentication page in your browser";
const AUTH_CONTINUE_MARKER: &str = "Do you want to continue?";
const AUTH_SELECT_HINT: &str = "Enter to select";
const AUTH_NAVIGATE_HINT: &str = "to navigate";

pub(super) fn status(output: &str) -> Option<StatusKind> {
    let frame = PaneOutputFrame::new(output);
    let idle_index = frame.rposition(gemini_idle_input_prompt_line);
    let busy_index = gemini_current_busy_marker_index(&frame);

    // A dismissed approval modal stays visible higher on the screen after the
    // turn moves on, so a busy marker only counts when it is anchored to the
    // current bottom frame (same 14-row window as the auth prompt below).
    if let Some(index) = busy_index
        && frame.is_within_tail(index, 14)
        && idle_index.is_none_or(|idle_index| idle_index < index)
    {
        return Some(StatusKind::Busy);
    }

    if let Some(index) = gemini_current_auth_prompt_index(&frame)
        && idle_index.is_none_or(|idle_index| idle_index < index)
    {
        return Some(StatusKind::Busy);
    }

    idle_index
        .is_some_and(|index| gemini_prompt_is_near_current_footer(&frame, index))
        .then_some(StatusKind::Idle)
}

fn gemini_idle_input_prompt_line(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with('>') && line.contains(IDLE_PROMPT_MARKER)
}

fn gemini_current_busy_marker_index(frame: &PaneOutputFrame<'_>) -> Option<usize> {
    let mut marker_index = frame.rposition(gemini_current_busy_marker_line);
    while let Some(index) = marker_index {
        let line = frame.line(index)?;
        if gemini_running_agent_marker_line(line)
            || gemini_approval_marker_has_current_chrome(frame, index)
        {
            return Some(index);
        }
        marker_index = frame.rposition_before(index, gemini_current_busy_marker_line);
    }
    None
}

fn gemini_current_busy_marker_line(line: &str) -> bool {
    let line = line.trim();
    line.contains(ACTION_REQUIRED_MARKER)
        || line.contains(APPLY_CHANGE_MARKER)
        || line.contains(ALLOW_EXECUTION_MARKER)
        || gemini_running_agent_marker_line(line)
}

fn gemini_running_agent_marker_line(line: &str) -> bool {
    line.contains(RUNNING_AGENT_MARKER) && line.contains(COLLAPSE_HINT)
}

fn gemini_approval_marker_has_current_chrome(
    frame: &PaneOutputFrame<'_>,
    marker_index: usize,
) -> bool {
    let chrome_start = marker_index.saturating_sub(3);
    let chrome_len = marker_index - chrome_start + 4;
    let has_nearby_box = frame.lines_from(chrome_start).is_some_and(|lines| {
        lines
            .iter()
            .take(chrome_len)
            .any(|line| gemini_modal_box_line(line))
    });
    let glyph_start = marker_index.saturating_sub(1);
    let glyph_len = marker_index - glyph_start + 2;
    let has_adjacent_status_glyph = frame.lines_from(glyph_start).is_some_and(|lines| {
        lines
            .iter()
            .take(glyph_len)
            .any(|line| line.contains('✋') || line.contains('⏲'))
    });

    has_nearby_box || has_adjacent_status_glyph
}

fn gemini_modal_box_line(line: &str) -> bool {
    let line = line.trim();
    (line.starts_with('│') && line.ends_with('│'))
        || ((line.starts_with('╭') || line.starts_with('╰')) && line.chars().any(|ch| ch == '─'))
}

fn gemini_current_auth_prompt_index(frame: &PaneOutputFrame<'_>) -> Option<usize> {
    let index = frame.rposition(|line| line.contains(AUTH_OPENING_MARKER))?;

    if !frame.is_within_tail(index, 14) {
        return None;
    }

    let lines = frame.lines_from(index)?;
    lines
        .iter()
        .any(|line| line.contains(AUTH_CONTINUE_MARKER))
        .then_some(())?;
    lines
        .iter()
        .any(|line| line.contains(AUTH_SELECT_HINT) || line.contains(AUTH_NAVIGATE_HINT))
        .then_some(index)
}

fn gemini_prompt_is_near_current_footer(frame: &PaneOutputFrame<'_>, prompt_index: usize) -> bool {
    frame.is_within_tail(prompt_index, 8)
}

#[cfg(test)]
mod tests {
    use crate::app::classify;
    use crate::app::tests::{pane_output_status_pane, proc_fallback_pane};
    use crate::app::{PaneStatus, Provider, StatusKind};

    #[test]
    fn gemini_pane_output_marks_current_prompt_idle_only_after_provider_is_known() {
        let mut gemini = pane_output_status_pane(775, Provider::Gemini, "Gemini CLI");

        classify::apply_pane_output_status_fallback(
            &mut gemini,
            "Welcome to Gemini CLI\n\
         \n\
         >   Type your message or @path/to/file\n\
         Workspace   Sandbox    Model\n\
         ~/code/app  no sandbox gemini-2.5-pro\n",
        );

        assert_eq!(gemini.status.kind, StatusKind::Idle);
        assert_eq!(gemini.status.source, crate::app::StatusSource::PaneOutput);

        let mut unknown = proc_fallback_pane(776, "zsh", "custom title");
        classify::apply_pane_output_status_fallback(
            &mut unknown,
            "Welcome to Gemini CLI\n\
         \n\
         >   Type your message or @path/to/file\n\
         Workspace   Sandbox    Model\n\
         ~/code/app  no sandbox gemini-2.5-pro\n",
        );

        assert_eq!(unknown.status.kind, StatusKind::Unknown);
        assert_eq!(unknown.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn gemini_pane_output_marks_action_required_busy() {
        let mut gemini = pane_output_status_pane(777, Provider::Gemini, "Gemini CLI");

        classify::apply_pane_output_status_fallback(
            &mut gemini,
            "╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ Action Required                                                             │\n\
         │ ?  ls list directory                                                        │\n\
         │ Allow execution of [ls]?                                                    │\n\
         │   1. Yes                                                                    │\n\
         │   2. No, suggest changes (esc)                                              │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n",
        );

        assert_eq!(gemini.status.kind, StatusKind::Busy);
        assert_eq!(gemini.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn gemini_pane_output_ignores_chromeless_action_required_prose() {
        let mut gemini = pane_output_status_pane(784, Provider::Gemini, "Gemini CLI");

        classify::apply_pane_output_status_fallback(
            &mut gemini,
            ">   Type your message or @path/to/file\n\
         Workspace   Sandbox    Model\n\
         The phrase Action Required can appear in generated documentation.\n",
        );

        assert_eq!(gemini.status.kind, StatusKind::Idle);
        assert_eq!(gemini.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn gemini_pane_output_withholds_status_from_chromeless_busy_marker() {
        let mut gemini = pane_output_status_pane(785, Provider::Gemini, "Gemini CLI");

        classify::apply_pane_output_status_fallback(
            &mut gemini,
            "Generated copy follows.\n\
         Apply this change? is an example confirmation message.\n\
         No modal is currently displayed.\n",
        );

        assert_eq!(gemini.status.kind, StatusKind::Unknown);
        assert_eq!(gemini.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn gemini_pane_output_marks_auth_prompt_busy() {
        let mut gemini = pane_output_status_pane(780, Provider::Gemini, "Gemini CLI");

        classify::apply_pane_output_status_fallback(
            &mut gemini,
            "Gemini CLI v0.49.0\n\
         \n\
         ╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │  Opening authentication page in your browser.                                │\n\
         │                                                                              │\n\
         │  Do you want to continue?                                                    │\n\
         │                                                                              │\n\
         │  ● 1. Yes                                                                    │\n\
         │    2. No                                                                     │\n\
         │                                                                              │\n\
         │  Enter to select · ↑/↓ to navigate · Esc to cancel                           │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n",
        );

        assert_eq!(gemini.status.kind, StatusKind::Busy);
        assert_eq!(gemini.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn gemini_pane_output_refines_ready_title_when_auth_prompt_is_visible() {
        let mut gemini = pane_output_status_pane(781, Provider::Gemini, "◇  Ready (gemini)");
        gemini.status = PaneStatus::title(StatusKind::Idle);

        classify::apply_pane_output_status_fallback(
            &mut gemini,
            "Gemini CLI v0.49.0\n\
         ╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │  Opening authentication page in your browser.                                │\n\
         │  Do you want to continue?                                                    │\n\
         │  Enter to select · ↑/↓ to navigate · Esc to cancel                           │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n",
        );

        assert_eq!(gemini.status.kind, StatusKind::Busy);
        assert_eq!(gemini.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn gemini_pane_output_uses_current_idle_prompt_over_stale_auth_prompt() {
        let mut gemini = pane_output_status_pane(782, Provider::Gemini, "Gemini CLI");

        classify::apply_pane_output_status_fallback(
            &mut gemini,
            "Gemini CLI v0.49.0\n\
         ╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │  Opening authentication page in your browser.                                │\n\
         │  Do you want to continue?                                                    │\n\
         │  Enter to select · ↑/↓ to navigate · Esc to cancel                           │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n\
         \n\
         >   Type your message or @path/to/file\n\
         Workspace   Sandbox    Model\n\
         ~/code/app  no sandbox gemini-2.5-pro\n",
        );

        assert_eq!(gemini.status.kind, StatusKind::Idle);
        assert_eq!(gemini.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn gemini_pane_output_uses_current_busy_marker_over_stale_idle_prompt() {
        let mut gemini = pane_output_status_pane(778, Provider::Gemini, "Gemini CLI");

        classify::apply_pane_output_status_fallback(
            &mut gemini,
            ">   Type your message or @path/to/file\n\
         Workspace   Sandbox    Model\n\
         ~/code/app  no sandbox gemini-2.5-pro\n\
         \n\
         ╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ Action Required                                                             │\n\
         │ Apply this change?                                                          │\n\
         │   1. Yes                                                                    │\n\
         │   2. No, suggest changes (esc)                                              │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n",
        );

        assert_eq!(gemini.status.kind, StatusKind::Busy);
        assert_eq!(gemini.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn gemini_pane_output_does_not_infer_idle_from_stale_prompt() {
        let mut gemini = pane_output_status_pane(779, Provider::Gemini, "Gemini CLI");

        classify::apply_pane_output_status_fallback(
            &mut gemini,
            ">   Type your message or @path/to/file\n\
         Workspace   Sandbox    Model\n\
         ~/code/app  no sandbox gemini-2.5-pro\n\
         \n\
         ✦ Working on the latest request\n\
         Reading files\n\
         Preparing answer\n\
         Updating edits\n\
         Running tests\n\
         Collecting output\n\
         Still working\n\
         More output\n\
         Current line\n",
        );

        assert_eq!(gemini.status.kind, StatusKind::Unknown);
        assert_eq!(gemini.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn gemini_pane_output_ignores_stale_busy_marker_above_current_frame() {
        // A dismissed approval modal is still visible higher on the screen, but the
        // current bottom frame is plain agent output with no live idle prompt. The
        // stale modal must not read as busy — the honest answer is unknown.
        let mut gemini = pane_output_status_pane(783, Provider::Gemini, "Gemini CLI");

        classify::apply_pane_output_status_fallback(
            &mut gemini,
            "╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │ Action Required                                                             │\n\
         │ Apply this change?                                                          │\n\
         │   1. Yes                                                                    │\n\
         │   2. No, suggest changes (esc)                                              │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n\
         \n\
         ✦ Applying the approved change\n\
         Reading files\n\
         Preparing answer\n\
         Updating edits\n\
         Running tests\n\
         Collecting output\n\
         Checking diagnostics\n\
         Formatting sources\n\
         Reviewing results\n\
         Still working\n\
         More output\n\
         Current line\n",
        );

        assert_eq!(gemini.status.kind, StatusKind::Unknown);
        assert_eq!(gemini.status.source, crate::app::StatusSource::NotChecked);
    }
}
