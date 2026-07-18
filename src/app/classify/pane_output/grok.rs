use super::{PaneOutputFrame, StatusKind, is_version_like_command};

// grok keybind footer key specs (`Shift+Tab:mode`, `Ctrl+.:shortcuts`).
const MODE_KEYBIND_KEY: &str = "Shift+Tab";
const CTRL_KEYBIND_PREFIX: &str = "Ctrl+";
// grok active-turn interrupt keybind actions (`Ctrl+c:cancel`, `Ctrl+Enter:interject`).
const CANCEL_ACTION: &str = "cancel";
const INTERJECT_ACTION: &str = "interject";

pub(super) fn status(output: &str) -> Option<StatusKind> {
    let frame = PaneOutputFrame::new(output);

    // grok pins its rounded input box (`│ ❯ … │`) at the bottom in both idle and busy states.
    // The keybind footer just below it reflects grok's own state: an in-flight turn adds
    // `Ctrl+c:cancel` / `Ctrl+Enter:interject` hints, an idle prompt shows only
    // mode/shortcuts, and a fresh prompt shows the version line (e.g. `0.2.3 [stable] Beta`).
    if let Some(border) = grok_current_input_box_border(&frame) {
        // Busy when grok's footer shows interrupt keybinds OR a live run spinner sits
        // directly above the pinned box. The footer wording is the primary signal; the
        // spinner backstop keeps a live turn from reading idle if grok relabels its hints,
        // while a stale spinner (completed-turn output between it and the box) is not
        // directly above the box and so won't trip it.
        let busy = grok_active_turn_footer_after(&frame, border)
            || grok_running_spinner_above_box(&frame, border);
        if busy {
            return Some(StatusKind::Busy);
        }
        return grok_idle_footer_after(&frame, border).then_some(StatusKind::Idle);
    }

    // No current input box (a transient mid-stream frame): a running spinner as the current
    // bottom line still means an in-flight turn.
    grok_current_running_spinner(&frame).then_some(StatusKind::Busy)
}

/// Index of the input box bottom border when the box is the current bottom frame.
///
/// The capture is already trailing-trimmed, so the box is current when its `╰ … ─╯` bottom
/// border has the `│ ❯ … │` input row directly above it and only grok's own footer chrome
/// (blank rows, the keybind footer, or the version line) below it. A box trailed by real turn
/// output is a stale frame in the scrollback capture, not the live prompt.
fn grok_current_input_box_border(frame: &PaneOutputFrame<'_>) -> Option<usize> {
    let border = frame.rposition(grok_prompt_box_bottom_border)?;
    let input_above = border > 0
        && frame
            .line(border - 1)
            .is_some_and(grok_prompt_box_input_line);
    let only_footer_below = frame.trailing_lines_after_are(border, |_, line, _| {
        let line = line.trim();
        line.is_empty() || grok_footer_line(line)
    });
    (input_above && only_footer_below).then_some(border)
}

fn grok_prompt_box_input_line(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with('│') && line.contains('❯')
}

fn grok_prompt_box_bottom_border(line: &str) -> bool {
    let line = line.trim();
    line.starts_with('╰') && line.ends_with('╯')
}

fn grok_prompt_box_top_border(line: &str) -> bool {
    let line = line.trim();
    line.starts_with('╭') && line.ends_with('╮')
}

/// grok's footer chrome directly below the input box: the keybind hints
/// (`Shift+Tab:mode │ Ctrl+.:shortcuts`), the version line (`0.2.3 [stable] Beta`), or the
/// bracketed release-channel-only form (`[stable]`). Matched by shape so a stale turn line below
/// the box is not mistaken for chrome.
fn grok_footer_line(line: &str) -> bool {
    grok_keybind_footer_line(line)
        || grok_version_footer_line(line)
        || grok_release_channel_footer_line(line)
}

fn grok_keybind_footer_line(line: &str) -> bool {
    grok_footer_tokens(line).any(grok_keybind_token)
}

