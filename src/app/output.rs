use super::*;

pub(super) fn emit_snapshot(snapshot: &SnapshotEnvelope, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Text => {
            print_list_text(&snapshot.panes);
            Ok(())
        }
        OutputFormat::Json => print_json(snapshot),
    }
}

fn print_list_text(panes: &[PaneRecord]) {
    if panes.is_empty() {
        println!("No matching tmux panes.");
        return;
    }

    for pane in panes {
        let provider = provider_display_marker(pane.provider);

        println!(
            "{} {}:{}.{} - {}",
            provider,
            pane.location.session_name,
            pane.location.window_index,
            pane.location.pane_index,
            pane.display_label()
        );
    }
}

pub(super) fn print_inspect_text(pane: &PaneRecord) {
    print!("{}", inspect_text(pane));
}

pub(super) fn inspect_text(pane: &PaneRecord) -> String {
    let mut lines = vec![
        format!("pane_id: {}", pane.pane_id),
        format!(
            "location: {}:{}.{} ({})",
            pane.location.session_name,
            pane.location.window_index,
            pane.location.pane_index,
            pane.location.window_name
        ),
        format!("location_tag: {}", pane.location_tag()),
        format!(
            "provider: {}",
            pane.provider
                .map(|provider| provider.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        ),
        format!(
            "provider_source: {}",
            pane.classification
                .matched_by
                .map(classification_match_kind_name)
                .unwrap_or("none")
        ),
        format!(
            "provider_confidence: {}",
            pane.classification
                .confidence
                .map(classification_confidence_name)
                .unwrap_or("none")
        ),
        format!("display_label: {}", pane.display.label),
    ];
    if let Some(activity_label) = pane.display.activity_label.as_deref() {
        lines.push(format!("activity_label: {activity_label}"));
    }
    lines.extend([
        format!("status: {}", status_kind_name(pane.status.kind)),
        format!("status_source: {}", status_source_name(pane.status.source)),
        format!(
            "command: {}",
            default_if_empty(&pane.tmux.pane_current_command, "<empty>")
        ),
        format!(
            "title_raw: {}",
            default_if_empty(&pane.tmux.pane_title_raw, "<empty>")
        ),
        format!(
            "cwd: {}",
            default_if_empty(&pane.tmux.pane_current_path, "<empty>")
        ),
        format!("tty: {}", default_if_empty(&pane.tmux.pane_tty, "<empty>")),
    ]);
    if pane.tmux.session_id.is_some() || pane.tmux.window_id.is_some() {
        lines.push(format!(
            "tmux_ids: session={} window={}",
            default_if_empty(pane.tmux.session_id.as_deref().unwrap_or(""), "<empty>"),
            default_if_empty(pane.tmux.window_id.as_deref().unwrap_or(""), "<empty>")
        ));
    }

    if pane.agent_metadata.provider.is_some()
        || pane.agent_metadata.label.is_some()
        || pane.agent_metadata.cwd.is_some()
        || pane.agent_metadata.state.is_some()
        || pane.agent_metadata.session_id.is_some()
    {
        lines.extend([
            "agent_metadata:".to_string(),
            format!(
                "  provider: {}",
                default_if_empty(
                    pane.agent_metadata.provider.as_deref().unwrap_or(""),
                    "<empty>"
                )
            ),
            format!(
                "  label: {}",
                default_if_empty(
                    pane.agent_metadata.label.as_deref().unwrap_or(""),
                    "<empty>"
                )
            ),
            format!(
                "  cwd: {}",
                default_if_empty(pane.agent_metadata.cwd.as_deref().unwrap_or(""), "<empty>")
            ),
            format!(
                "  state: {}",
                default_if_empty(
                    pane.agent_metadata.state.as_deref().unwrap_or(""),
                    "<empty>"
                )
            ),
            format!(
                "  session_id: {}",
                default_if_empty(
                    pane.agent_metadata.session_id.as_deref().unwrap_or(""),
                    "<empty>"
                )
            ),
        ]);
    }

    if pane.classification.reasons.is_empty() {
        lines.push("classification: none".to_string());
    } else {
        lines.push("classification:".to_string());
        for reason in &pane.classification.reasons {
            lines.push(format!("  - {reason}"));
        }
    }

    lines.extend([
        "proc_fallback:".to_string(),
        format!(
            "  outcome: {}",
            proc_fallback_outcome_name(pane.diagnostics.proc_fallback.outcome)
        ),
        format!("  reason: {}", pane.diagnostics.proc_fallback.reason),
    ]);
    if !pane.diagnostics.proc_fallback.commands.is_empty() {
        lines.push("  commands:".to_string());
        for command in &pane.diagnostics.proc_fallback.commands {
            lines.push(format!("    - {command}"));
        }
    }

    let mut text = lines.join("\n");
    text.push('\n');
    text
}

pub(super) fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).context("failed to serialize JSON output")?
    );
    Ok(())
}

