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
        let provider = pane
            .provider
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".to_string());

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
    println!("pane_id: {}", pane.pane_id);
    println!(
        "location: {}:{}.{} ({})",
        pane.location.session_name,
        pane.location.window_index,
        pane.location.pane_index,
        pane.location.window_name
    );
    println!("location_tag: {}", pane.location_tag());
    println!(
        "provider: {}",
        pane.provider
            .map(|provider| provider.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    );
    println!("display_label: {}", pane.display.label);
    if let Some(activity_label) = pane.display.activity_label.as_deref() {
        println!("activity_label: {activity_label}");
    }
    println!("status: {:?}", pane.status.kind);
    println!("status_source: {:?}", pane.status.source);
    println!(
        "command: {}",
        default_if_empty(&pane.tmux.pane_current_command, "<empty>")
    );
    println!(
        "title_raw: {}",
        default_if_empty(&pane.tmux.pane_title_raw, "<empty>")
    );
    println!(
        "cwd: {}",
        default_if_empty(&pane.tmux.pane_current_path, "<empty>")
    );
    println!("tty: {}", default_if_empty(&pane.tmux.pane_tty, "<empty>"));

    if pane.agent_metadata.provider.is_some()
        || pane.agent_metadata.label.is_some()
        || pane.agent_metadata.cwd.is_some()
        || pane.agent_metadata.state.is_some()
        || pane.agent_metadata.session_id.is_some()
    {
        println!("agent_metadata:");
        println!(
            "  provider: {}",
            default_if_empty(
                pane.agent_metadata.provider.as_deref().unwrap_or(""),
                "<empty>"
            )
        );
        println!(
            "  label: {}",
            default_if_empty(
                pane.agent_metadata.label.as_deref().unwrap_or(""),
                "<empty>"
            )
        );
        println!(
            "  cwd: {}",
            default_if_empty(pane.agent_metadata.cwd.as_deref().unwrap_or(""), "<empty>")
        );
        println!(
            "  state: {}",
            default_if_empty(
                pane.agent_metadata.state.as_deref().unwrap_or(""),
                "<empty>"
            )
        );
        println!(
            "  session_id: {}",
            default_if_empty(
                pane.agent_metadata.session_id.as_deref().unwrap_or(""),
                "<empty>"
            )
        );
    }

    if pane.classification.reasons.is_empty() {
        println!("classification: none");
    } else {
        println!("classification:");
        for reason in &pane.classification.reasons {
            println!("  - {reason}");
        }
    }
}

pub(super) fn print_popup_tsv(entries: &[PopupEntry]) {
    for entry in entries {
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            tsv_escape(&entry.pane_id),
            tsv_escape(
                &entry
                    .provider
                    .map(|provider| provider.to_string())
                    .unwrap_or_default()
            ),
            tsv_escape(status_kind_name(entry.status)),
            tsv_escape(&entry.session_name),
            entry.window_index,
            entry.pane_index,
            tsv_escape(&entry.display_label)
        );
    }
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

    println!("path: {}", path.display());
    println!("schema_version: {}", snapshot.schema_version);
    println!("generated_at: {}", snapshot.generated_at);
    println!("source: {:?}", snapshot.source.kind);
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
    max_age_seconds: Option<u64>,
) {
    println!("cache_valid: yes");
    println!("path: {}", path.display());
    println!("schema_version: {}", snapshot.schema_version);
    println!("generated_at: {}", snapshot.generated_at);
    println!("source: {:?}", snapshot.source.kind);
    println!("pane_count: {}", summary.pane_count);

    if let Some(max_age_seconds) = max_age_seconds {
        let age_seconds = cache::cache_age_seconds(summary.generated_at);
        println!("age_seconds: {age_seconds}");
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

pub(crate) fn tsv_escape(value: &str) -> String {
    value.replace(['\t', '\n', '\r'], " ")
}
