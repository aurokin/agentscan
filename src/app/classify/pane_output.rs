use super::*;

pub(crate) fn pane_output_status_fallback_candidate(pane: &PaneRecord) -> bool {
    matches!(
        pane.provider,
        Some(Provider::Codex)
            | Some(Provider::Claude)
            | Some(Provider::Copilot)
            | Some(Provider::CursorCli)
            | Some(Provider::Gemini)
            | Some(Provider::Grok)
            | Some(Provider::Hermes)
            | Some(Provider::Opencode)
            | Some(Provider::Pi)
            | Some(Provider::Antigravity)
            | Some(Provider::Droid)
    ) && pane.status.kind == StatusKind::Unknown
        && pane.status.source == StatusSource::NotChecked
}

pub(crate) fn apply_pane_output_status_fallback(pane: &mut PaneRecord, output: &str) {
    if !pane_output_status_fallback_candidate(pane) {
        return;
    }

    // Agent TUIs render their current prompt/footer at the bottom of what they have
    // drawn, but a pane is often taller than that — a freshly started or top-rendered
    // agent leaves dozens of blank trailing rows below its UI. Anchor every "near the
    // current footer" matcher to the last rendered line by dropping trailing blank rows
    // once here, so each provider matcher does not have to fight pane padding.
    let output = trim_trailing_blank_lines(output);

    let status = match pane.provider {
        Some(Provider::Codex) => codex_pane_output_status(output),
        Some(Provider::Claude) => claude_pane_output_status(output),
        Some(Provider::Copilot) => copilot_pane_output_status(output),
        Some(Provider::CursorCli) => cursor_cli_pane_output_status(output),
        Some(Provider::Gemini) => gemini_pane_output_status(output),
        Some(Provider::Grok) => grok_pane_output_status(output),
        Some(Provider::Hermes) => hermes_pane_output_status(output),
        Some(Provider::Opencode) => opencode_pane_output_status(output),
        Some(Provider::Pi) => pi_pane_output_status(output),
        Some(Provider::Antigravity) => antigravity_pane_output_status(output),
        Some(Provider::Droid) => droid_pane_output_status(output),
        _ => None,
    };

    if let Some(kind) = status {
        pane.status = PaneStatus::pane_output(kind);
    }
}

/// Returns `output` with trailing blank (whitespace-only) lines removed.
///
/// Only trailing *blank* rows are dropped; blank rows between content and trailing rendered
/// content are preserved, so the distance from a prompt to real content above it still
/// anchors the "stale frame" guards.
fn trim_trailing_blank_lines(output: &str) -> &str {
    let mut end = 0;
    let mut offset = 0;
    for line in output.split_inclusive('\n') {
        offset += line.len();
        if !line.trim().is_empty() {
            end = offset;
        }
    }
    &output[..end]
}

fn claude_pane_output_status(output: &str) -> Option<StatusKind> {
    let lines: Vec<&str> = output.lines().collect();
    let prompt_index = lines.iter().rposition(|line| claude_prompt_line(line));
    let busy_index = lines
        .iter()
        .rposition(|line| claude_current_busy_marker_line(line));

    if let Some(index) = busy_index
        && prompt_index.is_some_and(|prompt_index| {
            claude_busy_marker_is_near_current_prompt(&lines, index, prompt_index)
        })
    {
        return Some(StatusKind::Busy);
    }

    prompt_index
        .is_some_and(|index| claude_prompt_is_near_current_footer(&lines, index))
        .then_some(StatusKind::Idle)
}

fn claude_prompt_line(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with('❯')
}

fn claude_current_busy_marker_line(line: &str) -> bool {
    let line = line.trim();
    claude_interrupt_hint_line(line) || claude_waiting_permission_line(line)
}

fn claude_interrupt_hint_line(line: &str) -> bool {
    line.contains("esc") && line.contains("interrupt")
}

fn claude_waiting_permission_line(line: &str) -> bool {
    line.contains("Waiting for permission")
}

fn claude_busy_marker_is_near_current_prompt(
    lines: &[&str],
    busy_index: usize,
    prompt_index: usize,
) -> bool {
    let distance = prompt_index.abs_diff(busy_index);
    distance <= 6
        && claude_lines_between_are_status_gap(lines, busy_index, prompt_index)
        && claude_prompt_is_near_current_footer(lines, prompt_index)
}

