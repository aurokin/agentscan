use super::{PaneOutputFrame, StatusKind};

// Antigravity idle footer shown below the `>` input box while awaiting input.
const IDLE_FOOTER_MARKER: &str = "? for shortcuts";
// Antigravity busy footer shown during an active turn (with a spinner above).
const BUSY_FOOTER_MARKER: &str = "esc to cancel";

pub(super) fn status(output: &str) -> Option<StatusKind> {
    let frame = PaneOutputFrame::new(output);

    // Antigravity (closed-source) flips its own footer below the bordered `>` input box between
    // an idle prompt (`? for shortcuts`) and an active turn (`esc to cancel`, shown with a
    // `… Generating…`/`… Loading…` spinner above), so the footer is the current-frame busy/idle
    // anchor. Require the `>` box just above and only blank rows after it, so a stale scrollback
    // line carrying the phrase is not mistaken for the live footer; anything else stays unknown
    // rather than risk a guessed state.
    let footer_index = frame.rposition(|line| {
        antigravity_idle_footer_line(line) || antigravity_busy_footer_line(line)
    })?;
    let only_blank_after =
        frame.trailing_lines_after_are(footer_index, |_, line, _| line.trim().is_empty());
    if !(only_blank_after && antigravity_prompt_above_footer(&frame, footer_index)) {
        return None;
    }

    Some(
        if frame
            .line(footer_index)
            .is_some_and(antigravity_busy_footer_line)
        {
            StatusKind::Busy
        } else {
            StatusKind::Idle
        },
    )
}

fn antigravity_idle_footer_line(line: &str) -> bool {
    line.trim_start().starts_with(IDLE_FOOTER_MARKER)
}

fn antigravity_busy_footer_line(line: &str) -> bool {
    line.trim_start().starts_with(BUSY_FOOTER_MARKER)
}

fn antigravity_prompt_above_footer(frame: &PaneOutputFrame<'_>, footer_index: usize) -> bool {
    frame.window_before(footer_index, 6).is_some_and(|lines| {
        lines.iter().any(|line| {
            let line = line.trim();
            line == ">" || line.starts_with("> ")
        })
    })
}

#[cfg(test)]
mod tests {
    use crate::app::classify;
    use crate::app::tests::{pane_output_status_pane, proc_fallback_pane};
    use crate::app::{Provider, StatusKind};