fn grok_footer_tokens(line: &str) -> impl Iterator<Item = &str> {
    line.split(|ch: char| ch.is_whitespace() || ch == '│')
        .filter(|token| !token.is_empty())
}

/// A grok keybind hint token such as `Shift+Tab:mode`, `Ctrl+.:shortcuts`, or `Ctrl+c:cancel` —
/// a key spec bound to an action via `:`. Requiring the `Key+…:action` shape excludes prose that
/// merely mentions `Shift+Tab` or `Ctrl+C`, so a stale scrollback line is not mistaken for the
/// live footer chrome.
fn grok_keybind_token(token: &str) -> bool {
    let Some((key, action)) = token.split_once(':') else {
        return false;
    };
    !action.is_empty() && (key == MODE_KEYBIND_KEY || key.starts_with(CTRL_KEYBIND_PREFIX))
}

/// An active turn's footer adds interrupt keybinds (`Ctrl+c:cancel`, `Ctrl+Enter:interject`)
/// that the idle footer omits, so grok's own footer distinguishes a running turn from an idle
/// prompt without having to anchor the spinner against stale scrollback.
fn grok_active_turn_footer_after(frame: &PaneOutputFrame<'_>, border: usize) -> bool {
    frame.trailing_lines_after_any(border, |line| {
        let line = line.trim();
        grok_keybind_footer_line(line)
            && (line.contains(CANCEL_ACTION) || line.contains(INTERJECT_ACTION))
    })
}

/// An idle conclusion needs positive footer evidence rather than the absence of known busy
/// words. Every rendered row below the box must be known chrome, and keybind rows are idle-only
/// only when every shaped keybind names an observed idle action (`mode` or `shortcuts`).
fn grok_idle_footer_after(frame: &PaneOutputFrame<'_>, border: usize) -> bool {
    let mut saw_footer = false;
    let only_idle_footer = frame.trailing_lines_after_are(border, |_, line, _| {
        let line = line.trim();
        if line.is_empty() {
            return true;
        }
        saw_footer = true;
        grok_idle_footer_line(line)
    });
    saw_footer && only_idle_footer
}

fn grok_idle_footer_line(line: &str) -> bool {
    if grok_keybind_footer_line(line) {
        return grok_footer_tokens(line).all(|token| {
            let Some((_, action)) = token.split_once(':') else {
                return false;
            };
            grok_keybind_token(token) && matches!(action, "mode" | "shortcuts")
        });
    }
    grok_version_footer_line(line) || grok_release_channel_footer_line(line)
}

/// True when a live run spinner sits directly above the current input box — only blank rows
/// between the spinner (`⠋ … [✗]`) and the box's top border. grok renders the active-turn
/// spinner right above the pinned box, so this marks a running turn independent of footer
/// wording (a backstop if grok relabels its interrupt hints). A *stale* spinner from a prior
/// turn has real output (e.g. `Turn completed…`) between it and the box, so it is not directly
/// above and does not match.
fn grok_running_spinner_above_box(frame: &PaneOutputFrame<'_>, border: usize) -> bool {
    let Some(top) = frame.rposition_before(border, grok_prompt_box_top_border) else {
        return false;
    };
    frame
        .previous_nonblank_before(top)
        .is_some_and(grok_running_status_line)
}

/// Grok's version footer, e.g. `0.2.3 [stable] Beta`. The dotted version leads the line and any
/// trailing tokens are release-channel labels, so neither prose that mentions a version
/// mid-sentence (`See 0.2.3 docs`) nor version-led prose (`0.2.3 docs`, `1.4.0 release notes`)
/// is mistaken for chrome.
fn grok_version_footer_line(line: &str) -> bool {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let Some((first, rest)) = tokens.split_first() else {
        return false;
    };
    is_version_like_command(first)
        && rest.len() <= 2
        && rest.iter().all(|t| grok_release_channel_word(t))
}

