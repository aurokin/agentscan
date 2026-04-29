use super::*;

pub(super) fn status_from_ready_working_prefix(label: &str) -> Option<StatusKind> {
    let label = label.trim();
    if label == "Working" || label.starts_with("Working ") {
        return Some(StatusKind::Busy);
    }
    if label == "Ready" || label.starts_with("Ready ") {
        return Some(StatusKind::Idle);
    }

    None
}

pub(super) fn status_from_codex_run_state_label(label: &str) -> Option<StatusKind> {
    match label.trim() {
        "Working" | "Waiting" | "Thinking" | "Starting" | "Undoing" => Some(StatusKind::Busy),
        "Ready" => Some(StatusKind::Idle),
        _ => None,
    }
}

pub(super) fn is_generic_display_status_label(label: &str) -> bool {
    status_from_codex_run_state_label(label).is_some()
}

pub(super) fn status_from_gemini_generic_title(label: &str) -> Option<StatusKind> {
    match label.trim() {
        "Ready" => Some(StatusKind::Idle),
        "Working" | "Working…" | "Action Required" => Some(StatusKind::Busy),
        _ => None,
    }
}
