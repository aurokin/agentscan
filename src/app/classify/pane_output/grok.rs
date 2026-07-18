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