// A grok release-channel label such as `stable`, `Beta`, or the bracketed `[stable]` form.
fn grok_release_channel_word(token: &str) -> bool {
    let token = token.trim_matches(|ch| matches!(ch, '[' | ']' | '(' | ')'));
    matches!(
        token.to_ascii_lowercase().as_str(),
        "stable"
            | "beta"
            | "alpha"
            | "rc"
            | "dev"
            | "nightly"
            | "canary"
            | "preview"
            | "experimental"
            | "latest"
            | "edge"
    )
}

fn grok_release_channel_footer_line(line: &str) -> bool {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    matches!(tokens.as_slice(), [token] if grok_bracketed_release_channel_word(token))
}

fn grok_bracketed_release_channel_word(token: &str) -> bool {
    token.starts_with('[') && token.ends_with(']') && grok_release_channel_word(token)
}

// True when the bottom-most rendered line is a running spinner: a braille spinner glyph with
// grok's bracketed single-glyph run marker. The observed `[✗]` spelling corroborates this
// shape, but the durable anchor is a non-ASCII-alphanumeric glyph in brackets; rejecting ASCII
// letters and digits avoids treating prose annotations such as `[a]` as run markers.
fn grok_current_running_spinner(frame: &PaneOutputFrame<'_>) -> bool {
    frame.last_nonblank().is_some_and(grok_running_status_line)
}

fn grok_running_status_line(line: &str) -> bool {
    let line = line.trim_start();
    line.chars()
        .next()
        .is_some_and(|ch| ('\u{2800}'..='\u{28ff}').contains(&ch))
        && line.char_indices().any(|(index, ch)| {
            if ch != '[' {
                return false;
            }
            let mut marker = line[index + ch.len_utf8()..].chars();
            marker.next().is_some_and(|glyph| {
                !glyph.is_ascii_alphanumeric()
                    && !glyph.is_whitespace()
                    && !matches!(glyph, '[' | ']')
                    && marker.next() == Some(']')
            })
        })
}

#[cfg(test)]
mod tests {
    use crate::app::classify;
    use crate::app::tests::{pane_output_status_pane, proc_fallback_pane};
    use crate::app::{Provider, StatusKind};

