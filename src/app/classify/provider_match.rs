use super::*;

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn classify_provider(
    published_provider: Option<&str>,
    command: &str,
    title: &str,
) -> Option<ProviderMatch> {
    let title_analysis = analyze_title(title);
    classify_provider_from_analysis(published_provider, command, &title_analysis)
}

pub(super) fn classify_provider_from_analysis(
    published_provider: Option<&str>,
    command: &str,
    title_analysis: &TitleAnalysis<'_>,
) -> Option<ProviderMatch> {
    let command = current_command_for_analysis(command);

    if let Some(provider) = provider_from_metadata(published_provider) {
        return Some(ProviderMatch::single_reason(
            provider,
            ClassificationMatchKind::PaneMetadata,
            ClassificationConfidence::High,
            format!(
                "agent.provider={}",
                published_provider.unwrap_or_default().trim()
            ),
        ));
    }

    if let Some((provider, exact)) = provider_from_command(command) {
        return Some(ProviderMatch::single_reason(
            provider,
            ClassificationMatchKind::PaneCurrentCommand,
            if exact {
                ClassificationConfidence::High
            } else {
                ClassificationConfidence::Medium
            },
            format!("pane_current_command={command}"),
        ));
    }

    if let Some(provider) = title_analysis.classifyable_provider() {
        // An idle pi title is just the `π` glyph plus the cwd basename, which pi paints via an
        // OSC escape and tmux keeps painted after pi exits and the pane returns to a bare shell
        // prompt. Unlike the product-name titles other providers emit, that glyph+cwd signature
        // cannot tell a live idle session apart from stale residue, so a plain interactive shell
        // foreground (positive evidence the agent is gone) must defer to process evidence rather
        // than resurrect a ghost pane. A running pi reports `node`/`pi`/`bun` (not a bare shell),
        // and a spinner glyph is live evidence the title is being actively repainted — both still
        // classify here. This mirrors the corroboration grok and ascii `pi -` titles already need.
        let stale_pi_title = provider == Provider::Pi
            && command_is_interactive_shell(command)
            && !title_analysis.has_spinner_glyph;
        if !stale_pi_title {
            return Some(ProviderMatch::single_reason(
                provider,
                ClassificationMatchKind::PaneTitle,
                ClassificationConfidence::High,
                format!("pane_title={}", title_analysis.raw),
            ));
        }
    }

    if command.eq_ignore_ascii_case("pi") && title_analysis.pi_label.is_some() {
        return Some(ProviderMatch::new(
            Provider::Pi,
            ClassificationMatchKind::PaneCurrentCommand,
            ClassificationConfidence::Medium,
            vec![
                format!("pane_current_command={command}"),
                format!("pane_title={}", title_analysis.raw),
            ],
        ));
    }

    None
}

/// A plain interactive shell (`zsh`, `bash`, …) as the pane's foreground process is positive
/// evidence the agent exited and returned to the prompt: agent runtimes hold the pty foreground
/// while alive. Login shells may appear with a leading `-` (e.g. `-zsh`).
fn command_is_interactive_shell(command: &str) -> bool {
    let command = command.trim();
    let command = command.strip_prefix('-').unwrap_or(command);
    matches!(
        command,
        "sh" | "bash" | "zsh" | "fish" | "dash" | "ksh" | "nu" | "xonsh" | "pwsh" | "csh" | "tcsh"
    )
}

pub(super) fn current_command_for_analysis(command: &str) -> &str {
    let command = command.trim();
    if is_version_like_command(command) {
        ""
    } else {
        command
    }
}

pub(super) fn is_version_like_command(command: &str) -> bool {
    let command = command.trim();
    let mut parts = command.split('.');
    let Some(first) = parts.next() else {
        return false;
    };
    let Some(second) = parts.next() else {
        return false;
    };
    let Some(third) = parts.next() else {
        return false;
    };

    !first.is_empty()
        && !second.is_empty()
        && !third.is_empty()
        && parts.all(|part| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit()))
        && first.chars().all(|ch| ch.is_ascii_digit())
        && second.chars().all(|ch| ch.is_ascii_digit())
        && third.chars().all(|ch| ch.is_ascii_digit())
}
