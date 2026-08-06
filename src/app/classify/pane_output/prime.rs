use super::{PaneOutputFrame, StatusKind};

// Prime Agent renders its input prompt and footer while a turn runs, drawing a
// spinner loader line above them: `⠸ Executing · 3s · ↑ 34 tokens`. The label set
// comes from AGENT_ACTIVITY_LABELS in Prime's interactive mode; `Waiting` there
// means model latency, not a human-blocking question, so it is busy — Prime never
// reports `waiting` from pane output.
const ACTIVITY_LABELS: &[&str] = &[
    "Waiting",
    "Thinking",
    "Writing code",
    "Writing",
    "Executing",
];

/// How far above the current footer the loader line may sit: observed frames put
/// blank, tool-summary, hint, and prompt rows between them (up to seven rows in
/// captures), so allow a small window and treat anything further up as scrollback.
const MAX_ROWS_LOADER_TO_FOOTER: usize = 8;
/// The prompt row sits directly above the footer; allow slack for hint rows.
const MAX_ROWS_PROMPT_TO_FOOTER: usize = 4;
/// The footer is the last rendered line today; a small window leaves room for
/// future status rows without accepting a footer buried in scrollback.
const FOOTER_TAIL_WINDOW: usize = 3;

pub(super) fn status(output: &str) -> Option<StatusKind> {
    let frame = PaneOutputFrame::new(output);
    let footer_index = prime_current_footer_index(&frame)?;

    if let Some(busy_index) = frame.rposition(prime_busy_loader_line)
        && busy_index <= footer_index
        && footer_index - busy_index <= MAX_ROWS_LOADER_TO_FOOTER
    {
        return Some(StatusKind::Busy);
    }

    // Any other spinner-led row is a live-turn signal this matcher cannot
    // confirm: the recognized loader pushed outside the window by wrapped rows
    // in a narrow pane, or a spinner whose label/elapsed shape changed in a
    // Prime restyle. Both degrade to unknown rather than reading the still-
    // rendered prompt as idle — the forbidden busy→idle inversion.
    if frame.rposition(prime_spinner_led_line).is_some() {
        return None;
    }

    let prompt_index = frame.rposition(prime_prompt_line)?;
    (prompt_index <= footer_index && footer_index - prompt_index <= MAX_ROWS_PROMPT_TO_FOOTER)
        .then_some(StatusKind::Idle)
}

fn prime_current_footer_index(frame: &PaneOutputFrame<'_>) -> Option<usize> {
    frame
        .rposition(prime_footer_line)
        .filter(|&index| frame.is_within_tail(index, FOOTER_TAIL_WINDOW))
}

/// The Prime footer line: `← agents/resume  GPT-5.6 Sol • medium  …  6.0k (2%)`.
/// Durable anchors are the `•` separator and the right-aligned context pair —
/// a count (`0`, `34`, `6.0k`) followed by a parenthesized percentage. Pi footers
/// carry `%/` or `?/N` context tokens instead, so the two matchers are mutually
/// exclusive by construction.
fn prime_footer_line(line: &str) -> bool {
    let line = line.trim();
    if !line.contains(" • ") {
        return false;
    }
    let mut tokens = line.split_whitespace().rev();
    let Some(percent) = tokens.next() else {
        return false;
    };
    let Some(count) = tokens.next() else {
        return false;
    };
    prime_percent_token(percent) && prime_count_token(count)
}

fn prime_percent_token(token: &str) -> bool {
    token
        .strip_prefix('(')
        .and_then(|token| token.strip_suffix("%)"))
        .is_some_and(|digits| !digits.is_empty() && digits.chars().all(|ch| ch.is_ascii_digit()))
}

fn prime_count_token(token: &str) -> bool {
    let digits = token.strip_suffix('k').unwrap_or(token);
    !digits.is_empty()
        && digits.chars().filter(|ch| *ch == '.').count() <= 1
        && digits.chars().all(|ch| ch.is_ascii_digit() || ch == '.')
        && digits.chars().any(|ch| ch.is_ascii_digit())
}

