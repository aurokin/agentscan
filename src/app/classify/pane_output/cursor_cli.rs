use super::{PaneOutputFrame, StatusKind};

// Cursor CLI busy footer/prompt hint shown while a turn is running.
const STOP_HINT: &str = "ctrl+c to stop";
// Cursor CLI running-spinner status verb.
const RUNNING_MARKER: &str = "Running";
// Cursor CLI idle prompt placeholders.
const IDLE_FOLLOW_UP_PROMPT: &str = "Add a follow-up";
const IDLE_START_PROMPT: &str = "Plan, search, build anything";
// Cursor CLI composer footer markers.
const COMPOSER_MARKER: &str = "Composer";
const COMPOSER_AUTO_RUN_MARKER: &str = "Auto-run";
const COMPOSER_RUN_EVERYTHING_MARKER: &str = "Run Everything";

pub(super) fn status(output: &str) -> Option<StatusKind> {
    let frame = PaneOutputFrame::new(output);
    let Some(footer_top_index) = frame.rposition(cursor_cli_footer_top_border) else {
        return cursor_cli_borderless_prompt_status(&frame);
    };

    let prompt_index = frame
        .lines_from(footer_top_index)?
        .iter()
        .position(|line| line.trim().starts_with('→'))
        .map(|index| footer_top_index + index);
    let current_footer = prompt_index
        .and_then(|index| frame.line(index))
        .map(str::trim);

    if current_footer.is_some_and(|line| line.contains(STOP_HINT))
        || cursor_cli_current_status_line(&frame, footer_top_index)
            .is_some_and(cursor_cli_status_line_indicates_running)
    {
        return Some(StatusKind::Busy);
    }

    let prompt_index = prompt_index?;
    (cursor_cli_prompt_has_current_chrome(&frame, prompt_index, true, false)
        || current_footer.is_some_and(cursor_cli_footer_indicates_idle))
    .then_some(StatusKind::Idle)
}

fn cursor_cli_borderless_prompt_status(frame: &PaneOutputFrame<'_>) -> Option<StatusKind> {
    let prompt_index = frame.rposition(cursor_cli_borderless_prompt_line)?;
    let prompt = frame.line(prompt_index)?.trim();
    if cursor_cli_borderless_prompt_indicates_busy(prompt) {
        return cursor_cli_prompt_has_current_chrome(frame, prompt_index, false, true)
            .then_some(StatusKind::Busy);
    }

    cursor_cli_prompt_has_current_chrome(frame, prompt_index, false, false)
        .then_some(StatusKind::Idle)
}

fn cursor_cli_prompt_has_current_chrome(
    frame: &PaneOutputFrame<'_>,
    prompt_index: usize,
    allow_footer_border: bool,
    allow_task_count: bool,
) -> bool {
    let Some(lines_after) = frame.lines_from(prompt_index) else {
        return false;
    };
    let has_cursor_footer = lines_after.iter().any(|line| {
        let line = line.trim();
        cursor_cli_composer_footer_line(line) || cursor_cli_path_footer_line(line)
    });
    let only_cursor_chrome_after = frame.trailing_lines_after_are(prompt_index, |_, line, _| {
        let line = line.trim();
        line.is_empty()
            || cursor_cli_composer_footer_line(line)
            || cursor_cli_path_footer_line(line)
            || (allow_footer_border && cursor_cli_footer_bottom_border(line))
            || (allow_task_count && cursor_cli_task_count_line(line))
    });

    has_cursor_footer && only_cursor_chrome_after
}

fn cursor_cli_current_status_line<'a>(
    frame: &'a PaneOutputFrame<'a>,
    footer_top_index: usize,
) -> Option<&'a str> {
    frame
        .previous_nonblank_before(footer_top_index)
        .map(str::trim)
}

fn cursor_cli_status_line_indicates_running(line: &str) -> bool {
    let mut parts = line.split_whitespace();
    let Some(spinner) = parts.next() else {
        return false;
    };
    spinner
        .chars()
        .all(|ch| ('\u{2800}'..='\u{28ff}').contains(&ch))
        && parts.any(|part| part == RUNNING_MARKER)
}

fn cursor_cli_footer_indicates_idle(line: &str) -> bool {
    let line = line.trim_start_matches('→').trim();
    line == IDLE_FOLLOW_UP_PROMPT || line == IDLE_START_PROMPT
}

fn cursor_cli_borderless_prompt_line(line: &str) -> bool {
    line.trim_start().starts_with('→')
}

fn cursor_cli_borderless_prompt_indicates_busy(line: &str) -> bool {
    line.contains(STOP_HINT)
}

fn cursor_cli_composer_footer_line(line: &str) -> bool {
    line.contains(COMPOSER_MARKER)
        && (line.contains(COMPOSER_AUTO_RUN_MARKER)
            || line.contains(COMPOSER_RUN_EVERYTHING_MARKER))
}

fn cursor_cli_path_footer_line(line: &str) -> bool {
    (line.starts_with('/') || line.starts_with("~/")) && line.contains(" · ")
}

