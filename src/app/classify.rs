use super::*;

pub(crate) fn pane_from_row(row: TmuxPaneRow) -> PaneRecord {
    let agent_metadata = AgentMetadata {
        provider: row.agent_provider.clone(),
        label: row.agent_label.clone(),
        cwd: row.agent_cwd.clone(),
        state: row.agent_state.clone(),
        session_id: row.agent_session_id.clone(),
    };
    let provider_match = classify_provider(
        agent_metadata.provider.as_deref(),
        &row.pane_current_command,
        &row.pane_title_raw,
    );
    let provider = provider_match.as_ref().map(|matched| matched.provider);
    let title_status = infer_title_status(provider, &row.pane_title_raw);
    let status = infer_status(title_status, agent_metadata.state.as_deref());

    PaneRecord {
        pane_id: row.pane_id,
        location: PaneLocation {
            session_name: row.session_name,
            window_index: row.window_index,
            pane_index: row.pane_index,
            window_name: row.window_name.clone(),
        },
        tmux: TmuxPaneMetadata {
            pane_pid: row.pane_pid,
            pane_tty: row.pane_tty,
            pane_current_path: row.pane_current_path,
            pane_current_command: row.pane_current_command.clone(),
            pane_title_raw: row.pane_title_raw.clone(),
            session_id: row.session_id.clone(),
            window_id: row.window_id.clone(),
        },
        display: display_metadata(
            provider,
            agent_metadata.label.as_deref(),
            &row.pane_title_raw,
            &row.pane_current_command,
            &row.window_name,
        ),
        provider,
        status,
        classification: PaneClassification {
            matched_by: provider_match.as_ref().map(|matched| matched.matched_by),
            confidence: provider_match.as_ref().map(|matched| matched.confidence),
            reasons: provider_match
                .map(|matched| matched.reasons)
                .unwrap_or_default(),
        },
        agent_metadata,
        diagnostics: PaneDiagnostics {
            cache_origin: "direct_snapshot".to_string(),
        },
    }
}

pub(crate) fn classify_provider(
    published_provider: Option<&str>,
    command: &str,
    title: &str,
) -> Option<ProviderMatch> {
    let title = title.trim();
    let command = command.trim();

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

    if let Some(provider) = provider_from_title(title) {
        return Some(ProviderMatch {
            provider,
            matched_by: ClassificationMatchKind::PaneTitle,
            confidence: ClassificationConfidence::High,
            reasons: vec![format!("pane_title={title}")],
        });
    }

    if let Some(provider) = provider_from_command(command) {
        return Some(ProviderMatch {
            provider,
            matched_by: ClassificationMatchKind::PaneCurrentCommand,
            confidence: ClassificationConfidence::Medium,
            reasons: vec![format!("pane_current_command={command}")],
        });
    }

    None
}

fn provider_from_metadata(provider: Option<&str>) -> Option<Provider> {
    let normalized = provider?.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "codex" => Some(Provider::Codex),
        "claude" => Some(Provider::Claude),
        "gemini" => Some(Provider::Gemini),
        "opencode" => Some(Provider::Opencode),
        _ => None,
    }
}

fn provider_from_title(title: &str) -> Option<Provider> {
    let title = title.trim();
    if title.is_empty() {
        return None;
    }

    let stripped = strip_known_status_glyph(title);
    if stripped.starts_with("Claude Code | ")
        || stripped.starts_with("Claude | ")
        || stripped == "Claude Code"
    {
        return Some(Provider::Claude);
    }

    if stripped.starts_with("OC | ") {
        return Some(Provider::Opencode);
    }

    if looks_like_codex_title(stripped) {
        return Some(Provider::Codex);
    }

    let lower = stripped.to_ascii_lowercase();
    if lower.contains("gemini") {
        return Some(Provider::Gemini);
    }

    None
}

fn provider_from_command(command: &str) -> Option<Provider> {
    if matches_provider_name(command, "codex") {
        return Some(Provider::Codex);
    }
    if matches_provider_name(command, "claude") {
        return Some(Provider::Claude);
    }
    if matches_provider_name(command, "gemini") {
        return Some(Provider::Gemini);
    }
    if matches_provider_name(command, "opencode") {
        return Some(Provider::Opencode);
    }

    None
}

