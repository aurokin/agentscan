use super::status_label::status_from_codex_run_state_label;
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TitleHintStrength {
    Weak,
    Strong,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TitleProviderHintKind {
    Explicit,
    Fuzzy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TitleProviderHint {
    provider: Provider,
    strength: TitleHintStrength,
    kind: TitleProviderHintKind,
}

pub(super) struct TitleAnalysis<'a> {
    pub(super) raw: &'a str,
    pub(super) stripped: &'a str,
    pub(super) has_spinner_glyph: bool,
    pub(super) has_idle_glyph: bool,
    pub(super) claude_label: Option<&'a str>,
    pub(super) opencode_label: Option<&'a str>,
    pub(super) copilot_label: Option<&'a str>,
    pub(super) cursor_label: Option<&'a str>,
    pub(super) pi_label: Option<&'a str>,
    pub(super) cursor_title_shaped: bool,
    provider_hint: Option<TitleProviderHint>,
    pub(super) codex_status_title: String,
    codex_normalized_label: String,
    pub(super) gemini_title: Option<GeminiTitle>,
}

#[derive(Clone, Debug)]
pub(super) struct GeminiTitle {
    pub(super) status: Option<StatusKind>,
    label: Option<String>,
    pub(super) activity_label: Option<String>,
    strong_provider_signal: bool,
}

impl<'a> TitleAnalysis<'a> {
    pub(super) fn classifyable_provider(&self) -> Option<Provider> {
        self.provider_hint
            .filter(|hint| hint.strength == TitleHintStrength::Strong)
            .map(|hint| hint.provider)
    }

    pub(super) fn conflicts_with_resolved_provider(
        &self,
        provider: Option<Provider>,
        provider_match_kind: Option<ClassificationMatchKind>,
    ) -> bool {
        if provider_match_kind == Some(ClassificationMatchKind::PaneTitle) {
            return false;
        }

        self.provider_hint.is_some_and(|hint| {
            hint.kind == TitleProviderHintKind::Explicit
                && hint.strength == TitleHintStrength::Strong
                && !matches!(provider, Some(resolved_provider) if resolved_provider == hint.provider)
        })
    }

    pub(super) fn normalized_label(&self, provider: Option<Provider>) -> Option<String> {
        if self.stripped.is_empty() {
            return None;
        }

        if let Some(stripped) = self.claude_label {
            return Some(stripped.to_string());
        }
        if let Some(stripped) = self.opencode_label {
            return Some(stripped.to_string());
        }
        if matches!(provider, Some(Provider::Copilot))
            && let Some(stripped) = self.copilot_label
        {
            return Some(stripped.to_string());
        }
        if matches!(provider, Some(Provider::CursorCli))
            && let Some(stripped) = self.cursor_label
        {
            return Some(stripped.to_string());
        }
        if matches!(provider, Some(Provider::Pi))
            && let Some(stripped) = self.pi_label
        {
            return Some(stripped.to_string());
        }

        if matches!(provider, Some(Provider::Codex)) {
            return Some(self.codex_normalized_label.clone());
        }
        if matches!(provider, Some(Provider::Gemini))
            && let Some(label) = self
                .gemini_title
                .as_ref()
                .and_then(|title| title.label.clone())
        {
            return Some(label);
        }

        Some(self.stripped.to_string())
    }
}

pub(super) fn analyze_title(raw_title: &str) -> TitleAnalysis<'_> {
    let raw = raw_title.trim();
    let stripped = strip_known_status_glyph(raw).trim();
    let has_spinner_glyph = has_spinner_glyph(raw);
    let has_idle_glyph = has_idle_glyph(raw);
    let claude_label = provider_prefixed_title_label(Provider::Claude, stripped);
    let opencode_label = provider_prefixed_title_label(Provider::Opencode, stripped);
    let copilot_label = provider_prefixed_title_label(Provider::Copilot, stripped);
    let cursor_label = provider_prefixed_title_label(Provider::CursorCli, stripped);
    let cursor_title_shaped = cursor_label.is_some()
        || provider_title_aliases(Provider::CursorCli)
            .iter()
            .any(|alias| stripped.eq_ignore_ascii_case(alias));
    let pi_label = looks_like_pi_title(stripped)
        .then_some(())
        .and_then(|_| provider_prefixed_title_label(Provider::Pi, stripped));
    let gemini_title = parse_gemini_terminal_title(stripped);

    let provider_hint = if claude_label.is_some() || title_matches_alias(Provider::Claude, stripped)
    {
        Some(TitleProviderHint {
            provider: Provider::Claude,
            strength: TitleHintStrength::Strong,
            kind: TitleProviderHintKind::Explicit,
        })
    } else if opencode_label.is_some() || title_matches_alias(Provider::Opencode, stripped) {
        Some(TitleProviderHint {
            provider: Provider::Opencode,
            strength: TitleHintStrength::Strong,
            kind: TitleProviderHintKind::Explicit,
        })
    } else if looks_like_codex_title(stripped) {
        Some(TitleProviderHint {
            provider: Provider::Codex,
            strength: TitleHintStrength::Strong,
            kind: TitleProviderHintKind::Explicit,
        })
    } else if copilot_label.is_some() || title_matches_alias(Provider::Copilot, stripped) {
        Some(TitleProviderHint {
            provider: Provider::Copilot,
            strength: TitleHintStrength::Strong,
            kind: TitleProviderHintKind::Explicit,
        })
    } else if cursor_title_shaped {
        Some(TitleProviderHint {
            provider: Provider::CursorCli,
            strength: TitleHintStrength::Strong,
            kind: TitleProviderHintKind::Explicit,
        })
    } else if pi_label.is_some() {
        Some(TitleProviderHint {
            provider: Provider::Pi,
            strength: if stripped.starts_with("π - ") || has_spinner_glyph {
                TitleHintStrength::Strong
            } else {
                TitleHintStrength::Weak
            },
            kind: TitleProviderHintKind::Explicit,
        })
    } else if gemini_title
        .as_ref()
        .is_some_and(|title| title.strong_provider_signal)
    {
        Some(TitleProviderHint {
            provider: Provider::Gemini,
            strength: TitleHintStrength::Strong,
            kind: TitleProviderHintKind::Explicit,
        })
    } else if stripped.to_ascii_lowercase().contains("gemini") {
        Some(TitleProviderHint {
            provider: Provider::Gemini,
            strength: TitleHintStrength::Weak,
            kind: TitleProviderHintKind::Fuzzy,
        })
    } else {
        None
    };

    let codex_status_title = normalize_codex_title_before_status(stripped);
    let codex_normalized_label = normalize_codex_terminal_title_label(&codex_status_title);

    TitleAnalysis {
        raw,
        stripped,
        has_spinner_glyph,
        has_idle_glyph,
        claude_label,
        opencode_label,
        copilot_label,
        cursor_label,
        pi_label,
        cursor_title_shaped,
        provider_hint,
        codex_status_title,
        codex_normalized_label,
        gemini_title,
    }
}
fn provider_prefixed_title_label(provider: Provider, title: &str) -> Option<&str> {
    provider_title_prefixes(provider)
        .iter()
        .find_map(|prefix| title.strip_prefix(prefix))
}