    #[test]
    fn antigravity_pane_output_marks_current_prompt_idle_only_after_provider_is_known() {
        // Mirrors a real idle antigravity pane: the bordered `>` prompt and `? for shortcuts`
        // footer sit at the top of a much taller pane padded with blank rows below.
        let idle_screen = "Antigravity CLI 1.0.1\n\
         auro@hsadler.com\n\
         Gemini 3.5 Flash (Medium)\n\
         ~/code/agentscan\n\
         ────────────────────────────────────────────────\n\
         >\n\
         ────────────────────────────────────────────────\n\
         ? for shortcuts                       Gemini 3.5 Flash (Medium)\n\
         \n\
         \n\
         \n\
         \n";

        let mut antigravity =
            pane_output_status_pane(795, Provider::Antigravity, "koopa.home.arpa");
        classify::apply_pane_output_status_fallback(&mut antigravity, idle_screen);

        assert_eq!(antigravity.status.kind, StatusKind::Idle);
        assert_eq!(
            antigravity.status.source,
            crate::app::StatusSource::PaneOutput
        );

        let mut unknown = proc_fallback_pane(796, "zsh", "custom title");
        classify::apply_pane_output_status_fallback(&mut unknown, idle_screen);

        assert_eq!(unknown.status.kind, StatusKind::Unknown);
        assert_eq!(unknown.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn antigravity_pane_output_marks_active_turn_busy() {
        // Mirrors a real busy antigravity pane (CLI 1.0.2): a `… Generating…` spinner above the
        // `>` box, with the footer flipped from `? for shortcuts` to `esc to cancel` — the
        // current-frame busy anchor in the same position as the idle footer.
        let mut antigravity =
            pane_output_status_pane(802, Provider::Antigravity, "koopa.home.arpa");

        classify::apply_pane_output_status_fallback(
            &mut antigravity,
            "● Read(/Users/auro/code/agentscan/src/app/proc.rs) (ctrl+o to expand)\n\
         ⡿ Generating...\n\
         ────────────────────────────────────────────────\n\
         >\n\
         ────────────────────────────────────────────────\n\
         esc to cancel                         Gemini 3.5 Flash (Medium)\n\
         \n\
         \n",
        );

        assert_eq!(antigravity.status.kind, StatusKind::Busy);
        assert_eq!(
            antigravity.status.source,
            crate::app::StatusSource::PaneOutput
        );
    }

    #[test]
    fn antigravity_pane_output_marks_busy_with_stale_idle_footer_in_scrollback() {
        // A prior turn's idle footer sits in scrollback above a fresh active turn. Because the
        // live bottom footer is `esc to cancel`, the pane is busy — the stale idle footer above
        // must not win.
        let mut antigravity =
            pane_output_status_pane(803, Provider::Antigravity, "koopa.home.arpa");

        classify::apply_pane_output_status_fallback(
            &mut antigravity,
            "? for shortcuts                       Gemini 3.5 Flash (Medium)\n\
         ⡿ Generating...\n\
         ────────────────────────────────────────────────\n\
         >\n\
         ────────────────────────────────────────────────\n\
         esc to cancel                         Gemini 3.5 Flash (Medium)\n",
        );

        assert_eq!(antigravity.status.kind, StatusKind::Busy);
        assert_eq!(
            antigravity.status.source,
            crate::app::StatusSource::PaneOutput
        );
    }

    #[test]
    fn antigravity_pane_output_leaves_footerless_screen_unknown() {
        // Busy/idle are anchored on the footer below the `>` box. Free prose with neither footer
        // (nor the box) stays unknown rather than risk a guessed state.
        let mut antigravity =
            pane_output_status_pane(797, Provider::Antigravity, "koopa.home.arpa");

        classify::apply_pane_output_status_fallback(
            &mut antigravity,
            "Working on the request\n\
         The described approach will stop the leak.\n\
         Streaming the diff now\n",
        );

        assert_eq!(antigravity.status.kind, StatusKind::Unknown);
        assert_eq!(
            antigravity.status.source,
            crate::app::StatusSource::NotChecked
        );
    }

    #[test]
    fn antigravity_pane_output_does_not_infer_idle_from_scrolled_away_footer() {
        let mut antigravity =
            pane_output_status_pane(798, Provider::Antigravity, "koopa.home.arpa");

        classify::apply_pane_output_status_fallback(
            &mut antigravity,
            "? for shortcuts                       Gemini 3.5 Flash (Medium)\n\
         Reading files\n\
         Planning edits\n\
         Running tests\n\
         Current line\n",
        );

        assert_eq!(antigravity.status.kind, StatusKind::Unknown);
        assert_eq!(
            antigravity.status.source,
            crate::app::StatusSource::NotChecked
        );
    }

    #[test]
    fn antigravity_pane_output_idle_survives_stale_cancel_hint_in_scrollback() {
        // A prior turn's cancel hint is still in the scrollback capture above a fresh idle
        // footer. Because the live footer is the current bottom, the pane is idle — the stale
        // cancel hint must not force busy.
        let mut antigravity =
            pane_output_status_pane(801, Provider::Antigravity, "koopa.home.arpa");

        classify::apply_pane_output_status_fallback(
            &mut antigravity,
            "esc to cancel                         Gemini 3.5 Flash (Medium)\n\
         Done with the previous request.\n\
         ────────────────────────────────────────────────\n\
         >\n\
         ────────────────────────────────────────────────\n\
         ? for shortcuts                       Gemini 3.5 Flash (Medium)\n",
        );

        assert_eq!(antigravity.status.kind, StatusKind::Idle);
        assert_eq!(
            antigravity.status.source,
            crate::app::StatusSource::PaneOutput
        );
    }

    #[test]
    fn antigravity_pane_output_does_not_infer_idle_with_output_just_below_footer() {
        // A single output row below the `? for shortcuts` footer means it is a stale frame; only
        // blank rows may follow the current idle footer.
        let mut antigravity =
            pane_output_status_pane(800, Provider::Antigravity, "koopa.home.arpa");

        classify::apply_pane_output_status_fallback(
            &mut antigravity,
            "────────────────────────────────────────────────\n\
         >\n\
         ────────────────────────────────────────────────\n\
         ? for shortcuts                       Gemini 3.5 Flash (Medium)\n\
         Reading files\n",
        );

        assert_eq!(antigravity.status.kind, StatusKind::Unknown);
        assert_eq!(
            antigravity.status.source,
            crate::app::StatusSource::NotChecked
        );
    }
}