/// The loader line: a braille spinner glyph, an activity label, and a `· <elapsed>`
/// segment (`⠇ Waiting · 0s`, `⠸ Executing · 3s · ↑ 34 tokens`). The token counter
/// is a corroborator that may be absent; bare activity words echoed into the
/// transcript lack the spinner glyph and never match.
fn prime_busy_loader_line(line: &str) -> bool {
    if !prime_spinner_led_line(line) {
        return false;
    }
    let line = line.trim_start();
    let spinner = line
        .chars()
        .next()
        .expect("spinner-led line has a first char");
    let rest = line[spinner.len_utf8()..].trim_start();
    let Some((label, after)) = rest.split_once(" · ") else {
        return false;
    };
    ACTIVITY_LABELS.contains(&label) && after.split(" · ").next().is_some_and(prime_elapsed_segment)
}

/// A row led by a braille spinner glyph — the loader family, whether or not the
/// rest of the row matches the shape this matcher recognizes.
fn prime_spinner_led_line(line: &str) -> bool {
    line.trim_start()
        .chars()
        .next()
        .is_some_and(|ch| ('\u{2800}'..='\u{28ff}').contains(&ch))
}

/// The elapsed segment after the label: `0s`, `12s`, `1m 5s`.
fn prime_elapsed_segment(segment: &str) -> bool {
    let mut parts = segment.split_whitespace();
    let Some(first) = parts.next() else {
        return false;
    };
    first
        .strip_suffix(['s', 'm', 'h'])
        .is_some_and(|digits| !digits.is_empty() && digits.chars().all(|ch| ch.is_ascii_digit()))
}

/// Prime's input prompt row (` >   Try "explain how @<filepath> works"` or the
/// user's typed text). It stays rendered during a turn, so it corroborates a live
/// frame rather than distinguishing busy from idle on its own.
fn prime_prompt_line(line: &str) -> bool {
    let line = line.trim_start();
    line == ">" || line.starts_with("> ")
}

#[cfg(test)]
mod tests {
    use crate::app::classify;
    use crate::app::tests::pane_output_status_pane;
    use crate::app::{Provider, StatusKind};

    fn prime_pane(pid: u32) -> crate::app::PaneRecord {
        pane_output_status_pane(pid, Provider::Prime, "prime-agent - agentscan")
    }