fn title_matches_alias(provider: Provider, title: &str) -> bool {
    provider_title_aliases(provider)
        .iter()
        .any(|alias| title.eq_ignore_ascii_case(alias))
}

fn parse_gemini_terminal_title(title: &str) -> Option<GeminiTitle> {
    let title = title.trim();
    if let Some(context) = legacy_gemini_title_context(title) {
        return Some(GeminiTitle {
            status: None,
            label: context,
            activity_label: None,
            strong_provider_signal: true,
        });
    }

    let mut chars = title.chars();
    let glyph = chars.next()?;
    let after_glyph = chars.as_str();
    let rest = after_glyph.trim_start();
    match glyph {
        '◇' => {
            let label = gemini_label_after_status(rest, "Ready");
            let has_context = gemini_status_title_has_context(rest, "Ready");
            Some(GeminiTitle {
                status: label.as_ref().map(|_| StatusKind::Idle),
                activity_label: None,
                strong_provider_signal: has_context,
                label,
            })
        }
        '✋' => {
            let label = gemini_label_after_status(rest, "Action Required");
            let has_context = gemini_status_title_has_context(rest, "Action Required");
            Some(GeminiTitle {
                status: label.as_ref().map(|_| StatusKind::Busy),
                activity_label: None,
                strong_provider_signal: has_context,
                label,
            })
        }
        '⏲' => {
            let label = gemini_label_after_status(rest, "Working…")
                .or_else(|| gemini_label_after_status(rest, "Working"));
            let has_context = gemini_status_title_has_context(rest, "Working…")
                || gemini_status_title_has_context(rest, "Working");
            Some(GeminiTitle {
                status: label.as_ref().map(|_| StatusKind::Busy),
                activity_label: None,
                strong_provider_signal: has_context,
                label,
            })
        }
        '✦' => {
            let (label, activity_label) = gemini_active_title_parts(rest);
            let has_context = split_gemini_activity_context(rest).1.is_some();
            Some(GeminiTitle {
                status: label.as_ref().map(|_| StatusKind::Busy),
                activity_label,
                strong_provider_signal: has_context,
                label,
            })
        }
        _ => None,
    }
}

fn legacy_gemini_title_context(title: &str) -> Option<Option<String>> {
    if title == "Gemini CLI" {
        return Some(None);
    }

    title
        .strip_prefix("Gemini CLI ")
        .and_then(gemini_title_context)
        .map(Some)
}

