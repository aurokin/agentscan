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
        // OSC escape and tmux keeps painted after pi exits — and which any process launched next
        // in the same shell inherits, including a *different* agent (e.g. hermes' `python`). Unlike
        // the product-name titles other providers emit, that glyph+cwd signature cannot tell a live
        // pi session apart from stale residue, so it is only trustworthy when the pane foreground
        // is itself a live pi runtime (`pi`/`node`/`bun`) or a spinner glyph shows the title being
        // actively repainted. Any other foreground — a bare shell (the agent exited) or another
        // agent's runtime that merely inherited the residue — must defer to process evidence rather
        // than resurrect a ghost pane. This mirrors the corroboration grok and ascii `pi -` titles
        // already need.
        let stale_pi_title = provider == Provider::Pi
            && !command_is_pi_runtime(command)
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

/// Commands a live pi session presents as its pty foreground: the `pi` binary itself or the
/// `node`/`bun` runtime that hosts it. Agent runtimes hold the pty foreground while alive, so any
/// other foreground (a bare shell after pi exited, or a different agent's runtime that merely
/// inherited pi's residual `π - ` OSC title) is not a live pi session — see the stale-title
/// reasoning in `classify_provider_from_analysis`. Login shells may carry a leading `-`.
fn command_is_pi_runtime(command: &str) -> bool {
    let command = command.trim();
    let command = command.strip_prefix('-').unwrap_or(command);
    matches!(command, "pi" | "node" | "bun")
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