pub(crate) fn infer_title_status(provider: Option<Provider>, title: &str) -> PaneStatus {
    let title = title.trim();
    let stripped = strip_known_status_glyph(title);

    if matches!(provider, Some(Provider::Claude)) {
        if has_spinner_glyph(title) {
            return PaneStatus {
                kind: StatusKind::Busy,
                source: StatusSource::TmuxTitle,
            };
        }
        if has_idle_glyph(title) {
            return PaneStatus {
                kind: StatusKind::Idle,
                source: StatusSource::TmuxTitle,
            };
        }
        if let Some(rest) = strip_claude_title_prefix(stripped) {
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
        if stripped == "Working" || stripped.ends_with("| Working") {
            return PaneStatus {
                kind: StatusKind::Busy,
                source: StatusSource::TmuxTitle,
            };
        }
        if stripped == "Ready"
            || stripped == "Waiting"
            || stripped.ends_with("| Ready")
            || stripped.ends_with("| Waiting")
        {
            return PaneStatus {
                kind: StatusKind::Idle,
                source: StatusSource::TmuxTitle,
            };
        }
    }

    if matches!(provider, Some(Provider::Gemini)) {
        if title.contains("Working") {
            return PaneStatus {
                kind: StatusKind::Busy,
                source: StatusSource::TmuxTitle,
            };
        }
        if title.contains("Ready") {
            return PaneStatus {
                kind: StatusKind::Idle,
                source: StatusSource::TmuxTitle,
            };
        }
    }

    if matches!(provider, Some(Provider::Opencode))
        && let Some(rest) = stripped.strip_prefix("OC | ")
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

pub(crate) fn display_metadata(
    provider: Option<Provider>,
    published_label: Option<&str>,
    raw_title: &str,
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

    let title = raw_title.trim();
    if !title.is_empty() {
        let label = normalize_title_for_display(title);
        return DisplayMetadata {
            activity_label: infer_activity_label(provider, &label),
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

pub(crate) fn normalize_title_for_display(title: &str) -> String {
    let stripped = strip_known_status_glyph(title).trim();
    if let Some(stripped) = strip_claude_title_prefix(stripped) {
        return stripped.to_string();
    }
    if let Some(stripped) = strip_opencode_title_prefix(stripped) {
        return stripped.to_string();
    }
    let codex_normalized = normalize_codex_wrapper_title(stripped);
    let codex_normalized = strip_codex_args_from_title(&codex_normalized);
    strip_codex_provider_suffix(&codex_normalized)
}

fn strip_claude_title_prefix(title: &str) -> Option<&str> {
    title
        .strip_prefix("Claude Code | ")
        .or_else(|| title.strip_prefix("Claude | "))
}

fn strip_opencode_title_prefix(title: &str) -> Option<&str> {
    title.strip_prefix("OC | ")
}

fn infer_activity_label(provider: Option<Provider>, label: &str) -> Option<String> {
    let label = label.trim();
    if label.is_empty() {
        return None;
    }

    if matches!(provider, Some(Provider::Codex))
        && let Some((activity, status)) = label.rsplit_once(" | ")
        && is_generic_status_label(status)
    {
        let activity = activity.trim();
        if !activity.is_empty() {
            return Some(activity.to_string());
        }
    }

    if is_generic_status_label(label) {
        return None;
    }

    match provider {
        Some(Provider::Codex) => Some(label.to_string()),
        Some(Provider::Claude) | Some(Provider::Gemini) | Some(Provider::Opencode) => {
            Some(label.to_string())
        }
        _ => None,
    }
}

fn is_generic_status_label(label: &str) -> bool {
    matches!(label.trim(), "Working" | "Waiting" | "Ready")
}

pub(crate) fn strip_known_status_glyph(title: &str) -> &str {
    let trimmed = title.trim_start();
    let Some(first) = trimmed.chars().next() else {
        return trimmed;
    };

    if !(CLAUDE_SPINNER_GLYPHS.contains(&first) || IDLE_GLYPHS.contains(&first)) {
        return trimmed;
    }

    let rest = &trimmed[first.len_utf8()..];
    rest.trim_start()
}

fn has_spinner_glyph(title: &str) -> bool {
    title
        .trim_start()
        .chars()
        .next()
        .is_some_and(|glyph| CLAUDE_SPINNER_GLYPHS.contains(&glyph))
}

fn has_idle_glyph(title: &str) -> bool {
    title
        .trim_start()
        .chars()
        .next()
        .is_some_and(|glyph| IDLE_GLYPHS.contains(&glyph))
}

fn normalize_codex_wrapper_title(title: &str) -> String {
    if title.contains("lgpt.sh")
        && let Some((prefix, _)) = title.rsplit_once(':')
    {
        let prefix = prefix.trim_end();
        if !prefix.is_empty() {
            return prefix.to_string();
        }
    }

    title.to_string()
}

fn strip_codex_args_from_title(title: &str) -> String {
    if let Some((prefix, _suffix)) = title.split_once(" codex ") {
        return format!("{prefix} codex");
    }

    title.to_string()
}

fn strip_codex_provider_suffix(title: &str) -> String {
    if let Some((prefix, suffix)) = title.rsplit_once(':')
        && matches!(suffix.trim(), "gpt" | "codex")
    {
        let prefix = prefix.trim_end();
        if !prefix.is_empty() {
            return prefix.to_string();
        }
    }

    title.to_string()
}

fn matches_provider_name(command: &str, provider: &str) -> bool {
    command == provider
        || command.strip_prefix(provider) == Some("")
        || command
            .strip_prefix(provider)
            .is_some_and(|suffix| suffix.starts_with('-'))
}

pub(crate) fn looks_like_codex_title(title: &str) -> bool {
    if title.contains("lgpt.sh") {
        return true;
    }

    let Some((_, suffix)) = title.rsplit_once(':') else {
        return false;
    };

    let suffix = suffix.trim();
    suffix == "codex"
        || suffix.starts_with("codex ")
        || suffix.ends_with("/codex")
        || suffix.ends_with("/codex.sh")
}