    #[test]
    fn grok_pane_output_marks_current_prompt_box_idle_only_after_provider_is_known() {
        // Mirrors a real fresh idle grok pane (v0.2.3 capture): the rounded input box is the
        // current bottom UI with the version line below it, and the rest of the taller pane is
        // blank padding.
        let idle_screen = "   main ~/code/agentscan/\n\
         \n\
         Tip: Press Ctrl-W to start a parallel task in its own worktree.\n\
         \n\
         ╭────────────────────────────────────────────────╮\n\
         │ ❯                                                │\n\
         ╰──────────────── Grok Build · always-approve ─╯\n\
         \n\
         0.2.3 [stable] Beta\n\
         \n\
         \n\
         \n\
         \n";

        let mut grok = pane_output_status_pane(769, Provider::Grok, "grok");
        classify::apply_pane_output_status_fallback(&mut grok, idle_screen);

        assert_eq!(grok.status.kind, StatusKind::Idle);
        assert_eq!(grok.status.source, crate::app::StatusSource::PaneOutput);

        let mut unknown = proc_fallback_pane(770, "zsh", "custom title");
        classify::apply_pane_output_status_fallback(&mut unknown, idle_screen);

        assert_eq!(unknown.status.kind, StatusKind::Unknown);
        assert_eq!(unknown.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn grok_pane_output_marks_channel_only_footer_idle() {
        // Observed from Grok Build Beta 0.2.60: the fresh prompt footer can be just `[stable]`
        // below the input box, with no dotted version token.
        let mut grok = pane_output_status_pane(771, Provider::Grok, "grok");

        classify::apply_pane_output_status_fallback(
            &mut grok,
            "╭────────────────────────────────────────────────────────────────────────────╮\n\
         │ ❯                                                                          │\n\
         ╰──────────────────────────────────── Composer 2.5 Fast · always-approve ─╯\n\
         \n\
         [stable]\n",
        );

        assert_eq!(grok.status.kind, StatusKind::Idle);
        assert_eq!(grok.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn grok_pane_output_does_not_treat_plain_channel_word_below_box_as_chrome() {
        let mut grok = pane_output_status_pane(783, Provider::Grok, "grok");

        classify::apply_pane_output_status_fallback(
            &mut grok,
            "╭────────────────────────────────────────────────────────────────────────────╮\n\
         │ ❯                                                                          │\n\
         ╰──────────────────────────────────── Composer 2.5 Fast · always-approve ─╯\n\
         \n\
         stable\n",
        );

        assert_eq!(grok.status.kind, StatusKind::Unknown);
        assert_eq!(grok.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn grok_pane_output_marks_used_session_keybind_footer_idle() {
        // Mirrors a real used grok session (v0.2.3): after a completed turn the input box is the
        // current bottom UI with the idle keybind footer (mode/shortcuts only) below it.
        let mut grok = pane_output_status_pane(778, Provider::Grok, "grok");

        classify::apply_pane_output_status_fallback(
            &mut grok,
            "     ❯ hi                                          2:23 PM\n\
         \n\
         Turn completed in 1.9s.\n\
         \n\
         ╭────────────────────────────────────────────────╮\n\
         │ ❯                                                │\n\
         ╰──────────────── Grok Build · always-approve ─╯\n\
         \n\
         Shift+Tab:mode  │  Ctrl+.:shortcuts\n",
        );

        assert_eq!(grok.status.kind, StatusKind::Idle);
        assert_eq!(grok.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn grok_pane_output_marks_active_turn_footer_busy() {
        // Mirrors a real busy grok pane (v0.2.3): the input box stays pinned at the bottom during
        // a turn, with the running spinner above it and the active-turn footer (adding
        // cancel/interject keybinds) below it.
        let mut grok = pane_output_status_pane(779, Provider::Grok, "grok");

        classify::apply_pane_output_status_fallback(
            &mut grok,
            "     ◆ Search \"disable_reconcile\" in src (28 matches)\n\
         \n\
         ⠹ Thinking… 0.4s                              42s ⇣80.3k [✗]\n\
         \n\
         ╭────────────────────────────────────────────────╮\n\
         │ ❯                                                │\n\
         ╰──────────────── Grok Build · always-approve ─╯\n\
         \n\
         Shift+Tab:mode  │  Ctrl+c:cancel  │  Ctrl+Enter:interject  │  Ctrl+.:shortcuts\n",
        );

        assert_eq!(grok.status.kind, StatusKind::Busy);
        assert_eq!(grok.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn grok_pane_output_marks_active_turn_busy_via_spinner_when_footer_reworded() {
        // Same live-turn layout as the active-turn footer case, but the footer hints are reworded
        // so `cancel`/`interject` are absent (mirrors grok relabeling its interrupt keybinds). The
        // run spinner sitting directly above the pinned box still proves the turn is in flight, so
        // the pane stays busy without depending on the footer wording.
        let mut grok = pane_output_status_pane(780, Provider::Grok, "grok");

        classify::apply_pane_output_status_fallback(
            &mut grok,
            "     ◆ Search \"disable_reconcile\" in src (28 matches)\n\
         \n\
         ⠹ Thinking… 0.4s                              42s ⇣80.3k [■]\n\
         \n\
         ╭────────────────────────────────────────────────╮\n\
         │ ❯                                                │\n\
         ╰──────────────── Grok Build · always-approve ─╯\n\
         \n\
         Shift+Tab:mode  │  Ctrl+x:stop  │  Ctrl+.:shortcuts\n",
        );

        assert_eq!(grok.status.kind, StatusKind::Busy);
        assert_eq!(grok.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn grok_pane_output_leaves_unrecognized_keybind_action_unknown() {
        let mut grok = pane_output_status_pane(784, Provider::Grok, "grok");

        classify::apply_pane_output_status_fallback(
            &mut grok,
            "╭────────────────────────────────────────────────╮\n\
         │ ❯                                                │\n\
         ╰────────────── Grok Build · always-approve ─╯\n\
         \n\
         Shift+Tab:mode  │  Ctrl+x:stop  │  Ctrl+.:shortcuts\n",
        );

        assert_eq!(grok.status.kind, StatusKind::Unknown);
        assert_eq!(grok.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn grok_pane_output_marks_running_spinner_busy() {
        let mut grok = pane_output_status_pane(771, Provider::Grok, "grok");

        classify::apply_pane_output_status_fallback(
            &mut grok,
            "╭────────────────────────────────────────────────╮\n\
         │ ❯                                                │\n\
         ╰──────────────── Grok Build · always-approve ─╯\n\
         ⠹ Running: shell - agentscan 8s … ⇣123 [✗]\n",
        );

        assert_eq!(grok.status.kind, StatusKind::Busy);
        assert_eq!(grok.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn grok_pane_output_marks_running_body_marker_busy() {
        let mut grok = pane_output_status_pane(774, Provider::Grok, "grok");

        classify::apply_pane_output_status_fallback(
            &mut grok,
            "Turn completed in 2.8s.\n\
         ⠹ Editing files 5s … ⇣42 [✗]\n",
        );

        assert_eq!(grok.status.kind, StatusKind::Busy);
        assert_eq!(grok.status.source, crate::app::StatusSource::PaneOutput);
    }

    #[test]
    fn grok_pane_output_leaves_footerless_box_after_stale_spinner_unknown() {
        // The screen capture still holds a prior turn's running spinner, but the current bottom
        // UI is the input box, but a footerless box is not positive idle evidence. The stale spinner
        // must not force busy, and the absence of an idle footer must degrade to unknown.
        let mut grok = pane_output_status_pane(775, Provider::Grok, "grok");

        classify::apply_pane_output_status_fallback(
            &mut grok,
            "⠹ Running: shell - agentscan 8s … ⇣123 [✗]\n\
         Turn completed in 4.2s.\n\
         ╭────────────────────────────────────────────────╮\n\
         │ ❯                                                │\n\
         ╰──────────────── Grok Build · always-approve ─╯\n",
        );

        assert_eq!(grok.status.kind, StatusKind::Unknown);
        assert_eq!(grok.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn grok_pane_output_does_not_infer_idle_with_output_just_below_box_border() {
        // Even a single output row below the box border means the box is a stale frame in the
        // scrollback capture, not the current prompt — distance alone must not call it idle.
        let mut grok = pane_output_status_pane(776, Provider::Grok, "grok");

        classify::apply_pane_output_status_fallback(
            &mut grok,
            "╭────────────────────────────────────────────────╮\n\
         │ ❯                                                │\n\
         ╰──────────────── Grok Build · always-approve ─╯\n\
         Reading files\n",
        );

        assert_eq!(grok.status.kind, StatusKind::Unknown);
        assert_eq!(grok.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn grok_pane_output_does_not_infer_idle_without_current_prompt_box() {
        // A completed-turn line scrolled near the bottom with no current input box must not
        // be read as idle.
        let mut grok = pane_output_status_pane(772, Provider::Grok, "grok");

        classify::apply_pane_output_status_fallback(
            &mut grok,
            "Turn completed in 2.8s.\n\
         Reviewing the diff before the next step.\n",
        );

        assert_eq!(grok.status.kind, StatusKind::Unknown);
        assert_eq!(grok.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn grok_pane_output_does_not_infer_idle_from_scrolled_away_prompt_box() {
        // The input box exists in scrollback but a later turn pushed it far from the current
        // bottom, so it is no longer the active prompt.
        let mut grok = pane_output_status_pane(773, Provider::Grok, "grok");

        classify::apply_pane_output_status_fallback(
            &mut grok,
            "╭────────────────────────────────────────────────╮\n\
         │ ❯                                                │\n\
         ╰──────────────── Grok Build · always-approve ─╯\n\
         Reading files\n\
         Planning edits\n\
         Updating code\n\
         Running tests\n\
         Collecting output\n\
         Drafting summary\n\
         Current line\n",
        );

        assert_eq!(grok.status.kind, StatusKind::Unknown);
        assert_eq!(grok.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn grok_pane_output_does_not_treat_release_channel_prose_below_box_as_chrome() {
        // The version footer is shape-anchored (a couple of version/channel tokens). A prose
        // line that merely mentions a channel word like "Beta" sits below the box as real output,
        // so the box is a stale frame and the pane must not be inferred idle.
        let mut grok = pane_output_status_pane(777, Provider::Grok, "grok");

        classify::apply_pane_output_status_fallback(
            &mut grok,
            "╭────────────────────────────────────────────────╮\n\
         │ ❯                                                │\n\
         ╰──────────────── Grok Build · always-approve ─╯\n\
         Beta access for the new planner is rolling out now\n",
        );

        assert_eq!(grok.status.kind, StatusKind::Unknown);
        assert_eq!(grok.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn grok_pane_output_does_not_treat_ctrl_prose_below_box_as_keybind_footer() {
        // The keybind footer is matched by its `Ctrl+<key>:<action>` shape, not bare `Ctrl+`. Model
        // output that mentions `Ctrl+C` in prose sits below the box as real output, so the box is a
        // stale frame and the pane must not be inferred idle.
        let mut grok = pane_output_status_pane(780, Provider::Grok, "grok");

        classify::apply_pane_output_status_fallback(
            &mut grok,
            "╭────────────────────────────────────────────────╮\n\
         │ ❯                                                │\n\
         ╰──────────────── Grok Build · always-approve ─╯\n\
         You can press Ctrl+C to stop the dev server\n",
        );

        assert_eq!(grok.status.kind, StatusKind::Unknown);
        assert_eq!(grok.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn grok_pane_output_does_not_treat_shift_tab_prose_below_box_as_keybind_footer() {
        // The keybind footer is matched by its `Key:action` shape, not a bare `Shift+Tab` substring.
        // Prose mentioning Shift+Tab below a stale box is real output, so the pane stays unknown.
        let mut grok = pane_output_status_pane(782, Provider::Grok, "grok");

        classify::apply_pane_output_status_fallback(
            &mut grok,
            "╭────────────────────────────────────────────────╮\n\
         │ ❯                                                │\n\
         ╰──────────────── Grok Build · always-approve ─╯\n\
         Press Shift+Tab to cycle between the open editor tabs\n",
        );

        assert_eq!(grok.status.kind, StatusKind::Unknown);
        assert_eq!(grok.status.source, crate::app::StatusSource::NotChecked);
    }

    #[test]
    fn grok_pane_output_does_not_treat_version_led_prose_below_box_as_chrome() {
        // The version footer's trailing tokens must be release-channel labels, so version-led prose
        // like `0.2.3 docs` is real output below a stale box and the pane must not be inferred idle.
        let mut grok = pane_output_status_pane(781, Provider::Grok, "grok");

        classify::apply_pane_output_status_fallback(
            &mut grok,
            "╭────────────────────────────────────────────────╮\n\
         │ ❯                                                │\n\
         ╰──────────────── Grok Build · always-approve ─╯\n\
         0.2.3 docs\n",
        );

        assert_eq!(grok.status.kind, StatusKind::Unknown);
        assert_eq!(grok.status.source, crate::app::StatusSource::NotChecked);
    }
}
