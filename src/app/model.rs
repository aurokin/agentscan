use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::Provider;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourceKind {
    Snapshot,
    Daemon,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StatusKind {
    Idle,
    Busy,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StatusSource {
    PaneMetadata,
    TmuxTitle,
    PaneOutput,
    NotChecked,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClassificationConfidence {
    High,
    Medium,
    Low,
}

#[allow(clippy::enum_variant_names)]
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClassificationMatchKind {
    PaneMetadata,
    PaneCurrentCommand,
    PaneTitle,
    ProcProcessTree,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct SnapshotEnvelope {
    pub(crate) schema_version: u32,
    pub(crate) generated_at: String,
    pub(crate) source: SnapshotSource,
    pub(crate) panes: Vec<PaneRecord>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct SnapshotSource {
    pub(crate) kind: SourceKind,
    pub(crate) tmux_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) daemon_generated_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct PaneRecord {
    pub(crate) pane_id: String,
    pub(crate) location: PaneLocation,
    pub(crate) tmux: TmuxPaneMetadata,
    pub(crate) display: DisplayMetadata,
    pub(crate) provider: Option<Provider>,
    pub(crate) status: PaneStatus,
    pub(crate) classification: PaneClassification,
    pub(crate) agent_metadata: AgentMetadata,
    pub(crate) diagnostics: PaneDiagnostics,
}

impl PaneRecord {
    pub(crate) fn display_label(&self) -> &str {
        &self.display.label
    }

    pub(crate) fn location_tag(&self) -> String {
        self.location.tag()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct PaneLocation {
    pub(crate) session_name: String,
    pub(crate) window_index: u32,
    pub(crate) pane_index: u32,
    pub(crate) window_name: String,
}

impl PaneLocation {
    pub(crate) fn tag(&self) -> String {
        format!(
            "{}:{}.{}",
            self.session_name, self.window_index, self.pane_index
        )
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct TmuxPaneMetadata {
    pub(crate) pane_pid: u32,
    pub(crate) pane_tty: String,
    pub(crate) pane_current_path: String,
    pub(crate) pane_current_command: String,
    pub(crate) pane_title_raw: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) window_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct DisplayMetadata {
    pub(crate) label: String,
    pub(crate) activity_label: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct PaneStatus {
    pub(crate) kind: StatusKind,
    pub(crate) source: StatusSource,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct PaneClassification {
    pub(crate) matched_by: Option<ClassificationMatchKind>,
    pub(crate) confidence: Option<ClassificationConfidence>,
    pub(crate) reasons: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProcFallbackOutcome {
    NotRun,
    Skipped,
    NoMatch,
    Error,
    Resolved,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ProcFallbackDiagnostics {
    pub(crate) outcome: ProcFallbackOutcome,
    pub(crate) reason: String,
    pub(crate) commands: Vec<String>,
}

impl Default for ProcFallbackDiagnostics {
    fn default() -> Self {
        Self {
            outcome: ProcFallbackOutcome::NotRun,
            reason: "proc fallback was not evaluated".to_string(),
            commands: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct AgentMetadata {
    pub(crate) provider: Option<String>,
    pub(crate) label: Option<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) state: Option<String>,
    pub(crate) session_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct PaneDiagnostics {
    pub(crate) cache_origin: String,
    #[serde(default)]
    pub(crate) proc_fallback: ProcFallbackDiagnostics,
}

#[derive(Debug)]
pub(crate) struct CacheSummary {
    pub(crate) generated_at: OffsetDateTime,
    pub(crate) pane_count: usize,
    pub(crate) agent_pane_count: usize,
    pub(crate) provider_counts: Vec<(Provider, usize)>,
    pub(crate) status_counts: Vec<(StatusKind, usize)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DaemonCacheStatus {
    Healthy,
    Stale,
    SnapshotOnly,
    Unavailable,
}

#[derive(Debug)]
pub(crate) struct CacheDiagnostics {
    pub(crate) cache_age_seconds: u64,
    pub(crate) daemon_age_seconds: Option<u64>,
    pub(crate) daemon_cache_status: DaemonCacheStatus,
    pub(crate) daemon_status_reason: String,
}

#[derive(Clone, Debug)]
pub(crate) struct TmuxPaneRow {
    pub(crate) session_name: String,
    pub(crate) window_index: u32,
    pub(crate) pane_index: u32,
    pub(crate) pane_id: String,
    pub(crate) pane_pid: u32,
    pub(crate) pane_current_command: String,
    pub(crate) pane_title_raw: String,
    pub(crate) pane_tty: String,
    pub(crate) pane_current_path: String,
    pub(crate) window_name: String,
    pub(crate) session_id: Option<String>,
    pub(crate) window_id: Option<String>,
    pub(crate) agent_provider: Option<String>,
    pub(crate) agent_label: Option<String>,
    pub(crate) agent_cwd: Option<String>,
    pub(crate) agent_state: Option<String>,
    pub(crate) agent_session_id: Option<String>,
}

#[derive(Debug)]
pub(crate) struct ProviderMatch {
    pub(crate) provider: Provider,
    pub(crate) matched_by: ClassificationMatchKind,
    pub(crate) confidence: ClassificationConfidence,
    pub(crate) reasons: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct TmuxClientRow {
    pub(crate) client_tty: String,
    pub(crate) client_activity: i64,
}

pub(crate) fn status_kind_name(status: StatusKind) -> &'static str {
    match status {
        StatusKind::Idle => "idle",
        StatusKind::Busy => "busy",
        StatusKind::Unknown => "unknown",
    }
}

pub(crate) fn status_source_name(source: StatusSource) -> &'static str {
    match source {
        StatusSource::PaneMetadata => "pane_metadata",
        StatusSource::TmuxTitle => "tmux_title",
        StatusSource::PaneOutput => "pane_output",
        StatusSource::NotChecked => "not_checked",
    }
}

pub(crate) fn classification_match_kind_name(kind: ClassificationMatchKind) -> &'static str {
    match kind {
        ClassificationMatchKind::PaneMetadata => "pane_metadata",
        ClassificationMatchKind::PaneCurrentCommand => "pane_current_command",
        ClassificationMatchKind::PaneTitle => "pane_title",
        ClassificationMatchKind::ProcProcessTree => "proc_process_tree",
    }
}

pub(crate) fn classification_confidence_name(confidence: ClassificationConfidence) -> &'static str {
    match confidence {
        ClassificationConfidence::High => "high",
        ClassificationConfidence::Medium => "medium",
        ClassificationConfidence::Low => "low",
    }
}

pub(crate) fn proc_fallback_outcome_name(outcome: ProcFallbackOutcome) -> &'static str {
    match outcome {
        ProcFallbackOutcome::NotRun => "not_run",
        ProcFallbackOutcome::Skipped => "skipped",
        ProcFallbackOutcome::NoMatch => "no_match",
        ProcFallbackOutcome::Error => "error",
        ProcFallbackOutcome::Resolved => "resolved",
    }
}

pub(crate) fn daemon_cache_status_name(status: DaemonCacheStatus) -> &'static str {
    match status {
        DaemonCacheStatus::Healthy => "healthy",
        DaemonCacheStatus::Stale => "stale",
        DaemonCacheStatus::SnapshotOnly => "snapshot_only",
        DaemonCacheStatus::Unavailable => "unavailable",
    }
}