fn claude_lines_between_are_status_gap(lines: &[&str], first: usize, second: usize) -> bool {
    let start = first.min(second) + 1;
    let end = first.max(second);
    lines[start..end]
        .iter()
        .all(|line| claude_status_gap_line(line))
}

fn claude_status_gap_line(line: &str) -> bool {
    let line = line.trim();
    line.is_empty() || claude_prompt_border_line(line)
}

fn claude_prompt_is_near_current_footer(lines: &[&str], prompt_index: usize) -> bool {
    let tail_len = lines.len().saturating_sub(prompt_index);
    tail_len <= 8
        && lines[prompt_index..]
            .iter()
            .any(|line| claude_current_footer_line(line))
}

fn claude_current_footer_line(line: &str) -> bool {
    let line = line.trim();
    line.contains("? for shortcuts")
        || line.contains("shift+tab") && line.contains("cycle")
        || line.contains("auto on")
        || line.contains("plan on")
        || line.contains("accept edits on")
        || line.contains("bypass permissions on")
        || line.contains("ultraplan on")
        || claude_interrupt_hint_line(line)
}

fn claude_prompt_border_line(line: &str) -> bool {
    let line = line.trim();
    line.chars().count() >= 8
        && line
            .chars()
            .all(|ch| matches!(ch, '─' | '╭' | '╮' | '╰' | '╯'))
}

fn codex_pane_output_status(output: &str) -> Option<StatusKind> {
    let lines: Vec<&str> = output.lines().collect();
    let idle_index = lines.iter().rposition(|line| codex_idle_prompt_line(line));
    let busy_index = lines
        .iter()
        .rposition(|line| codex_current_busy_marker_line(line));

    if let Some(index) = busy_index
        && idle_index.is_none_or(|idle_index| {
            idle_index < index
                || codex_busy_marker_is_near_current_prompt(&lines, index, idle_index)
        })
    {
        return Some(StatusKind::Busy);
    }

    idle_index
        .is_some_and(|index| codex_prompt_is_near_current_footer(&lines, index))
        .then_some(StatusKind::Idle)
}

fn codex_idle_prompt_line(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with('›') && line.contains("Ask Codex to do anything")
}

fn codex_current_busy_marker_line(line: &str) -> bool {
    let line = line.trim();
    codex_interrupt_status_line(line) || codex_approval_prompt_line(line)
}

fn codex_interrupt_status_line(line: &str) -> bool {
    line.contains("esc to interrupt)") && line.contains('(')
}

fn codex_approval_prompt_line(line: &str) -> bool {
    line.contains("Press enter to confirm or esc to cancel")
        || line.contains("Yes, proceed")
        || line.contains("Reviewing ") && line.contains("approval request")
}

fn codex_busy_marker_is_near_current_prompt(
    lines: &[&str],
    busy_index: usize,
    idle_index: usize,
) -> bool {
    idle_index.saturating_sub(busy_index) <= 4
        && lines[busy_index + 1..idle_index]
            .iter()
            .all(|line| codex_status_gap_line(line))
        && codex_prompt_is_near_current_footer(lines, idle_index)
}

fn codex_status_gap_line(line: &str) -> bool {
    let line = line.trim();
    line.is_empty() || line.starts_with('└')
}

fn codex_prompt_is_near_current_footer(lines: &[&str], prompt_index: usize) -> bool {
    let tail_len = lines.len().saturating_sub(prompt_index);
    tail_len <= 6
        && lines[prompt_index..]
            .iter()
            .any(|line| codex_footer_line(line))
}

fn codex_footer_line(line: &str) -> bool {
    let line = line.trim();
    line.contains("context left")
        || (line.contains("Context ") && line.contains(" used"))
        || line.contains("Fast on")
        || line.contains("tab to queue message")
        || codex_model_path_footer_line(line)
}

fn codex_model_path_footer_line(line: &str) -> bool {
    line.contains(" · ")
        && codex_model_footer_token(line)
        && (codex_footer_has_path_context(line) || codex_footer_has_mode_context(line))
}

fn codex_model_footer_token(line: &str) -> bool {
    line.split_whitespace()
        .any(|token| token.starts_with("gpt-") || token.starts_with("o"))
}