    #[test]
    fn prime_pane_output_marks_loader_above_current_footer_busy() {
        let mut pane = prime_pane(801);
        classify::apply_pane_output_status_fallback(
            &mut pane,
            " ◆ bash · sleep 15 · ↑ 1 lines · Ctrl+O to expand\n\
             ⠸ Executing · 3s · ↑ 34 tokens\n\
             >   Try \"explain how @<filepath> works\"\n\
            ← agents/resume  GPT-5.6 Sol • medium                                    34 (0%)\n",
        );

        assert_eq!(pane.status.kind, StatusKind::Busy);
        assert_eq!(pane.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn prime_pane_output_marks_waiting_loader_busy_not_waiting() {
        // Prime's `Waiting` activity label is model latency, not a human-blocking
        // question, so it must read busy.
        let mut pane = prime_pane(802);
        classify::apply_pane_output_status_fallback(
            &mut pane,
            " ⠇ Waiting · 0s\n\
             >   Try \"explain how @<filepath> works\"\n\
            ← agents/resume  GPT-5.6 Sol • medium                                     0 (0%)\n",
        );

        assert_eq!(pane.status.kind, StatusKind::Busy);
    }

    #[test]
    fn prime_pane_output_marks_prompt_and_footer_idle() {
        let mut pane = prime_pane(803);
        classify::apply_pane_output_status_fallback(
            &mut pane,
            " ✓ bash · sleep 15 · ↑ 1 lines · 15.0s · Ctrl+O to expand\n\
             done\n\
             >   Try \"explain how @<filepath> works\"\n\
            ← agents/resume  GPT-5.6 Sol • medium                                  6.0k (2%)\n",
        );

        assert_eq!(pane.status.kind, StatusKind::Idle);
        assert_eq!(pane.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn prime_pane_output_ignores_activity_words_without_spinner_glyph() {
        // Transcript prose echoing an activity label must never read busy.
        let mut pane = prime_pane(804);
        classify::apply_pane_output_status_fallback(
            &mut pane,
            " The loader shows Executing · 3s while a tool runs.\n\
             >   Try \"explain how @<filepath> works\"\n\
            ← agents/resume  GPT-5.6 Sol • medium                                  6.0k (2%)\n",
        );

        assert_eq!(pane.status.kind, StatusKind::Idle);
    }

    #[test]
    fn prime_pane_output_loader_outside_window_degrades_to_unknown_not_idle() {
        // Wrapped hint/tool rows in a narrow pane can push a live loader past the
        // fixed geometry window; that frame is ambiguous and must not read idle.
        let mut pane = prime_pane(808);
        classify::apply_pane_output_status_fallback(
            &mut pane,
            " ⠸ Executing · 3s · ↑ 34 tokens\n\
             wrapped row\n\
             wrapped row\n\
             wrapped row\n\
             wrapped row\n\
             wrapped row\n\
             wrapped row\n\
             wrapped row\n\
             wrapped row\n\
             >   Try \"explain how @<filepath> works\"\n\
            ← agents/resume  GPT-5.6 Sol • medium                                  6.0k (2%)\n",
        );

        assert_eq!(pane.status.kind, StatusKind::Unknown);
        assert_eq!(pane.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn prime_pane_output_unrecognized_spinner_shape_degrades_to_unknown_not_idle() {
        // A restyled loader (new label or elapsed format) still signals a live
        // turn; the prompt below it must not read idle.
        let mut pane = prime_pane(809);
        classify::apply_pane_output_status_fallback(
            &mut pane,
            " ⠸ Reticulating — 3 sec\n\
             >   Try \"explain how @<filepath> works\"\n\
            ← agents/resume  GPT-5.6 Sol • medium                                  6.0k (2%)\n",
        );

        assert_eq!(pane.status.kind, StatusKind::Unknown);
        assert_eq!(pane.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn prime_pane_output_without_footer_stays_unknown() {
        let mut pane = prime_pane(805);
        classify::apply_pane_output_status_fallback(
            &mut pane,
            " ⠸ Executing · 3s · ↑ 34 tokens\n\
             >   Try \"explain how @<filepath> works\"\n",
        );

        assert_eq!(pane.status.kind, StatusKind::Unknown);
        assert_eq!(pane.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn prime_pane_output_ignores_footer_buried_in_scrollback() {
        let mut pane = prime_pane(806);
        classify::apply_pane_output_status_fallback(
            &mut pane,
            "← agents/resume  GPT-5.6 Sol • medium                                  6.0k (2%)\n\
             one\n\
             two\n\
             three\n\
             four\n",
        );

        assert_eq!(pane.status.kind, StatusKind::Unknown);
    }

    #[test]
    fn prime_pane_output_footer_without_prompt_or_loader_stays_unknown() {
        // A footer left behind by an exited Prime, with a shell prompt below.
        let mut pane = prime_pane(807);
        classify::apply_pane_output_status_fallback(
            &mut pane,
            "← agents/resume  GPT-5.6 Sol • medium                                  6.0k (2%)\n\
             auro@koopa ~/code/app %\n",
        );

        assert_eq!(pane.status.kind, StatusKind::Unknown);
    }

    #[test]
    fn prime_frames_do_not_match_pi_and_pi_frames_do_not_match_prime() {
        let prime_frame = " ⠇ Waiting · 0s\n\
             >   Try \"explain how @<filepath> works\"\n\
            ← agents/resume  GPT-5.6 Sol • medium                                     0 (0%)\n";
        assert_eq!(classify::classify_output(Provider::Pi, prime_frame), None);

        let pi_frame = "────────────────────────────────\n\
             \n\
            ────────────────────────────────\n\
            ~/code/app\n\
            0.0%/200k                                      claude-sonnet\n";
        assert_eq!(classify::classify_output(Provider::Prime, pi_frame), None);
    }
}
