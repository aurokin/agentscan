use super::{PaneOutputFrame, StatusKind};

// kimi renders a bordered input box (`│ > … │`) at the current prompt in every state.
const INPUT_BOX_MARKER: &str = "│ >";
// While a turn runs, a moon-phase spinner line (`<moon> · <tip>`) is rendered directly
// above the input box; it is absent once the turn completes.
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

    // A moon-glyph line without the observed ` · ` separator is ambiguous: echoed
    // output, or a future release restyling the tip separator. Withhold status rather
    // than letting a decorative-string mismatch silently flip busy to idle.
    if current_prompt_lines.iter().any(|line| kimi_moon_line(line)) {
        return None;
    }

    // The current input box is present with no live spinner above it. Other states
    // (approval dialogs, alternate UIs) drop the box and fall through to `None`.
    Some(StatusKind::Idle)
}

fn kimi_input_box_line(line: &str) -> bool {
    line.trim_start().starts_with(INPUT_BOX_MARKER)
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
