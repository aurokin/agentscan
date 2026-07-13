use super::status_label::{
    status_from_codex_run_state_label, status_from_gemini_generic_title,
    status_from_ready_working_prefix,
};
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
    pub(super) grok_label: Option<&'a str>,
    pub(super) droid_label: Option<&'a str>,
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

struct TitleProviderSignals<'a> {
    stripped: &'a str,
    has_spinner_glyph: bool,
    claude_label: Option<&'a str>,
    opencode_label: Option<&'a str>,
    copilot_label: Option<&'a str>,
    cursor_title_shaped: bool,
    pi_label: Option<&'a str>,
    grok_label: Option<&'a str>,
    droid_label: Option<&'a str>,
    gemini_strong_provider_signal: bool,
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
                && (hint.strength == TitleHintStrength::Strong
                    || matches!(
                        hint.provider,
                        Provider::Copilot | Provider::CursorCli | Provider::Droid
                    ))
                && !matches!(provider, Some(resolved_provider) if resolved_provider == hint.provider)
        })
    }

    pub(super) fn normalized_label(&self, provider: Option<Provider>) -> Option<String> {
        if self.stripped.is_empty() {
            return None;
        }

        // Claude and OpenCode title prefixes are self-identifying, so their stripped label wins
        // regardless of the resolved provider. This precedence predates the per-provider spec
        // dispatch below and is preserved here as an explicit special case.
        if let Some(stripped) = self.claude_label {
            return Some(stripped.to_string());
        }
        if let Some(stripped) = self.opencode_label {
            return Some(stripped.to_string());
        }

        if let Some(provider) = provider
            && let Some(spec) = provider_title_spec(provider)
            && let Some(label) = (spec.normalized_label)(self)
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
    let grok_label = grok_title_label(stripped);
    let droid_label = provider_prefixed_title_label(Provider::Droid, stripped);
    let gemini_title = parse_gemini_terminal_title(stripped);

    let provider_hint = provider_hint_from_title_signals(&TitleProviderSignals {
        stripped,
        has_spinner_glyph,
        claude_label,
        opencode_label,
        copilot_label,
        cursor_title_shaped,
        pi_label,
        grok_label,
        droid_label,
        gemini_strong_provider_signal: gemini_title
            .as_ref()
            .is_some_and(|title| title.strong_provider_signal),
    });

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
        grok_label,
        droid_label,
        cursor_title_shaped,
        provider_hint,
        codex_status_title,
        codex_normalized_label,
        gemini_title,
    }
}

