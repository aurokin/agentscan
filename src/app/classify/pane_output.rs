use super::{PaneRecord, PaneStatus, Provider, StatusKind, StatusSource, is_version_like_command};

mod antigravity;
mod claude;
mod codex;
mod copilot;
mod cursor_cli;
mod droid;
mod frame;
mod gemini;
mod grok;
mod hermes;
mod kimi_code;
mod opencode;
mod pi;

use frame::PaneOutputFrame;

type PaneOutputClassifier = fn(&str) -> Option<StatusKind>;

pub(crate) fn pane_output_status_fallback_candidate(pane: &PaneRecord) -> bool {
    pane.provider.and_then(classifier_for).is_some()
        && ((pane.status.kind == StatusKind::Unknown
            && pane.status.source == StatusSource::NotChecked)
            || pane_output_status_refinement_candidate(pane))
}

pub(crate) fn pane_output_status_activity_candidate(pane: &PaneRecord) -> bool {
    pane.status.source == StatusSource::PaneOutput || pane_output_status_fallback_candidate(pane)
}

pub(crate) fn apply_pane_output_status_fallback(pane: &mut PaneRecord, output: &str) {
    let can_fill_unknown =
        pane.status.kind == StatusKind::Unknown && pane.status.source == StatusSource::NotChecked;
    let can_refine_idle = idle_title_refinement_candidate(pane);
    let can_refine_waiting = waiting_refinement_candidate(pane);
    if !(can_fill_unknown || can_refine_idle || can_refine_waiting) {
        return;
    }

    let Some(classifier) = pane.provider.and_then(classifier_for) else {
        return;
    };

    // Agent TUIs render their current prompt/footer at the bottom of what they have
    // drawn, but a pane is often taller than that — a freshly started or top-rendered
    // agent leaves dozens of blank trailing rows below its UI. Anchor every "near the
    // current footer" matcher to the last rendered line by dropping trailing blank rows
    // once here, so each provider matcher does not have to fight pane padding.
    let output = trim_trailing_blank_lines(output);

    if let Some(kind) = classifier(output) {
        let accept = can_fill_unknown
            || (can_refine_idle && matches!(kind, StatusKind::Busy | StatusKind::Waiting))
            // A busy-titled pane may only be upgraded to waiting; a busy or idle
            // read keeps the title verdict so refinement can never invert status
            // or churn provenance.
            || (can_refine_waiting && kind == StatusKind::Waiting);
        if accept {
            pane.status = PaneStatus::pane_output(kind);
        }
    }
}

fn pane_output_status_refinement_candidate(pane: &PaneRecord) -> bool {
    idle_title_refinement_candidate(pane) || waiting_refinement_candidate(pane)
}

fn idle_title_refinement_candidate(pane: &PaneRecord) -> bool {
    matches!(pane.provider, Some(Provider::Gemini))
        && pane.status.kind == StatusKind::Idle
        && pane.status.source == StatusSource::TmuxTitle
}

// Providers whose pane-output matchers can distinguish "waiting on the user"
// (approval/question prompts) from plain busy. A title-derived busy pane is
// worth one capture to check for that upgrade; trusted metadata busy is not
// second-guessed.
fn waiting_refinement_candidate(pane: &PaneRecord) -> bool {
    matches!(
        pane.provider,
        Some(Provider::Claude | Provider::Codex | Provider::Opencode)
    ) && pane.status.kind == StatusKind::Busy
        && pane.status.source == StatusSource::TmuxTitle
}

pub(crate) fn pane_output_status_candidate_cacheable(pane: &PaneRecord) -> bool {
    !matches!(pane.provider, Some(Provider::Gemini))
}

fn classifier_for(provider: Provider) -> Option<PaneOutputClassifier> {
    match provider {
        Provider::Codex => Some(codex::status),
        Provider::Claude => Some(claude::status),
        Provider::Aider => None,
        Provider::Copilot => Some(copilot::status),
        Provider::CursorCli => Some(cursor_cli::status),
        Provider::Gemini => Some(gemini::status),
        Provider::Grok => Some(grok::status),
        Provider::Hermes => Some(hermes::status),
        Provider::Opencode => Some(opencode::status),
        Provider::Pi => Some(pi::status),
        Provider::Antigravity => Some(antigravity::status),
        Provider::Droid => Some(droid::status),
        Provider::KimiCode => Some(kimi_code::status),
    }
}

#[cfg(test)]
pub(crate) fn classify_output(provider: Provider, output: &str) -> Option<StatusKind> {
    classifier_for(provider).and_then(|classifier| classifier(trim_trailing_blank_lines(output)))
}

/// Returns `output` with trailing blank (whitespace-only) lines removed.
///
/// Only trailing *blank* rows are dropped; blank rows between content and trailing rendered
/// content are preserved, so the distance from a prompt to real content above it still
/// anchors the "stale frame" guards.
fn trim_trailing_blank_lines(output: &str) -> &str {
    let mut end = 0;
    let mut offset = 0;
    for line in output.split_inclusive('\n') {
        offset += line.len();
        if !line.trim().is_empty() {
            end = offset;
        }
    }
    &output[..end]
}
