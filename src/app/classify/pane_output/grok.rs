use super::*;

pub(super) fn status(output: &str) -> Option<StatusKind> {
    let lines: Vec<&str> = output.lines().collect();

    // grok pins its rounded input box (`│ ❯ … │`) at the bottom in both idle and busy states.
    // The keybind footer just below it reflects grok's own state: an in-flight turn adds
    // `Ctrl+c:cancel` / `Ctrl+Enter:interject` hints, an idle prompt shows only
    // mode/shortcuts, and a fresh prompt shows the version line (e.g. `0.2.3 [stable] Beta`).
    if let Some(border) = grok_current_input_box_border(&lines) {
        // Busy when grok's footer shows interrupt keybinds OR a live run spinner sits
        // directly above the pinned box. The footer wording is the primary signal; the
        // spinner backstop keeps a live turn from reading idle if grok relabels its hints,
        // while a stale spinner (completed-turn output between it and the box) is not
        // directly above the box and so won't trip it.
        let busy = grok_active_turn_footer_after(&lines, border)
            || grok_running_spinner_above_box(&lines, border);
        return Some(if busy {
            StatusKind::Busy
        } else {
            StatusKind::Idle
        });
    }

    // No current input box (a transient mid-stream frame): a running spinner as the current
    // bottom line still means an in-flight turn.
    grok_current_running_spinner(&lines).then_some(StatusKind::Busy)
}

/// Index of the input box bottom border when the box is the current bottom frame.
///
/// The capture is already trailing-trimmed, so the box is current when its `╰ … ─╯` bottom
/// border has the `│ ❯ … │` input row directly above it and only grok's own footer chrome
/// (blank rows, the keybind footer, or the version line) below it. A box trailed by real turn
/// output is a stale frame in the scrollback capture, not the live prompt.
fn grok_current_input_box_border(lines: &[&str]) -> Option<usize> {
    let border = lines
        .iter()
        .rposition(|line| grok_prompt_box_bottom_border(line))?;
    let input_above = border > 0 && grok_prompt_box_input_line(lines[border - 1]);
    let only_footer_below = lines[border + 1..].iter().all(|line| {
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
/// (`Shift+Tab:mode │ Ctrl+.:shortcuts`) or, on a fresh prompt, the version line
/// (`0.2.3 [stable] Beta`). Matched by shape so a stale turn line below the box is not
/// mistaken for chrome.
fn grok_footer_line(line: &str) -> bool {
    grok_keybind_footer_line(line) || grok_version_footer_line(line)
}

fn grok_keybind_footer_line(line: &str) -> bool {
    line.split(|ch: char| ch.is_whitespace() || ch == '│')
        .any(grok_keybind_token)
}

/// A grok keybind hint token such as `Shift+Tab:mode`, `Ctrl+.:shortcuts`, or `Ctrl+c:cancel` —
/// a key spec bound to an action via `:`. Requiring the `Key+…:action` shape excludes prose that
/// merely mentions `Shift+Tab` or `Ctrl+C`, so a stale scrollback line is not mistaken for the
/// live footer chrome.
fn grok_keybind_token(token: &str) -> bool {
    let Some((key, action)) = token.split_once(':') else {
        return false;
    };
    !action.is_empty() && (key == "Shift+Tab" || key.starts_with("Ctrl+"))
}

/// An active turn's footer adds interrupt keybinds (`Ctrl+c:cancel`, `Ctrl+Enter:interject`)
/// that the idle footer omits, so grok's own footer distinguishes a running turn from an idle
/// prompt without having to anchor the spinner against stale scrollback.
fn grok_active_turn_footer_after(lines: &[&str], border: usize) -> bool {
    lines[border + 1..].iter().any(|line| {
        let line = line.trim();
        grok_keybind_footer_line(line) && (line.contains("cancel") || line.contains("interject"))
    })
}

/// True when a live run spinner sits directly above the current input box — only blank rows
/// between the spinner (`⠋ … [✗]`) and the box's top border. grok renders the active-turn
/// spinner right above the pinned box, so this marks a running turn independent of footer
/// wording (a backstop if grok relabels its interrupt hints). A *stale* spinner from a prior
/// turn has real output (e.g. `Turn completed…`) between it and the box, so it is not directly
/// above and does not match.
fn grok_running_spinner_above_box(lines: &[&str], border: usize) -> bool {
    let Some(top) = lines[..border]
        .iter()
        .rposition(|line| grok_prompt_box_top_border(line))
    else {
        return false;
    };
    lines[..top]
        .iter()
        .rev()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| grok_running_status_line(line))
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
    dotted_version_token(first)
        && rest.len() <= 2
        && rest.iter().all(|t| grok_release_channel_word(t))
}

/// A grok release-channel label such as `stable`, `Beta`, or the bracketed `[stable]` form.
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

/// True when the bottom-most rendered line is a running spinner: a braille spinner glyph with
/// grok's in-flight run marker (`[✗]`).
fn grok_current_running_spinner(lines: &[&str]) -> bool {
    lines
        .iter()
        .rev()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| grok_running_status_line(line))
}

fn grok_running_status_line(line: &str) -> bool {
    let line = line.trim_start();
    line.chars()
        .next()
        .is_some_and(|ch| ('\u{2800}'..='\u{28ff}').contains(&ch))
        && line.contains("[✗]")
}