// Per-provider title/status behavior, table-driven so adding a provider means adding one
// `ProviderTitleSpec` row rather than editing the hint, status, and label ladders in lockstep.
//   * `detect_hint` powers `provider_hint_from_title_signals`; it returns the hint strength and
//     kind when this provider's title signals match, or `None` otherwise.
//   * `title_status` powers `infer_title_status_from_analysis` via `provider_title_status`.
//   * `normalized_label` powers `TitleAnalysis::normalized_label` for the resolved provider.
// Bespoke parsers (Codex status-title, Gemini structured title) stay as dedicated functions and
// are merely referenced here rather than open-coded in the ladders.
type HintDetectFn =
    fn(&TitleProviderSignals<'_>) -> Option<(TitleHintStrength, TitleProviderHintKind)>;
type TitleStatusFn = fn(&TitleAnalysis<'_>) -> Option<StatusKind>;
type NormalizedLabelFn = fn(&TitleAnalysis<'_>) -> Option<String>;

struct ProviderTitleSpec {
    provider: Provider,
    detect_hint: HintDetectFn,
    title_status: TitleStatusFn,
    normalized_label: NormalizedLabelFn,
}

// Row order defines cross-ladder hint priority: strong self-identifying providers (Claude,
// OpenCode, Codex) are probed before the weak title-prefix providers, so a Codex title that also
// carries a stray `Copilot |` prefix still resolves to Codex. Gemini is probed last because its
// weakest signal is a fuzzy "contains gemini" substring match. `title_status` and
// `normalized_label` are looked up by `provider`, so their ordering here does not matter.
const PROVIDER_TITLE_SPECS: &[ProviderTitleSpec] = &[
    ProviderTitleSpec {
        provider: Provider::Claude,
        detect_hint: detect_claude_hint,
        title_status: claude_title_status,
        normalized_label: no_spec_label,
    },
    ProviderTitleSpec {
        provider: Provider::Opencode,
        detect_hint: detect_opencode_hint,
        title_status: no_title_status,
        normalized_label: no_spec_label,
    },
    ProviderTitleSpec {
        provider: Provider::Codex,
        detect_hint: detect_codex_hint,
        title_status: codex_title_status,
        normalized_label: codex_spec_label,
    },
    ProviderTitleSpec {
        provider: Provider::Copilot,
        detect_hint: detect_copilot_hint,
        title_status: copilot_title_status,
        normalized_label: copilot_spec_label,
    },
    ProviderTitleSpec {
        provider: Provider::CursorCli,
        detect_hint: detect_cursor_hint,
        title_status: cursor_title_status,
        normalized_label: cursor_spec_label,
    },
    ProviderTitleSpec {
        provider: Provider::Pi,
        detect_hint: detect_pi_hint,
        title_status: pi_title_status,
        normalized_label: pi_spec_label,
    },
    ProviderTitleSpec {
        provider: Provider::Grok,
        detect_hint: detect_grok_hint,
        title_status: grok_title_status,
        normalized_label: grok_spec_label,
    },
    ProviderTitleSpec {
        provider: Provider::Droid,
        detect_hint: detect_droid_hint,
        title_status: no_title_status,
        normalized_label: droid_spec_label,
    },
    ProviderTitleSpec {
        provider: Provider::Gemini,
        detect_hint: detect_gemini_hint,
        title_status: gemini_title_status,
        normalized_label: gemini_spec_label,
    },
    ProviderTitleSpec {
        provider: Provider::Aider,
        detect_hint: no_hint,
        title_status: no_title_status,
        normalized_label: no_spec_label,
    },
    ProviderTitleSpec {
        provider: Provider::Antigravity,
        detect_hint: no_hint,
        title_status: no_title_status,
        normalized_label: no_spec_label,
    },
    ProviderTitleSpec {
        provider: Provider::Hermes,
        detect_hint: no_hint,
        title_status: no_title_status,
        normalized_label: no_spec_label,
    },
];

fn provider_title_spec(provider: Provider) -> Option<&'static ProviderTitleSpec> {
    PROVIDER_TITLE_SPECS
        .iter()
        .find(|spec| spec.provider == provider)
}

pub(super) fn provider_title_status(
    provider: Provider,
    analysis: &TitleAnalysis<'_>,
) -> Option<StatusKind> {
    provider_title_spec(provider).and_then(|spec| (spec.title_status)(analysis))
}

fn provider_hint_from_title_signals(
    signals: &TitleProviderSignals<'_>,
) -> Option<TitleProviderHint> {
    PROVIDER_TITLE_SPECS.iter().find_map(|spec| {
        (spec.detect_hint)(signals).map(|(strength, kind)| TitleProviderHint {
            provider: spec.provider,
            strength,
            kind,
        })
    })
}

fn no_hint(_: &TitleProviderSignals<'_>) -> Option<(TitleHintStrength, TitleProviderHintKind)> {
    None
}

fn detect_claude_hint(
    signals: &TitleProviderSignals<'_>,
) -> Option<(TitleHintStrength, TitleProviderHintKind)> {
    (signals.claude_label.is_some() || title_matches_alias(Provider::Claude, signals.stripped))
        .then_some((TitleHintStrength::Strong, TitleProviderHintKind::Explicit))
}

fn detect_opencode_hint(
    signals: &TitleProviderSignals<'_>,
) -> Option<(TitleHintStrength, TitleProviderHintKind)> {
    (signals.opencode_label.is_some() || title_matches_alias(Provider::Opencode, signals.stripped))
        .then_some((TitleHintStrength::Strong, TitleProviderHintKind::Explicit))
}

fn detect_codex_hint(
    signals: &TitleProviderSignals<'_>,
) -> Option<(TitleHintStrength, TitleProviderHintKind)> {
    looks_like_codex_title(signals.stripped)
        .then_some((TitleHintStrength::Strong, TitleProviderHintKind::Explicit))
}

fn detect_copilot_hint(
    signals: &TitleProviderSignals<'_>,
) -> Option<(TitleHintStrength, TitleProviderHintKind)> {
    (signals.copilot_label.is_some() || title_matches_alias(Provider::Copilot, signals.stripped))
        .then_some((TitleHintStrength::Weak, TitleProviderHintKind::Explicit))
}

fn detect_cursor_hint(
    signals: &TitleProviderSignals<'_>,
) -> Option<(TitleHintStrength, TitleProviderHintKind)> {
    signals
        .cursor_title_shaped
        .then_some((TitleHintStrength::Weak, TitleProviderHintKind::Explicit))
}

fn detect_pi_hint(
    signals: &TitleProviderSignals<'_>,
) -> Option<(TitleHintStrength, TitleProviderHintKind)> {
    signals.pi_label.is_some().then(|| {
        let strength = if signals.stripped.starts_with("π - ") || signals.has_spinner_glyph {
            TitleHintStrength::Strong
        } else {
            TitleHintStrength::Weak
        };
        (strength, TitleProviderHintKind::Explicit)
    })
}

fn detect_grok_hint(
    signals: &TitleProviderSignals<'_>,
) -> Option<(TitleHintStrength, TitleProviderHintKind)> {
    signals.grok_label.is_some().then(|| {
        let strength = if signals.has_spinner_glyph {
            TitleHintStrength::Strong
        } else {
            TitleHintStrength::Weak
        };
        (strength, TitleProviderHintKind::Explicit)
    })
}

fn detect_droid_hint(
    signals: &TitleProviderSignals<'_>,
) -> Option<(TitleHintStrength, TitleProviderHintKind)> {
    signals
        .droid_label
        .is_some()
        .then_some((TitleHintStrength::Weak, TitleProviderHintKind::Explicit))
}

fn detect_gemini_hint(
    signals: &TitleProviderSignals<'_>,
) -> Option<(TitleHintStrength, TitleProviderHintKind)> {
    if signals.gemini_strong_provider_signal {
        Some((TitleHintStrength::Strong, TitleProviderHintKind::Explicit))
    } else if signals.stripped.to_ascii_lowercase().contains("gemini") {
        Some((TitleHintStrength::Weak, TitleProviderHintKind::Fuzzy))
    } else {
        None
    }
}

fn no_title_status(_: &TitleAnalysis<'_>) -> Option<StatusKind> {
    None
}

fn claude_title_status(analysis: &TitleAnalysis<'_>) -> Option<StatusKind> {
    if analysis.has_spinner_glyph {
        return Some(StatusKind::Busy);
    }
    if analysis.has_idle_glyph {
        return Some(StatusKind::Idle);
    }
    analysis
        .claude_label
        .and_then(status_from_ready_working_prefix)
}

fn codex_title_status(analysis: &TitleAnalysis<'_>) -> Option<StatusKind> {
    if codex_title_has_spinner_indicator(analysis.raw) {
        return Some(StatusKind::Busy);
    }
    codex_run_state_from_title(&analysis.codex_status_title)
}

fn gemini_title_status(analysis: &TitleAnalysis<'_>) -> Option<StatusKind> {
    analysis
        .gemini_title
        .as_ref()
        .and_then(|title| title.status)
        .or_else(|| status_from_gemini_generic_title(analysis.stripped))
}

fn copilot_title_status(analysis: &TitleAnalysis<'_>) -> Option<StatusKind> {
    if copilot_title_has_working_indicator(analysis.raw) {
        return Some(StatusKind::Busy);
    }
    analysis
        .copilot_label
        .and_then(status_from_ready_working_prefix)
}

fn cursor_title_status(analysis: &TitleAnalysis<'_>) -> Option<StatusKind> {
    analysis
        .cursor_label
        .and_then(status_from_ready_working_prefix)
}

fn pi_title_status(analysis: &TitleAnalysis<'_>) -> Option<StatusKind> {
    (analysis.pi_label.is_some() && analysis.has_spinner_glyph).then_some(StatusKind::Busy)
}

fn grok_title_status(analysis: &TitleAnalysis<'_>) -> Option<StatusKind> {
    analysis.has_spinner_glyph.then_some(StatusKind::Busy)
}

fn copilot_title_has_working_indicator(title: &str) -> bool {
    title.trim_start().starts_with("🤖")
}

fn no_spec_label(_: &TitleAnalysis<'_>) -> Option<String> {
    None
}

fn copilot_spec_label(analysis: &TitleAnalysis<'_>) -> Option<String> {
    analysis.copilot_label.map(str::to_string)
}

fn cursor_spec_label(analysis: &TitleAnalysis<'_>) -> Option<String> {
    analysis.cursor_label.map(str::to_string)
}

fn pi_spec_label(analysis: &TitleAnalysis<'_>) -> Option<String> {
    analysis.pi_label.map(str::to_string)
}

fn grok_spec_label(analysis: &TitleAnalysis<'_>) -> Option<String> {
    analysis.grok_label.map(str::to_string)
}

fn droid_spec_label(analysis: &TitleAnalysis<'_>) -> Option<String> {
    analysis.droid_label.map(str::to_string)
}

fn codex_spec_label(analysis: &TitleAnalysis<'_>) -> Option<String> {
    Some(analysis.codex_normalized_label.clone())
}

fn gemini_spec_label(analysis: &TitleAnalysis<'_>) -> Option<String> {
    analysis
        .gemini_title
        .as_ref()
        .and_then(|title| title.label.clone())
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

fn grok_title_label(title: &str) -> Option<&str> {
    if title.eq_ignore_ascii_case("grok") {
        return Some(title);
    }

    let suffix = " - grok";
    if title.len() <= suffix.len() {
        return None;
    }

    if !title.to_ascii_lowercase().ends_with(suffix) {
        return None;
    }

    let label = &title[..title.len() - suffix.len()];
    let label = label
        .trim()
        .strip_prefix("- ")
        .unwrap_or_else(|| label.trim())
        .trim();
    (!label.is_empty()).then_some(label)
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
    codex_activity_from_status_title(title).unwrap_or_else(|| normalize_codex_activity_label(title))
}

fn normalize_codex_title_before_status(title: &str) -> String {
    strip_codex_provider_suffix(title)
}

pub(super) fn codex_activity_from_status_title(title: &str) -> Option<String> {
    let segments = codex_title_segments(title);
    let status_index = codex_run_state_segment_index(&segments)?;
    let has_action_required = segments
        .iter()
        .any(|segment| codex_action_required_status_from_title_segment(segment).is_some());
    let candidates: Vec<&str> = segments
        .iter()
        .enumerate()
        .filter_map(|(index, segment)| {
            let keep = index != status_index
                && codex_action_required_status_from_title_segment(segment).is_none()
                && !(has_action_required && codex_status_from_title_segment(segment).is_some());
            keep.then_some(*segment)
        })
        .collect();
    let activity = candidates.join(" | ");
    let activity = normalize_codex_activity_label(&activity);

    (!activity.is_empty()).then_some(activity)
}

fn normalize_codex_activity_label(activity: &str) -> String {
    let activity = strip_codex_activity_indicator_tokens(activity);
    let wrapper_label = normalize_codex_wrapper_title(&activity);
    let command_label = strip_codex_args_from_title(&wrapper_label);
    if !looks_like_codex_title(&activity) {
        return command_label;
    }

    strip_codex_provider_suffix(&command_label)
}

pub(super) fn codex_run_state_from_title(title: &str) -> Option<StatusKind> {
    let segments = codex_title_segments(title);
    if segments
        .iter()
        .any(|segment| codex_action_required_status_from_title_segment(segment).is_some())
    {
        return Some(StatusKind::Busy);
    }

    let status_index = codex_run_state_segment_index(&segments)?;
    codex_status_from_title_segment(segments[status_index])
}

pub(super) fn codex_title_has_spinner_indicator(title: &str) -> bool {
    codex_title_segments(title)
        .into_iter()
        .any(codex_segment_has_boundary_spinner)
}

fn codex_title_segments(title: &str) -> Vec<&str> {
    title
        .split(" | ")
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn codex_run_state_segment_index(segments: &[&str]) -> Option<usize> {
    // Upstream Codex lets users configure terminal-title items in any order, and it does not tag
    // item provenance in the OSC title. Once provider identity is already resolved, prefer the
    // rightmost run-state-like segment so `activity | run-state | project` works as well as
    // status-first/last layouts. Pane-output fallback remains responsible when no run-state
    // segment is present.
    segments
        .iter()
        .rposition(|segment| codex_status_from_title_segment(segment).is_some())
}

fn codex_status_from_title_segment(segment: &str) -> Option<StatusKind> {
    let segment = segment.trim();
    status_from_codex_run_state_label(segment)
        .or_else(|| {
            status_from_codex_run_state_label(&strip_codex_activity_indicator_tokens(segment))
        })
        .or_else(|| codex_action_required_status_from_title_segment(segment))
}

fn codex_action_required_status_from_title_segment(segment: &str) -> Option<StatusKind> {
    matches!(
        segment.trim(),
        "[ ! ] Action Required" | "[ . ] Action Required" | "Action Required"
    )
    .then_some(StatusKind::Busy)
}

fn strip_codex_activity_indicator_tokens(value: &str) -> String {
    codex_title_segments(value)
        .into_iter()
        .map(strip_codex_boundary_spinner)
        .collect::<Vec<_>>()
        .join(" | ")
        .trim()
        .to_string()
}

fn codex_segment_has_boundary_spinner(segment: &str) -> bool {
    let segment = segment.trim();
    segment.chars().next().is_some_and(codex_spinner_glyph)
        || segment.chars().next_back().is_some_and(codex_spinner_glyph)
}

fn strip_codex_boundary_spinner(segment: &str) -> String {
    let segment = segment.trim();
    let segment = segment
        .strip_prefix(codex_spinner_glyph)
        .unwrap_or(segment)
        .trim_start();
    let segment = segment
        .strip_suffix(codex_spinner_glyph)
        .unwrap_or(segment)
        .trim_end();
    segment.to_string()
}

fn codex_spinner_glyph(character: char) -> bool {
    CLAUDE_SPINNER_GLYPHS.contains(&character)
}

fn strip_codex_args_from_title(title: &str) -> String {
    if let Some((prefix, suffix)) = title.split_once(" codex ")
        && prefix.trim_end().ends_with(':')
        && suffix.trim_start().starts_with('-')
    {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn provider_hint(title: &str) -> Option<TitleProviderHint> {
        analyze_title(title).provider_hint
    }

    #[test]
    fn provider_hint_keeps_codex_priority_over_weak_title_prefixes() {
        assert_eq!(
            provider_hint("Copilot | Review patch: codex"),
            Some(TitleProviderHint {
                provider: Provider::Codex,
                strength: TitleHintStrength::Strong,
                kind: TitleProviderHintKind::Explicit,
            })
        );
    }

    #[test]
    fn provider_hint_preserves_explicit_title_strengths() {
        assert_eq!(
            provider_hint("Claude Code | Refactor auth"),
            Some(TitleProviderHint {
                provider: Provider::Claude,
                strength: TitleHintStrength::Strong,
                kind: TitleProviderHintKind::Explicit,
            })
        );
        assert_eq!(
            provider_hint("Copilot | Review patch"),
            Some(TitleProviderHint {
                provider: Provider::Copilot,
                strength: TitleHintStrength::Weak,
                kind: TitleProviderHintKind::Explicit,
            })
        );
        assert_eq!(
            provider_hint("π - agentscan - refactor"),
            Some(TitleProviderHint {
                provider: Provider::Pi,
                strength: TitleHintStrength::Strong,
                kind: TitleProviderHintKind::Explicit,
            })
        );
    }

    #[test]
    fn provider_hint_preserves_gemini_signal_kinds() {
        assert_eq!(
            provider_hint("◇ Ready (agentscan)"),
            Some(TitleProviderHint {
                provider: Provider::Gemini,
                strength: TitleHintStrength::Strong,
                kind: TitleProviderHintKind::Explicit,
            })
        );
        assert_eq!(
            provider_hint("notes about gemini ergonomics"),
            Some(TitleProviderHint {
                provider: Provider::Gemini,
                strength: TitleHintStrength::Weak,
                kind: TitleProviderHintKind::Fuzzy,
            })
        );
    }
}