pub(super) fn print_cache_summary_text(snapshot: &SnapshotEnvelope) -> Result<()> {
    let path = cache::cache_path()?;
    let summary = cache::summarize_snapshot(snapshot)?;
    let diagnostics = cache::cache_diagnostics(snapshot, None)?;

    print_cache_snapshot_fields(&path, snapshot, &diagnostics, true);
    print_daemon_cache_diagnostics(&diagnostics);
    println!(
        "tmux_version: {}",
        snapshot
            .source
            .tmux_version
            .as_deref()
            .unwrap_or("<unknown>")
    );
    println!("pane_count: {}", summary.pane_count);
    println!("agent_pane_count: {}", summary.agent_pane_count);
    println!(
        "providers: {}",
        format_provider_counts(&summary.provider_counts)
    );
    println!("statuses: {}", format_status_counts(&summary.status_counts));

    Ok(())
}

pub(super) fn print_cache_validate_text(
    path: &Path,
    snapshot: &SnapshotEnvelope,
    summary: &CacheSummary,
    diagnostics: &CacheDiagnostics,
    max_age_seconds: Option<u64>,
) {
    println!("cache_valid: yes");
    print_cache_snapshot_fields(path, snapshot, diagnostics, true);
    print_daemon_cache_diagnostics(diagnostics);
    println!("pane_count: {}", summary.pane_count);
    print_max_age_seconds(max_age_seconds);
}

pub(super) fn print_daemon_status_text(
    path: &Path,
    snapshot: &SnapshotEnvelope,
    summary: &CacheSummary,
    diagnostics: &CacheDiagnostics,
    max_age_seconds: Option<u64>,
) {
    print_daemon_cache_diagnostics(diagnostics);
    print_cache_file_fields(path, snapshot, diagnostics, false);
    print_daemon_refresh_fields(snapshot, diagnostics);
    println!("source: {:?}", snapshot.source.kind);
    println!("pane_count: {}", summary.pane_count);
    print_max_age_seconds(max_age_seconds);
}

fn print_cache_snapshot_fields(
    path: &Path,
    snapshot: &SnapshotEnvelope,
    diagnostics: &CacheDiagnostics,
    include_schema: bool,
) {
    print_cache_file_fields(path, snapshot, diagnostics, include_schema);
    println!("source: {:?}", snapshot.source.kind);
    print_daemon_refresh_fields(snapshot, diagnostics);
}

fn print_cache_file_fields(
    path: &Path,
    snapshot: &SnapshotEnvelope,
    diagnostics: &CacheDiagnostics,
    include_schema: bool,
) {
    println!("path: {}", path.display());
    if include_schema {
        println!("schema_version: {}", snapshot.schema_version);
    }
    println!("generated_at: {}", snapshot.generated_at);
    println!("cache_age_seconds: {}", diagnostics.cache_age_seconds);
}

fn print_daemon_refresh_fields(snapshot: &SnapshotEnvelope, diagnostics: &CacheDiagnostics) {
    println!(
        "daemon_generated_at: {}",
        snapshot
            .source
            .daemon_generated_at
            .as_deref()
            .unwrap_or("<none>")
    );
    if let Some(daemon_age_seconds) = diagnostics.daemon_age_seconds {
        println!("daemon_age_seconds: {daemon_age_seconds}");
    }
}

fn print_daemon_cache_diagnostics(diagnostics: &CacheDiagnostics) {
    println!(
        "daemon_cache_status: {}",
        daemon_cache_status_name(diagnostics.daemon_cache_status)
    );
    println!("daemon_cache_reason: {}", diagnostics.daemon_status_reason);
}

fn print_max_age_seconds(max_age_seconds: Option<u64>) {
    if let Some(max_age_seconds) = max_age_seconds {
        println!("max_age_seconds: {max_age_seconds}");
    }
}

fn format_provider_counts(counts: &[(Provider, usize)]) -> String {
    if counts.is_empty() {
        return "none".to_string();
    }

    counts
        .iter()
        .map(|(provider, count)| format!("{provider}={count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_status_counts(counts: &[(StatusKind, usize)]) -> String {
    if counts.is_empty() {
        return "none".to_string();
    }

    counts
        .iter()
        .map(|(status, count)| format!("{}={count}", status_kind_name(*status)))
        .collect::<Vec<_>>()
        .join(", ")
}
