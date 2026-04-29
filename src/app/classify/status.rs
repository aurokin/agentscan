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
        return PaneStatus {
            kind: StatusKind::Unknown,
            source: StatusSource::NotChecked,
        };
    }

    if matches!(provider, Some(Provider::Claude)) {
        if title_analysis.has_spinner_glyph {
            return PaneStatus {
                kind: StatusKind::Busy,
                source: StatusSource::TmuxTitle,
            };
        }
        if title_analysis.has_idle_glyph {
            return PaneStatus {
                kind: StatusKind::Idle,
                source: StatusSource::TmuxTitle,
            };
        }
        if let Some(rest) = title_analysis.claude_label {
            if rest == "Working" || rest.starts_with("Working ") {
                return PaneStatus {
                    kind: StatusKind::Busy,
                    source: StatusSource::TmuxTitle,
                };
            }
            if rest == "Ready" || rest.starts_with("Ready ") {
                return PaneStatus {
                    kind: StatusKind::Idle,
                    source: StatusSource::TmuxTitle,
                };
            }
        }
    }

    if matches!(provider, Some(Provider::Codex)) {
        if title_analysis.has_spinner_glyph {
            return PaneStatus {
                kind: StatusKind::Busy,
                source: StatusSource::TmuxTitle,
            };
        }
        if let Some(status) = codex_run_state_from_title(&title_analysis.codex_status_title) {
            return PaneStatus {
                kind: status,
                source: StatusSource::TmuxTitle,
            };
        }
    }

    if matches!(provider, Some(Provider::Gemini))
        && let Some(status) = title_analysis
            .gemini_title
            .as_ref()
            .and_then(|title| title.status)
    {
        return PaneStatus {
            kind: status,
            source: StatusSource::TmuxTitle,
        };
    }

    if matches!(provider, Some(Provider::Gemini)) {
        match title_analysis.stripped {
            "Ready" => {
                return PaneStatus {
                    kind: StatusKind::Idle,
                    source: StatusSource::TmuxTitle,
                };
            }
            "Working" | "Working…" | "Action Required" => {
                return PaneStatus {
                    kind: StatusKind::Busy,
                    source: StatusSource::TmuxTitle,
                };
            }
            _ => {}
        }
    }

    if matches!(provider, Some(Provider::Copilot))
        && let Some(rest) = title_analysis.copilot_label
    {
        if rest == "Working" || rest.starts_with("Working ") {
            return PaneStatus {
                kind: StatusKind::Busy,
                source: StatusSource::TmuxTitle,
            };
        }
        if rest == "Ready" || rest.starts_with("Ready ") {
            return PaneStatus {
                kind: StatusKind::Idle,
                source: StatusSource::TmuxTitle,
            };
        }
    }

    if matches!(provider, Some(Provider::CursorCli))
        && let Some(rest) = title_analysis.cursor_label
    {
        if rest == "Working" || rest.starts_with("Working ") {
            return PaneStatus {
                kind: StatusKind::Busy,
                source: StatusSource::TmuxTitle,
            };
        }
        if rest == "Ready" || rest.starts_with("Ready ") {
            return PaneStatus {
                kind: StatusKind::Idle,
                source: StatusSource::TmuxTitle,
            };
        }
    }

    if matches!(provider, Some(Provider::Pi))
        && title_analysis.pi_label.is_some()
        && title_analysis.has_spinner_glyph
    {
        return PaneStatus {
            kind: StatusKind::Busy,
            source: StatusSource::TmuxTitle,
        };
    }

    PaneStatus {
        kind: StatusKind::Unknown,
        source: StatusSource::NotChecked,
    }
}

pub(crate) fn infer_status(title_status: PaneStatus, published_state: Option<&str>) -> PaneStatus {
    if title_status.kind != StatusKind::Unknown {
        return title_status;
    }

    match published_state.map(|value| value.trim().to_ascii_lowercase()) {
        Some(state) if state == "busy" => PaneStatus {
            kind: StatusKind::Busy,
            source: StatusSource::PaneMetadata,
        },
        Some(state) if state == "idle" => PaneStatus {
            kind: StatusKind::Idle,
            source: StatusSource::PaneMetadata,
        },
        Some(state) if state == "unknown" => PaneStatus {
            kind: StatusKind::Unknown,
            source: StatusSource::PaneMetadata,
        },
        _ => title_status,
    }
}