fn cursor_cli_task_count_line(line: &str) -> bool {
    let line = line.trim();
    let Some(count) = line
        .strip_suffix(" task")
        .or_else(|| line.strip_suffix(" tasks"))
    else {
        return false;
    };

    count.trim().parse::<u32>().is_ok()
}

fn cursor_cli_footer_top_border(line: &str) -> bool {
    line.trim_start().starts_with("▄▄▄▄▄▄")
}

fn cursor_cli_footer_bottom_border(line: &str) -> bool {
    line.trim_start().starts_with("▀▀▀▀▀▀")
}

#[cfg(test)]
mod tests {
    use crate::app::classify;
    use crate::app::tests::pane_output_status_pane;
    use crate::app::{Provider, StatusKind};

    #[test]
    fn cursor_cli_pane_output_marks_current_running_prompt_busy() {
        let mut cursor = pane_output_status_pane(750, Provider::CursorCli, "Command Runner");

        classify::apply_pane_output_status_fallback(
            &mut cursor,
            "  $ sleep 15; printf cursor-smoke-ok > result.txt 11s in\n\
           /tmp/agentscan-cursor-smoke\n\
         \n\
         ⠳⠀ Running  187 tokens\n\
         ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄\n\
          → Add a follow-up                                             ctrl+c to stop\n\
         ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
          Auto · 5%                                                           Auto-run\n\
          /private/tmp/agentscan-cursor-smoke\n",
        );

        assert_eq!(cursor.status.kind, StatusKind::Busy);
        assert_eq!(cursor.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn cursor_cli_pane_output_marks_prompt_footer_running_busy() {
        let mut cursor = pane_output_status_pane(753, Provider::CursorCli, "Command Runner");

        classify::apply_pane_output_status_fallback(
            &mut cursor,
            "  $ sleep 12; printf cursor-smoke-ok-3 > result3.txt 12s\n\
         \n\
         ⠜⠃ Running  238 tokens\n\
         ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄\n\
          → Please run exactly this shell command: sleep 12; printf cursor-smoke-ok-3\n\
            > result3.txt. Do not edit anything else.\n\
         ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
          Auto · 5.1%                                                         Auto-run\n\
          /private/tmp/agentscan-cursor-smoke\n",
        );

        assert_eq!(cursor.status.kind, StatusKind::Busy);
        assert_eq!(cursor.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn cursor_cli_pane_output_ignores_stale_running_lines() {
        let mut cursor = pane_output_status_pane(751, Provider::CursorCli, "Command Runner");

        classify::apply_pane_output_status_fallback(
            &mut cursor,
            " ⠳⠀ Running  187 tokens\n\
         \n\
          Completed. I ran exactly:\n\
         \n\
          sleep 15; printf cursor-smoke-ok > result.txt\n\
         \n\
         ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄\n\
          → Add a follow-up\n\
         ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
          Auto · 5.1%                                                         Auto-run\n\
          /private/tmp/agentscan-cursor-smoke\n",
        );

        assert_eq!(cursor.status.kind, StatusKind::Idle);
        assert_eq!(cursor.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn cursor_cli_pane_output_uses_latest_footer() {
        let mut cursor = pane_output_status_pane(752, Provider::CursorCli, "Command Runner");

        classify::apply_pane_output_status_fallback(
            &mut cursor,
            " ⠳⠀ Running  187 tokens\n\
         ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄\n\
          → Add a follow-up                                             ctrl+c to stop\n\
         ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
         \n\
          Completed. I ran exactly:\n\
         \n\
          sleep 15; printf cursor-smoke-ok > result.txt\n\
         \n\
         ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄\n\
          → Add a follow-up\n\
         ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
          Auto · 5.1%                                                         Auto-run\n\
          /private/tmp/agentscan-cursor-smoke\n",
        );

        assert_eq!(cursor.status.kind, StatusKind::Idle);
        assert_eq!(cursor.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn cursor_cli_pane_output_ignores_stale_running_footer_block() {
        let mut cursor = pane_output_status_pane(754, Provider::CursorCli, "Command Runner");

        classify::apply_pane_output_status_fallback(
            &mut cursor,
            " ⠜⠃ Running  238 tokens\n\
         ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄\n\
          → Please run exactly this shell command: sleep 12; printf cursor-smoke-ok-3\n\
            > result3.txt. Do not edit anything else.\n\
         ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
         \n\
          Completed. I ran exactly:\n\
         \n\
          sleep 12; printf cursor-smoke-ok-3 > result3.txt\n\
         \n\
         ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄\n\
          → Add a follow-up\n\
         ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
          Auto · 5.1%                                                         Auto-run\n\
          /private/tmp/agentscan-cursor-smoke\n",
        );

        assert_eq!(cursor.status.kind, StatusKind::Idle);
        assert_eq!(cursor.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn cursor_cli_pane_output_ignores_response_text_before_idle_footer() {
        let mut cursor = pane_output_status_pane(755, Provider::CursorCli, "Command Runner");

        classify::apply_pane_output_status_fallback(
            &mut cursor,
            "  Running `cargo test` now passes.\n\
         \n\
         ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄\n\
          → Add a follow-up\n\
         ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
          Auto · 5.1%                                                         Auto-run\n\
          /private/tmp/agentscan-cursor-smoke\n",
        );

        assert_eq!(cursor.status.kind, StatusKind::Idle);
        assert_eq!(cursor.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn cursor_cli_pane_output_marks_initial_prompt_idle() {
        let mut cursor = pane_output_status_pane(756, Provider::CursorCli, "Cursor Agent");

        classify::apply_pane_output_status_fallback(
            &mut cursor,
            "  Cursor Agent\n\
          v2026.04.28-e984b46\n\
         \n\
         ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄\n\
          → Plan, search, build anything\n\
         ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
          Auto\n\
          ~/code/agentscan · master\n",
        );

        assert_eq!(cursor.status.kind, StatusKind::Idle);
        assert_eq!(cursor.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn cursor_cli_pane_output_marks_borderless_initial_prompt_idle() {
        // Observed from Cursor Agent v2026.06.04: the initial prompt no longer has the
        // `▄▄▄▄`/`▀▀▀▀` footer borders, but still renders the `→ Plan...` prompt above
        // Cursor's composer/path footer.
        let mut cursor = pane_output_status_pane(757, Provider::CursorCli, "Cursor Agent");

        classify::apply_pane_output_status_fallback(
            &mut cursor,
            "  Cursor Agent\n\
          v2026.06.04-5fd875e\n\
          Use /run-everything to skip all approvals.\n\
         \n\
         \n\
           → Plan, search, build anything\n\
         \n\
         \n\
           Composer 2.5                                                   Auto-run -- INSERT --\n\
           /private/tmp/agentscan-cursor-smoke · main\n",
        );

        assert_eq!(cursor.status.kind, StatusKind::Idle);
        assert_eq!(cursor.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn cursor_cli_pane_output_marks_run_everything_footer_idle() {
        let mut cursor = pane_output_status_pane(758, Provider::CursorCli, "Cursor Agent");

        classify::apply_pane_output_status_fallback(
            &mut cursor,
            "  Cursor Agent\n\
         \n\
           → Plan, search, build anything\n\
         \n\
         \n\
           Composer 2.5                                             Run Everything -- INSERT --\n\
           /private/tmp/agentscan-cursor-smoke · main\n",
        );

        assert_eq!(cursor.status.kind, StatusKind::Idle);
        assert_eq!(cursor.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn cursor_cli_pane_output_marks_restyled_prompt_idle_from_footer_chrome() {
        let mut cursor = pane_output_status_pane(761, Provider::CursorCli, "Cursor Agent");

        classify::apply_pane_output_status_fallback(
            &mut cursor,
            "  Cursor Agent\n\
         \n\
         ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄\n\
          → Ask Cursor anything\n\
         ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n\
          Composer 2.5                                                   Auto-run -- INSERT --\n\
          /private/tmp/agentscan-cursor-smoke · main\n",
        );

        assert_eq!(cursor.status.kind, StatusKind::Idle);
        assert_eq!(cursor.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn cursor_cli_pane_output_withholds_status_without_current_prompt_chrome() {
        let mut cursor = pane_output_status_pane(762, Provider::CursorCli, "Cursor Agent");

        classify::apply_pane_output_status_fallback(
            &mut cursor,
            "  Cursor Agent\n\
         \n\
           → Ask Cursor anything\n\
         \n\
         response text with no current composer or path footer\n",
        );

        assert_eq!(cursor.status.kind, StatusKind::Unknown);
        assert_eq!(cursor.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn cursor_cli_pane_output_marks_borderless_stop_hint_busy() {
        let mut cursor = pane_output_status_pane(759, Provider::CursorCli, "Command Runner");

        classify::apply_pane_output_status_fallback(
            &mut cursor,
            "$ sleep 90 40s\n\
           ctrl+b twice to send to background\n\
         \n\
         ⠘⠤ Running  46 tokens\n\
            Tip: Use subagents to parallelize work and preserve context.\n\
         \n\
           → Add a follow-up                                                     ctrl+c to stop\n\
         \n\
         \n\
           1 task\n\
           Composer 2.5                                             Run Everything -- INSERT --\n\
           /private/tmp/agentscan-cursor-smoke · main\n",
        );

        assert_eq!(cursor.status.kind, StatusKind::Busy);
        assert_eq!(cursor.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn cursor_cli_pane_output_ignores_stale_borderless_stop_hint() {
        let mut cursor = pane_output_status_pane(760, Provider::CursorCli, "Command Runner");

        classify::apply_pane_output_status_fallback(
            &mut cursor,
            "$ sleep 90 40s\n\
           ctrl+b twice to send to background\n\
         \n\
         ⠘⠤ Running  46 tokens\n\
            Tip: Use subagents to parallelize work and preserve context.\n\
         \n\
           → Add a follow-up                                                     ctrl+c to stop\n\
         \n\
         Completed. I ran the requested command.\n\
         There is no current Cursor composer footer below this output.\n",
        );

        assert_eq!(cursor.status.kind, StatusKind::Unknown);
        assert_eq!(cursor.status.source, crate::app::StatusSource::NotChecked);
    }
}
