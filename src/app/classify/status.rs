use super::*;

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn infer_title_status(
    provider: Option<Provider>,
    provider_match_kind: Option<ClassificationMatchKind>,
    title: &str,
) -> PaneStatus {
    let title_analysis = analyze_title(title);
    infer_title_status_from_analysis(provider, provider_match_kind, &title_analysis)
}

pub(super) fn infer_title_status_from_analysis(
    provider: Option<Provider>,
    provider_match_kind: Option<ClassificationMatchKind>,
    title_analysis: &TitleAnalysis<'_>,
) -> PaneStatus {
    if title_analysis.conflicts_with_resolved_provider(provider, provider_match_kind) {
        return PaneStatus::not_checked();
    }

    // Provider identity is already resolved here, so a single spec lookup replaces the former
    // per-provider ladder. `provider_title_status` returns the title-derived status for the
    // resolved provider (or `None`, which stays "not checked" so pane-output fallback can run).
    provider
        .and_then(|provider| provider_title_status(provider, title_analysis))
        .map(PaneStatus::title)
        .unwrap_or_else(PaneStatus::not_checked)
}

pub(crate) fn infer_status(title_status: PaneStatus, published_state: Option<&str>) -> PaneStatus {
    match published_state.map(|value| value.trim().to_ascii_lowercase()) {
        Some(state) if state == "busy" => PaneStatus::metadata(StatusKind::Busy),
        Some(state) if state == "idle" => PaneStatus::metadata(StatusKind::Idle),
        Some(state) if state == "waiting" => PaneStatus::metadata(StatusKind::Waiting),
        // A trusted literal `unknown` is explicit but intentionally yields to
        // heuristics: the publisher has no answer, while the title or current
        // pane output may still provide a useful status.
        Some(state) if state == "unknown" => title_status,
        _ => title_status,
    }
}
