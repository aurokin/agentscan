use super::{PaneRecord, PaneStatus, Provider, StatusKind, StatusSource};

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
mod opencode;
mod pi;

use frame::PaneOutputFrame;

type PaneOutputClassifier = fn(&str) -> Option<StatusKind>;

pub(crate) fn pane_output_status_fallback_candidate(pane: &PaneRecord) -> bool {
    pane.provider.and_then(classifier_for).is_some()
        && pane.status.kind == StatusKind::Unknown
        && pane.status.source == StatusSource::NotChecked
}

pub(crate) fn apply_pane_output_status_fallback(pane: &mut PaneRecord, output: &str) {
    if pane.status.kind != StatusKind::Unknown || pane.status.source != StatusSource::NotChecked {
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
        pane.status = PaneStatus::pane_output(kind);
    }
}

fn classifier_for(provider: Provider) -> Option<PaneOutputClassifier> {
    match provider {
        Provider::Codex => Some(codex::status),
        Provider::Claude => Some(claude::status),
        Provider::Copilot => Some(copilot::status),
        Provider::CursorCli => Some(cursor_cli::status),
        Provider::Gemini => Some(gemini::status),
        Provider::Grok => Some(grok::status),
        Provider::Hermes => Some(hermes::status),
        Provider::Opencode => Some(opencode::status),
        Provider::Pi => Some(pi::status),
        Provider::Antigravity => Some(antigravity::status),
        Provider::Droid => Some(droid::status),
    }
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

fn dotted_version_token(token: &str) -> bool {
    let segments: Vec<&str> = token.split('.').collect();
    segments.len() >= 3
        && segments
            .iter()
            .all(|segment| !segment.is_empty() && segment.chars().all(|ch| ch.is_ascii_digit()))
}