fn codex_footer_has_path_context(line: &str) -> bool {
    line.split(" · ").any(|part| {
        let part = part.trim();
        part.starts_with('/')
            || part.starts_with("~/")
            || part.starts_with("./")
            || part.contains("/Users/")
            || part.contains("/tmp/")
    })
}

fn codex_footer_has_mode_context(line: &str) -> bool {
    line.contains("Plan mode")
        || line.contains("Default mode")
        || line.contains("Shell mode")
        || line.contains("Side from ")
        || line.contains("Goal ")
}

fn copilot_pane_output_indicates_busy(output: &str) -> bool {
    copilot_current_status_line(output).is_some_and(|line| line.contains("Thinking (Esc to cancel"))
        || copilot_current_trust_prompt_visible(output)
}

fn copilot_pane_output_status(output: &str) -> Option<StatusKind> {
    if copilot_pane_output_indicates_busy(output) {
        return Some(StatusKind::Busy);
    }

    copilot_current_prompt_visible(output).then_some(StatusKind::Idle)
}

fn copilot_current_status_line(output: &str) -> Option<&str> {
    let lines: Vec<&str> = output.lines().collect();
    let prompt_index = lines.iter().rposition(|line| line.trim() == "❯")?;
    let context_index = lines[..prompt_index]
        .iter()
        .rposition(|line| copilot_prompt_context_line(line))?;

    let status_line = lines[..context_index].last()?.trim();
    (!status_line.is_empty()).then_some(status_line)
}

fn copilot_prompt_context_line(line: &str) -> bool {
    let line = line.trim();
    (line.starts_with('/') || line.starts_with("~/")) && !line.starts_with("/ commands")
}

fn copilot_current_prompt_visible(output: &str) -> bool {
    let lines: Vec<&str> = output.lines().collect();
    let Some(prompt_index) = lines.iter().rposition(|line| line.trim() == "❯") else {
        return false;
    };
    let Some(context_index) = lines[..prompt_index]
        .iter()
        .rposition(|line| copilot_prompt_context_line(line))
    else {
        return false;
    };

    lines[prompt_index..]
        .iter()
        .any(|line| line.contains("/ commands") && line.contains("? help"))
        && lines[context_index..prompt_index]
            .iter()
            .any(|line| copilot_separator_line(line))
}

fn copilot_separator_line(line: &str) -> bool {
    let line = line.trim();
    line.len() >= 8 && line.chars().all(|ch| ch == '─')
}

fn copilot_current_trust_prompt_visible(output: &str) -> bool {
    let lines: Vec<&str> = output.lines().collect();
    let Some(modal_index) = lines
        .iter()
        .rposition(|line| line.contains("Confirm folder trust"))
    else {
        return false;
    };

    let modal_lines = &lines[modal_index..];
    let normal_prompt_after_modal = modal_lines.iter().any(|line| line.trim() == "❯");
    !normal_prompt_after_modal
        && modal_lines
            .iter()
            .any(|line| line.contains("Do you trust the files in this folder?"))
}

fn cursor_cli_pane_output_status(output: &str) -> Option<StatusKind> {
    let lines: Vec<&str> = output.lines().collect();
    let footer_top_index = lines
        .iter()
        .rposition(|line| cursor_cli_footer_top_border(line))?;

    let current_footer = lines[footer_top_index..]
        .iter()
        .map(|line| line.trim())
        .find(|line| line.starts_with('→'));

    if current_footer.is_some_and(|line| line.contains("ctrl+c to stop"))
        || cursor_cli_current_status_line(&lines, footer_top_index)
            .is_some_and(cursor_cli_status_line_indicates_running)
    {
        return Some(StatusKind::Busy);
    }

    current_footer
        .is_some_and(cursor_cli_footer_indicates_idle)
        .then_some(StatusKind::Idle)
}

fn cursor_cli_current_status_line<'a>(
    lines: &'a [&'a str],
    footer_top_index: usize,
) -> Option<&'a str> {
    lines[..footer_top_index]
        .iter()
        .rev()
        .map(|line| line.trim())
        .find(|line| !line.is_empty())
}

fn cursor_cli_status_line_indicates_running(line: &str) -> bool {
    let mut parts = line.split_whitespace();
    let Some(spinner) = parts.next() else {
        return false;
    };
    spinner
        .chars()
        .all(|ch| ('\u{2800}'..='\u{28ff}').contains(&ch))
        && parts.any(|part| part == "Running")
}

