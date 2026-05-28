use super::*;

mod antigravity;
mod claude;
mod codex;
mod copilot;
mod cursor_cli;
mod droid;
mod gemini;
mod grok;
mod hermes;
mod opencode;
mod pi;

pub(crate) fn pane_output_status_fallback_candidate(pane: &PaneRecord) -> bool {
    matches!(
        pane.provider,
        Some(Provider::Codex)
            | Some(Provider::Claude)
            | Some(Provider::Copilot)
            | Some(Provider::CursorCli)
            | Some(Provider::Gemini)
            | Some(Provider::Grok)
            | Some(Provider::Hermes)
            | Some(Provider::Opencode)
            | Some(Provider::Pi)
            | Some(Provider::Antigravity)
            | Some(Provider::Droid)
    ) && pane.status.kind == StatusKind::Unknown
        && pane.status.source == StatusSource::NotChecked
}

pub(crate) fn apply_pane_output_status_fallback(pane: &mut PaneRecord, output: &str) {
    if !pane_output_status_fallback_candidate(pane) {
        return;
    }

    // Agent TUIs render their current prompt/footer at the bottom of what they have
    // drawn, but a pane is often taller than that — a freshly started or top-rendered
    // agent leaves dozens of blank trailing rows below its UI. Anchor every "near the
    // current footer" matcher to the last rendered line by dropping trailing blank rows
    // once here, so each provider matcher does not have to fight pane padding.
    let output = trim_trailing_blank_lines(output);

    let status = match pane.provider {
        Some(Provider::Codex) => codex::status(output),
        Some(Provider::Claude) => claude::status(output),
        Some(Provider::Copilot) => copilot::status(output),
        Some(Provider::CursorCli) => cursor_cli::status(output),
        Some(Provider::Gemini) => gemini::status(output),
        Some(Provider::Grok) => grok::status(output),
        Some(Provider::Hermes) => hermes::status(output),
        Some(Provider::Opencode) => opencode::status(output),
        Some(Provider::Pi) => pi::status(output),
        Some(Provider::Antigravity) => antigravity::status(output),
        Some(Provider::Droid) => droid::status(output),
        _ => None,
    };

    if let Some(kind) = status {
        pane.status = PaneStatus::pane_output(kind);
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
