use super::*;

mod display;
mod pane_output;
mod proc_fallback;
mod provider_match;
mod status;
mod status_label;
mod title;

use display::display_metadata_from_analysis;
#[cfg(test)]
pub(crate) use display::{display_metadata, normalize_title_for_display};
pub(crate) use pane_output::{
    apply_pane_output_status_fallback, pane_output_status_fallback_candidate,
};
pub(crate) use proc_fallback::apply_proc_fallback;
#[cfg(test)]
pub(crate) use provider_match::classify_provider;
use provider_match::{
    classify_provider_from_analysis, current_command_for_analysis, is_version_like_command,
};
pub(crate) use status::infer_status;
#[cfg(test)]
pub(crate) use status::infer_title_status;
use status::infer_title_status_from_analysis;
use title::{
    TitleAnalysis, analyze_title, codex_activity_from_status_title, codex_run_state_from_title,
    command_basename,
};
#[cfg(test)]
pub(crate) use title::{looks_like_codex_title, strip_known_status_glyph};

pub(crate) fn pane_from_row(row: TmuxPaneRow) -> PaneRecord {
    let agent_metadata = AgentMetadata {
        provider: row.agent_provider.clone(),
        label: row.agent_label.clone(),
        cwd: row.agent_cwd.clone(),
        state: row.agent_state.clone(),
        session_id: row.agent_session_id.clone(),
    };
    let title_analysis = analyze_title(&row.pane_title_raw);
    let current_command = current_command_for_analysis(&row.pane_current_command);
    let provider_match = classify_provider_from_analysis(
        agent_metadata.provider.as_deref(),
        current_command,
        &title_analysis,
    );
    let derived = pane_derived_fields(
        &title_analysis,
        provider_match,
        &agent_metadata,
        current_command,
        &row.window_name,
    );

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
        display: derived.display,
        provider: derived.provider,
        status: derived.status,
        classification: derived.classification,
        agent_metadata,
        diagnostics: PaneDiagnostics {
            cache_origin: "direct_snapshot".to_string(),
            proc_fallback: ProcFallbackDiagnostics::default(),
        },
    }
}

pub(crate) fn panes_from_rows_with_proc_fallback(
    rows: Vec<TmuxPaneRow>,
    inspector: &impl proc::ProcessInspector,
) -> Vec<PaneRecord> {
    rows.into_iter()
        .map(|row| {
            let mut pane = pane_from_row(row);
            apply_proc_fallback(&mut pane, inspector);
            pane
        })
        .collect()
}

pub(in crate::app::classify) struct PaneDerivedFields {
    provider: Option<Provider>,
    status: PaneStatus,
    display: DisplayMetadata,
    classification: PaneClassification,
}

pub(in crate::app::classify) fn pane_derived_fields(
    title_analysis: &TitleAnalysis<'_>,
    provider_match: Option<ProviderMatch>,
    agent_metadata: &AgentMetadata,
    current_command: &str,
    window_name: &str,
) -> PaneDerivedFields {
    let provider = provider_match.as_ref().map(|matched| matched.provider);
    let match_kind = provider_match.as_ref().map(|matched| matched.matched_by);
    let title_status = infer_title_status_from_analysis(provider, match_kind, title_analysis);
    let status = infer_status(title_status, agent_metadata.state.as_deref());
    let display = display_metadata_from_analysis(
        title_analysis,
        provider,
        match_kind,
        agent_metadata.label.as_deref(),
        current_command,
        window_name,
    );
    let classification = PaneClassification {
        matched_by: match_kind,
        confidence: provider_match.as_ref().map(|matched| matched.confidence),
        reasons: provider_match
            .map(|matched| matched.reasons)
            .unwrap_or_default(),
    };

    PaneDerivedFields {
        provider,
        status,
        display,
        classification,
    }
}