fn cursor_cli_footer_indicates_idle(line: &str) -> bool {
    let line = line.trim_start_matches('→').trim();
    line == "Add a follow-up" || line == "Plan, search, build anything"
}

fn cursor_cli_footer_top_border(line: &str) -> bool {
    line.trim_start().starts_with("▄▄▄▄▄▄")
}

fn droid_pane_output_status(output: &str) -> Option<StatusKind> {
    let lines: Vec<&str> = output.lines().collect();
    let footer_index = lines
        .iter()
        .rposition(|line| droid_current_footer_line(line))?;
    let prompt_window_start = footer_index.saturating_sub(8);
    let current_prompt_lines = &lines[prompt_window_start..=footer_index];

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

fn droid_current_footer_line(line: &str) -> bool {
    let line = line.trim();
    line.contains("? for help") && line.contains("IDE")
}

fn droid_current_busy_prompt_line(line: &str) -> bool {
    let line = line.trim();
    line.contains("> Enter to steer")
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
        && line.contains("Press ESC to stop")
}

fn droid_current_idle_prompt_line(line: &str) -> bool {
    let line = line.trim();
    line.starts_with("│ >") && !line.contains("Enter to steer")
}

fn gemini_pane_output_status(output: &str) -> Option<StatusKind> {
    let lines: Vec<&str> = output.lines().collect();
    let idle_index = lines
        .iter()
        .rposition(|line| gemini_idle_input_prompt_line(line));
    let busy_index = lines
        .iter()
        .rposition(|line| gemini_current_busy_marker_line(line));

    if let Some(index) = busy_index
        && idle_index.is_none_or(|idle_index| idle_index < index)
    {
        return Some(StatusKind::Busy);
    }

    idle_index
        .is_some_and(|index| gemini_prompt_is_near_current_footer(&lines, index))
        .then_some(StatusKind::Idle)
}

fn gemini_idle_input_prompt_line(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with('>') && line.contains("Type your message")
}

fn gemini_current_busy_marker_line(line: &str) -> bool {
    let line = line.trim();
    line.contains("Action Required")
        || line.contains("Apply this change?")
        || line.contains("Allow execution of")
        || (line.contains("Running Agent") && line.contains("ctrl+o to collapse"))
}

fn gemini_prompt_is_near_current_footer(lines: &[&str], prompt_index: usize) -> bool {
    let tail_len = lines.len().saturating_sub(prompt_index);
    tail_len <= 8
}

fn grok_pane_output_status(output: &str) -> Option<StatusKind> {
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

fn antigravity_pane_output_status(output: &str) -> Option<StatusKind> {
    let lines: Vec<&str> = output.lines().collect();

    // Antigravity (closed-source) flips its own footer below the bordered `>` input box between
    // an idle prompt (`? for shortcuts`) and an active turn (`esc to cancel`, shown with a
    // `… Generating…`/`… Loading…` spinner above), so the footer is the current-frame busy/idle
    // anchor. Require the `>` box just above and only blank rows after it, so a stale scrollback
    // line carrying the phrase is not mistaken for the live footer; anything else stays unknown
    // rather than risk a guessed state.
    let footer_index = lines.iter().rposition(|line| {
        antigravity_idle_footer_line(line) || antigravity_busy_footer_line(line)
    })?;
    let only_blank_after = lines[footer_index + 1..]
        .iter()
        .all(|line| line.trim().is_empty());
    if !(only_blank_after && antigravity_prompt_above_footer(&lines, footer_index)) {
        return None;
    }

    Some(if antigravity_busy_footer_line(lines[footer_index]) {
        StatusKind::Busy
    } else {
        StatusKind::Idle
    })
}

fn antigravity_idle_footer_line(line: &str) -> bool {
    line.trim_start().starts_with("? for shortcuts")
}

fn antigravity_busy_footer_line(line: &str) -> bool {
    line.trim_start().starts_with("esc to cancel")
}

fn antigravity_prompt_above_footer(lines: &[&str], footer_index: usize) -> bool {
    let start = footer_index.saturating_sub(6);
    lines[start..footer_index].iter().any(|line| {
        let line = line.trim();
        line == ">" || line.starts_with("> ")
    })
}

fn hermes_pane_output_status(output: &str) -> Option<StatusKind> {
    let lines: Vec<&str> = output.lines().collect();
    let busy_index = lines.iter().rposition(|line| hermes_busy_prompt_line(line));
    let idle_index = lines.iter().rposition(|line| hermes_idle_prompt_line(line));

    if let Some(index) = busy_index
        && hermes_status_bar_directly_above(&lines, index)
        && idle_index.is_none_or(|idle_index| idle_index < index)
        && hermes_prompt_is_current_frame(&lines, index)
    {
        return Some(StatusKind::Busy);
    }

    if let Some(index) = idle_index
        && hermes_status_bar_directly_above(&lines, index)
        && busy_index.is_none_or(|busy_index| busy_index < index)
        && hermes_prompt_is_current_frame(&lines, index)
    {
        return Some(StatusKind::Idle);
    }

    None
}

/// Whether a hermes prompt line reflects the current bottom frame rather than stale scrollback.
///
/// The live input box renders its `❯`/`⚕ ❯` prompt directly above the box's closing `────` rule
/// at the bottom of what hermes has drawn. The idle matcher accepts any `❯ <draft>` line, so a
/// submitted prompt or agent output that merely contains a `❯ …` line could otherwise sit deep in
/// scrollback with later output below it and be misread as the live prompt. Require that only box
/// rules and blank rows follow the prompt: any real content below it (output from a turn that has
/// since run) marks it stale. A multi-line draft conservatively reads as unknown rather than risk
/// resurrecting a ghost prompt.
fn hermes_prompt_is_current_frame(lines: &[&str], prompt_index: usize) -> bool {
    lines[prompt_index + 1..].iter().all(|line| {
        let line = line.trim();
        line.is_empty() || hermes_box_rule_line(line)
    })
}

fn hermes_box_rule_line(line: &str) -> bool {
    line.chars().count() >= 8 && line.chars().all(|ch| ch == '─' || ch == '━')
}

/// Whether the live hermes status bar sits directly above this prompt index.
///
/// The live input box renders as `<status bar>` → optional `────` rule → `❯`/`⚕ ❯` prompt, so
/// the status bar is at most a couple of rows above the prompt and only a box rule or blank may
/// sit between them. Requiring both proximity AND a clean intervening gap prevents an unrelated
/// `❯ <text>` line — e.g. a quoted shell prompt like `❯ npm test` in agent output, possibly with
/// prose like `Run this:` between it and an older status bar — from being classified idle.
fn hermes_status_bar_directly_above(lines: &[&str], prompt_index: usize) -> bool {
    let start = prompt_index.saturating_sub(3);
    lines[start..prompt_index]
        .iter()
        .enumerate()
        .rev()
        .find(|(_, line)| hermes_status_bar_line(line.trim()))
        .is_some_and(|(rel_index, _)| {
            let status_index = start + rel_index;
            lines[status_index + 1..prompt_index].iter().all(|line| {
                let line = line.trim();
                line.is_empty() || hermes_box_rule_line(line)
            })
        })
}

fn hermes_status_bar_line(line: &str) -> bool {
    line.starts_with("⚕ ") && line.contains('│') && (line.contains("ctx") || line.contains("K/"))
}

/// Hermes' live input prompt while idle: a bare `❯`, or `❯ <draft>` when the user has typed but
/// not yet submitted (the agent is still not running a turn). The busy prompt is `⚕ ❯ …`, which
/// starts with `⚕`, so this stays unambiguous against it.
fn hermes_idle_prompt_line(line: &str) -> bool {
    let line = line.trim();
    line == "❯" || line.starts_with("❯ ")
}

fn hermes_busy_prompt_line(line: &str) -> bool {
    let line = line.trim();
    line.starts_with("⚕ ❯")
        && line.contains("msg=interrupt")
        && line.contains("/queue")
        && line.contains("Ctrl+C cancel")
}

fn opencode_pane_output_status(output: &str) -> Option<StatusKind> {
    let lines: Vec<&str> = output.lines().collect();

    // Bottom-most current idle prompt: either the placeholder input row (older build) near
    // the current footer, or the newer build's command-bar input box. Folding both into one
    // index lets a busy marker only win when it is below the live prompt — a stale
    // approval/question row above the current prompt must not force busy.
    let placeholder_index = lines
        .iter()
        .rposition(|line| opencode_idle_prompt_line(line))
        .filter(|&index| opencode_prompt_is_near_current_footer(&lines, index));
    let command_bar_index = opencode_current_command_bar_index(&lines);
    let input_box_index = opencode_current_input_box_index(&lines);
    let idle_index = placeholder_index
        .max(command_bar_index)
        .max(input_box_index);

    let busy_index = lines
        .iter()
        .rposition(|line| opencode_current_busy_marker_line(line));

    if let Some(index) = busy_index
        && opencode_busy_marker_is_current(
            &lines,
            index,
            idle_index,
            command_bar_index,
            input_box_index,
        )
    {
        return Some(StatusKind::Busy);
    }

    idle_index.map(|_| StatusKind::Idle)
}

/// Whether a busy marker reflects the current bottom frame rather than stale scrollback.
///
/// The capture is the last 30 rows including scrollback, so an old approval/interrupt line
/// can sit above a frame that has since scrolled on. A busy marker is current when it is
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
    lines: &[&str],
    busy_index: usize,
    idle_index: Option<usize>,
    command_bar_index: Option<usize>,
    input_box_index: Option<usize>,
) -> bool {
    let in_current_footer =
        opencode_busy_marker_in_current_footer(lines, busy_index, command_bar_index)
            || opencode_busy_marker_in_current_footer(lines, busy_index, input_box_index);
    match idle_index {
        Some(idle_index) => idle_index < busy_index || in_current_footer,
        None => in_current_footer || opencode_prompt_is_near_current_footer(lines, busy_index),
    }
}

fn opencode_busy_marker_in_current_footer(
    lines: &[&str],
    busy_index: usize,
    command_bar_index: Option<usize>,
) -> bool {
    let Some(command_bar) = command_bar_index else {
        return false;
    };
    busy_index < command_bar
        && lines[busy_index + 1..command_bar]
            .iter()
            .all(|line| opencode_prompt_gap_line(line))
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
    line.contains("Ask anything... \"") || line.contains("Run a command... \"")
}

/// Index of the newer build's command bar when its input box is the current prompt.
///
/// The bordered input box sits directly above a `tab agents  ctrl+p commands` command bar.
/// The capture is the last 30 rows including scrollback, so only opencode's own trailing
/// chrome (blank rows, notices, the bottom status bar) may follow it; a command bar trailed
/// by real agent output is a stale frame, not the current prompt.
fn opencode_current_command_bar_index(lines: &[&str]) -> Option<usize> {
    let footer_index = lines
        .iter()
        .rposition(|line| opencode_command_bar_footer_line(line))?;
    let box_window_start = footer_index.saturating_sub(2);
    let has_input_box = lines[box_window_start..footer_index]
        .iter()
        .any(|line| opencode_input_box_bottom_border(line));

    // Only opencode's own chrome may follow the command bar: blank rows, a `● Tip` notice,
    // or the pinned bottom status bar as the final row. Anything else means the command bar
    // is a stale frame in the scrollback capture, not the current prompt.
    let last_index = lines.len().saturating_sub(1);
    let only_trailing_chrome =
        lines
            .iter()
            .enumerate()
            .skip(footer_index + 1)
            .all(|(index, line)| {
                let line = line.trim();
                line.is_empty()
                    || line.starts_with("● Tip")
                    || (index == last_index && opencode_bottom_status_bar_line(line))
            });
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
fn opencode_current_input_box_index(lines: &[&str]) -> Option<usize> {
    let border_index = lines
        .iter()
        .rposition(|line| opencode_input_box_bottom_border(line))?;
    let last_index = lines.len().saturating_sub(1);
    let only_trailing_chrome =
        lines
            .iter()
            .enumerate()
            .skip(border_index + 1)
            .all(|(index, line)| {
                let line = line.trim();
                line.is_empty()
                    || line.starts_with("● Tip")
                    || opencode_command_bar_footer_line(line)
                    || (index == last_index && opencode_bottom_status_bar_line(line))
            });
    only_trailing_chrome.then_some(border_index)
}

fn opencode_command_bar_footer_line(line: &str) -> bool {
    let line = line.trim();
    line.contains("tab agents") && line.contains("ctrl+p commands")
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
    if line.contains("• OpenCode") {
        return line.split_whitespace().any(dotted_version_token);
    }
    (line.starts_with("~/") || line.starts_with('/'))
        && line.contains(':')
        && line
            .split_whitespace()
            .next_back()
            .is_some_and(dotted_version_token)
}

fn dotted_version_token(token: &str) -> bool {
    let segments: Vec<&str> = token.split('.').collect();
    segments.len() >= 3
        && segments
            .iter()
            .all(|segment| !segment.is_empty() && segment.chars().all(|ch| ch.is_ascii_digit()))
}

fn opencode_current_busy_marker_line(line: &str) -> bool {
    let line = line.trim();
    opencode_interrupt_hint_line(line)
        || opencode_permission_prompt_line(line)
        || opencode_question_prompt_line(line)
}

fn opencode_interrupt_hint_line(line: &str) -> bool {
    line.contains("esc") && line.contains("interrupt")
}

fn opencode_permission_prompt_line(line: &str) -> bool {
    line.contains("Permission required")
        || line.contains("Reject permission")
        || line.contains("Allow once")
        || line.contains("Allow always")
        || (line.contains('△') && line.contains("Permission"))
}

fn opencode_question_prompt_line(line: &str) -> bool {
    line.contains("Reject question")
        || line.contains("Waiting for question event")
        || line.contains("# Questions")
}

fn opencode_prompt_is_near_current_footer(lines: &[&str], prompt_index: usize) -> bool {
    let tail_len = lines.len().saturating_sub(prompt_index);
    tail_len <= 8
}

fn pi_pane_output_status(output: &str) -> Option<StatusKind> {
    let lines: Vec<&str> = output.lines().collect();
    let idle_index = lines.iter().rposition(|line| pi_editor_border_line(line));
    let busy_index = lines
        .iter()
        .rposition(|line| pi_current_busy_marker_line(line));

    if let Some(index) = busy_index
        && idle_index.is_none_or(|idle_index| {
            idle_index < index || pi_busy_marker_is_near_current_editor(&lines, index, idle_index)
        })
    {
        return Some(StatusKind::Busy);
    }

    idle_index
        .is_some_and(|index| pi_editor_frame_is_near_current_footer(&lines, index))
        .then_some(StatusKind::Idle)
}

fn pi_current_busy_marker_line(line: &str) -> bool {
    let line = line.trim();
    pi_working_loader_line(line)
        || pi_retry_loader_line(line)
        || pi_compaction_loader_line(line)
        || pi_running_bash_line(line)
}

fn pi_busy_marker_is_near_current_editor(
    lines: &[&str],
    busy_index: usize,
    idle_index: usize,
) -> bool {
    idle_index.saturating_sub(busy_index) <= 4
        && lines[busy_index + 1..idle_index]
            .iter()
            .all(|line| pi_editor_gap_line(line))
        && pi_editor_frame_is_near_current_footer(lines, idle_index)
}

fn pi_editor_gap_line(line: &str) -> bool {
    let line = line.trim();
    line.is_empty() || pi_editor_border_line(line)
}

fn pi_working_loader_line(line: &str) -> bool {
    line.contains("Working...") || line.contains(" to interrupt)")
}

fn pi_retry_loader_line(line: &str) -> bool {
    line.contains("Retrying (") && line.contains(" to cancel)")
}

fn pi_compaction_loader_line(line: &str) -> bool {
    (line.contains("Compacting context...") || line.contains("Auto-compacting..."))
        && line.contains(" to cancel)")
}

fn pi_running_bash_line(line: &str) -> bool {
    line.contains("Running...") && line.contains(" to cancel)")
}

fn pi_editor_border_line(line: &str) -> bool {
    let line = line.trim();
    line.chars().count() >= 8 && line.chars().all(|ch| ch == '─')
}

fn pi_editor_frame_is_near_current_footer(lines: &[&str], border_index: usize) -> bool {
    let tail_len = lines.len().saturating_sub(border_index);
    tail_len <= 6
        && lines[border_index..]
            .iter()
            .any(|line| pi_footer_context_line(line))
}

fn pi_footer_context_line(line: &str) -> bool {
    let line = line.trim();
    pi_footer_context_token(line)
        && !pi_current_busy_marker_line(line)
        && !line.contains("Error:")
        && !line.contains("Warning:")
}

fn pi_footer_context_token(line: &str) -> bool {
    line.split_whitespace().any(|token| {
        token.contains("%/")
            || (token.starts_with("?/") && token.chars().skip(2).any(|ch| ch.is_ascii_digit()))
    })
}
