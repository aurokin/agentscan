use super::*;

pub(super) fn status(output: &str) -> Option<StatusKind> {
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
