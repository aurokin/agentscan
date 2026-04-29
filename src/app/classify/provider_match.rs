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
        return Some(ProviderMatch {
            provider,
            matched_by: ClassificationMatchKind::PaneMetadata,
            confidence: ClassificationConfidence::High,
            reasons: vec![format!(
                "agent.provider={}",
                published_provider.unwrap_or_default().trim()
            )],
        });
    }

    if let Some((provider, exact)) = provider_from_command(command) {
        return Some(ProviderMatch {
            provider,
            matched_by: ClassificationMatchKind::PaneCurrentCommand,
            confidence: if exact {
                ClassificationConfidence::High
            } else {
                ClassificationConfidence::Medium
            },
            reasons: vec![format!("pane_current_command={command}")],
        });
    }

    if let Some(provider) = title_analysis.classifyable_provider() {
        return Some(ProviderMatch {
            provider,
            matched_by: ClassificationMatchKind::PaneTitle,
            confidence: ClassificationConfidence::High,
            reasons: vec![format!("pane_title={}", title_analysis.raw)],
        });
    }

    if command.eq_ignore_ascii_case("pi") && title_analysis.pi_label.is_some() {
        return Some(ProviderMatch {
            provider: Provider::Pi,
            matched_by: ClassificationMatchKind::PaneCurrentCommand,
            confidence: ClassificationConfidence::Medium,
            reasons: vec![
                format!("pane_current_command={command}"),
                format!("pane_title={}", title_analysis.raw),
            ],
        });
    }

    None
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
