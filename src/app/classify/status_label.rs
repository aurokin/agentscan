use super::*;

pub(super) fn status_from_ready_working_prefix(label: &str) -> Option<StatusKind> {
    match label.trim() {
        "Working" | "Working…" | "Working..." => Some(StatusKind::Busy),
        "Ready" => Some(StatusKind::Idle),
        _ => None,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_working_status_rejects_task_summary_prefixes() {
        assert_eq!(
            status_from_ready_working_prefix("Working tree cleanup"),
            None
        );
        assert_eq!(status_from_ready_working_prefix("Ready for review"), None);
    }

    #[test]
    fn ready_working_status_accepts_only_exact_status_forms() {
        assert_eq!(
            status_from_ready_working_prefix("Working"),
            Some(StatusKind::Busy)
        );
        assert_eq!(
            status_from_ready_working_prefix("Working…"),
            Some(StatusKind::Busy)
        );
        assert_eq!(
            status_from_ready_working_prefix("Working..."),
            Some(StatusKind::Busy)
        );
        assert_eq!(
            status_from_ready_working_prefix("Ready"),
            Some(StatusKind::Idle)
        );
    }
}
