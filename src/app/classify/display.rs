use super::status_label::is_generic_display_status_label;
use super::*;

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn display_metadata(
    provider: Option<Provider>,
    provider_match_kind: Option<ClassificationMatchKind>,
    published_label: Option<&str>,
    title: &str,
    current_command: &str,
    window_name: &str,
) -> DisplayMetadata {
    let title_analysis = analyze_title(title);
    display_metadata_from_analysis(
        &title_analysis,
        provider,
        provider_match_kind,
        published_label,
        current_command,
        window_name,
    )
}

pub(super) fn display_metadata_from_analysis(
    title_analysis: &TitleAnalysis<'_>,
    provider: Option<Provider>,
    provider_match_kind: Option<ClassificationMatchKind>,
    published_label: Option<&str>,
    current_command: &str,
    window_name: &str,
) -> DisplayMetadata {
    if let Some(label) = published_label
        .map(str::trim)
        .filter(|label| !label.is_empty())
    {
        return DisplayMetadata {
            label: label.to_string(),
            activity_label: infer_activity_label(provider, label),
        };
    }

    if let Some(label) = display_label_from_title(
        provider,
        provider_match_kind,
        title_analysis,
        current_command,
    ) {
        let activity_label = if matches!(provider, Some(Provider::Codex)) {
            codex_activity_from_status_title(&title_analysis.codex_status_title)
                .or_else(|| infer_activity_label(provider, &label))
        } else if matches!(provider, Some(Provider::Gemini)) {
            title_analysis
                .gemini_title
                .as_ref()
                .and_then(|title| title.activity_label.clone())
                .or_else(|| {
                    title_analysis
                        .gemini_title
                        .is_none()
                        .then(|| infer_activity_label(provider, &label))
                        .flatten()
                })
        } else if title_activity_should_stay_empty(provider, title_analysis) {
            None
        } else {
            infer_activity_label(provider, &label)
        };
        return DisplayMetadata {
            activity_label,
            label,
        };
    }
    if !window_name.trim().is_empty() {
        return DisplayMetadata {
            label: window_name.trim().to_string(),
            activity_label: None,
        };
    }

    DisplayMetadata {
        label: current_command.trim().to_string(),
        activity_label: None,
    }
}

fn title_activity_should_stay_empty(
    provider: Option<Provider>,
    title_analysis: &TitleAnalysis<'_>,
) -> bool {
    (matches!(provider, Some(Provider::Pi)) && title_analysis.stripped.starts_with("π - "))
        || (matches!(provider, Some(Provider::Opencode))
            && (title_analysis.opencode_label.is_some() || title_analysis.stripped == "OpenCode"))
}

fn display_label_from_title(
    provider: Option<Provider>,
    provider_match_kind: Option<ClassificationMatchKind>,
    title_analysis: &TitleAnalysis<'_>,
    current_command: &str,
) -> Option<String> {
    if title_analysis.conflicts_with_resolved_provider(provider, provider_match_kind) {
        return None;
    }

    let normalized = title_analysis.normalized_label(provider)?;
    if matches!(provider, Some(Provider::CursorCli))
        && cursor_cli_should_fall_back_to_window_name(
            provider_match_kind,
            title_analysis.cursor_title_shaped,
            &normalized,
            current_command,
        )
    {
        return None;
    }

    Some(normalized)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn normalize_title_for_display(provider: Option<Provider>, title: &str) -> String {
    analyze_title(title)
        .normalized_label(provider)
        .unwrap_or_default()
}
fn cursor_cli_should_fall_back_to_window_name(
    provider_match_kind: Option<ClassificationMatchKind>,
    cursor_title_shaped: bool,
    normalized_title: &str,
    current_command: &str,
) -> bool {
    if provider_match_kind == Some(ClassificationMatchKind::PaneTitle) && cursor_title_shaped {
        return false;
    }

    if is_generic_provider_label(Some(Provider::CursorCli), normalized_title)
        || is_generic_status_label(normalized_title)
    {
        return true;
    }

    if provider_match_kind != Some(ClassificationMatchKind::PaneMetadata) && !cursor_title_shaped {
        return true;
    }

    normalized_title.eq_ignore_ascii_case(current_command.trim())
}

fn infer_activity_label(provider: Option<Provider>, label: &str) -> Option<String> {
    let label = label.trim();
    if label.is_empty() {
        return None;
    }

    if is_generic_provider_label(provider, label) {
        return None;
    }

    if matches!(provider, Some(Provider::Codex))
        && let Some(activity) = codex_activity_from_status_title(label)
    {
        return Some(activity);
    }

    if is_generic_status_label(label) {
        return None;
    }

    match provider {
        Some(Provider::Codex) => Some(label.to_string()),
        Some(Provider::Claude)
        | Some(Provider::Gemini)
        | Some(Provider::Opencode)
        | Some(Provider::Copilot)
        | Some(Provider::CursorCli)
        | Some(Provider::Pi) => Some(label.to_string()),
        _ => None,
    }
}

fn is_generic_provider_label(provider: Option<Provider>, label: &str) -> bool {
    provider.is_some_and(|provider| {
        provider_generic_display_labels(provider)
            .iter()
            .any(|generic| label.eq_ignore_ascii_case(generic))
    })
}

fn is_generic_status_label(label: &str) -> bool {
    is_generic_display_status_label(label)
}