fn gemini_label_after_status(rest: &str, status_label: &str) -> Option<String> {
    let rest = rest.trim();
    if rest == status_label {
        return Some(status_label.to_string());
    }

    let context = rest.strip_prefix(status_label)?.trim_start();
    if context.is_empty() {
        return Some(status_label.to_string());
    }
    gemini_title_context(context)
}

fn gemini_status_title_has_context(rest: &str, status_label: &str) -> bool {
    rest.trim()
        .strip_prefix(status_label)
        .is_some_and(|context| gemini_title_context(context.trim_start()).is_some())
}

fn gemini_active_title_parts(rest: &str) -> (Option<String>, Option<String>) {
    let rest = rest.trim();
    if rest.is_empty() {
        return (None, None);
    }

    let (activity, context) = split_gemini_activity_context(rest);
    let activity = activity.trim();
    if matches!(activity, "Working" | "Working…") {
        return (context.or_else(|| Some(activity.to_string())), None);
    }
    let activity = activity.to_string();
    (Some(activity.clone()), Some(activity))
}

fn split_gemini_activity_context(rest: &str) -> (&str, Option<String>) {
    if let Some(open_index) = trailing_gemini_context_open_index(rest)
        && let Some(context) = gemini_title_context(&rest[open_index..])
    {
        return (&rest[..open_index], Some(context));
    }

    (rest, None)
}

fn trailing_gemini_context_open_index(value: &str) -> Option<usize> {
    if !value.ends_with(')') {
        return None;
    }

    let mut depth = 0_u32;
    for (index, character) in value.char_indices().rev() {
        match character {
            ')' => depth += 1,
            '(' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    let prefix = &value[..index];
                    return prefix.ends_with(char::is_whitespace).then_some(index);
                }
            }
            _ => {}
        }
    }

    None
}

fn gemini_title_context(value: &str) -> Option<String> {
    let context = value.strip_prefix('(')?.strip_suffix(')')?.trim();
    (!context.is_empty()).then(|| context.to_string())
}

#[cfg_attr(not(test), allow(dead_code))]
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

fn normalize_codex_terminal_title_label(title: &str) -> String {
    codex_activity_from_status_title(title).unwrap_or_else(|| {
        let wrapper_label = normalize_codex_wrapper_title(title);
        let command_label = strip_codex_args_from_title(&wrapper_label);
        strip_codex_provider_suffix(&command_label)
    })
}

fn normalize_codex_title_before_status(title: &str) -> String {
    strip_codex_provider_suffix(title)
}

pub(super) fn codex_activity_from_status_title(title: &str) -> Option<String> {
    if let Some((activity, status)) = title.rsplit_once(" | ")
        && status_from_codex_run_state_label(status).is_some()
    {
        let activity = activity.trim();
        if !activity.is_empty() {
            return Some(normalize_codex_activity_label(activity));
        }
    }

    if let Some((status, activity)) = title.split_once(" | ")
        && status_from_codex_run_state_label(status).is_some()
    {
        let activity = activity.trim();
        if !activity.is_empty() {
            return Some(normalize_codex_activity_label(activity));
        }
    }

    None
}

fn normalize_codex_activity_label(activity: &str) -> String {
    if !looks_like_codex_title(activity) {
        return activity.to_string();
    }

    let wrapper_label = normalize_codex_wrapper_title(activity);
    let command_label = strip_codex_args_from_title(&wrapper_label);
    strip_codex_provider_suffix(&command_label)
}

pub(super) fn codex_run_state_from_title(title: &str) -> Option<StatusKind> {
    if let Some(status) = status_from_codex_run_state_label(title) {
        return Some(status);
    }
    if let Some((_activity, status)) = title.rsplit_once(" | ")
        && let Some(status) = status_from_codex_run_state_label(status)
    {
        return Some(status);
    }
    if let Some((status, _activity)) = title.split_once(" | ")
        && let Some(status) = status_from_codex_run_state_label(status)
    {
        return Some(status);
    }

    None
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

#[cfg_attr(not(test), allow(dead_code))]
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

fn looks_like_pi_title(title: &str) -> bool {
    if let Some(rest) = title.strip_prefix("π - ") {
        return pi_title_has_nonempty_segments(rest);
    }

    if let Some(rest) = title.strip_prefix("pi - ") {
        return pi_title_has_multiple_segments(rest);
    }

    false
}

fn pi_title_has_nonempty_segments(rest: &str) -> bool {
    rest.split(" - ")
        .map(str::trim)
        .all(|segment| !segment.is_empty())
}

fn pi_title_has_multiple_segments(rest: &str) -> bool {
    let mut segments = rest.split(" - ").map(str::trim);
    let Some(first) = segments.next() else {
        return false;
    };
    let Some(second) = segments.next() else {
        return false;
    };

    if first.is_empty() || second.is_empty() {
        return false;
    }

    segments.all(|segment| !segment.is_empty())
}
