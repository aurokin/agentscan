use super::*;

/// Write one-shot command output to stdout, surfacing the I/O error (notably
/// `BrokenPipe`) instead of panicking the way `println!`/`print!` do. The CLI's
/// top-level broken-pipe handling in `commands::run` turns that into a clean exit,
/// so piping into `head` or quitting a pager early no longer aborts with exit 101
/// and a backtrace. This mirrors the subscribe stream's discipline (see
/// `lifecycle::write_subscription_event_json_line`) for the one-shot output paths.
pub(super) fn write_stdout(text: &str) -> Result<()> {
    write_text(std::io::stdout().lock(), text)
}

/// Writer-generic core of [`write_stdout`], split out so the broken-pipe path is
/// testable without a real closed pipe.
pub(super) fn write_text(mut writer: impl Write, text: &str) -> Result<()> {
    writer.write_all(text.as_bytes())?;
    writer.flush()?;
    Ok(())
}

pub(super) fn emit_snapshot(
    snapshot: &SnapshotEnvelope,
    format: OutputFormat,
    icon_mode: IconMode,
) -> Result<()> {
    match format {
        OutputFormat::Text => print_list_text(&snapshot.panes, icon_mode),
        OutputFormat::Json => print_json(snapshot),
    }
}

/// Versioned envelope for the `providers --format json` output. Wraps the
/// provider array the way [`SnapshotEnvelope`] wraps `panes`, so a field change
/// is a versioned break instead of a silent one for machine consumers.
#[derive(Serialize)]
struct ProvidersEnvelope<'a> {
    schema_version: u32,
    providers: &'a [ProviderSummary],
}

/// Versioned envelope for the `hotkeys --format json` output. The desktop shell
/// consumes these rows, so the envelope makes any row-shape change explicit.
#[derive(Serialize)]
pub(super) struct PickerRowsEnvelope<'a> {
    pub(super) schema_version: u32,
    pub(super) rows: &'a [picker::PickerRow],
}

pub(super) fn emit_providers(
    providers: &[ProviderSummary],
    format: OutputFormat,
    icon_mode: IconMode,
) -> Result<()> {
    match format {
        OutputFormat::Text => print_providers_text(providers, icon_mode),
        OutputFormat::Json => print_json(&ProvidersEnvelope {
            schema_version: PROVIDERS_SCHEMA_VERSION,
            providers,
        }),
    }
}

pub(super) fn emit_picker_rows(rows: &[picker::PickerRow], format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Text => print_picker_rows_text(rows),
        OutputFormat::Json => print_json(&PickerRowsEnvelope {
            schema_version: PICKER_ROWS_SCHEMA_VERSION,
            rows,
        }),
    }
}

fn print_list_text(panes: &[PaneRecord], icon_mode: IconMode) -> Result<()> {
    if panes.is_empty() {
        return write_stdout("No matching tmux panes.\n");
    }

    let mut lines = Vec::with_capacity(panes.len());
    for pane in panes {
        let provider = provider_display_marker(pane.provider, icon_mode);

        lines.push(format!(
            "{} {}:{}.{} - {}",
            provider,
            pane.location.session_name,
            pane.location.window_index,
            pane.location.pane_index,
            pane.display_label()
        ));
    }

    let mut text = lines.join("\n");
    text.push('\n');
    write_stdout(&text)
}

fn print_picker_rows_text(rows: &[picker::PickerRow]) -> Result<()> {
    if rows.is_empty() {
        return write_stdout("No matching tmux panes.\n");
    }

    let mut lines = Vec::with_capacity(rows.len());
    for row in rows {
        let provider = row
            .provider
            .map(|provider| provider.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        lines.push(format!(
            "[{}] {} {} {} - {}",
            row.key,
            row.status.kind.as_str(),
            provider,
            picker_row_location_text(row),
            row.display_label
        ));
    }

    let mut text = lines.join("\n");
    text.push('\n');
    write_stdout(&text)
}

fn picker_row_location_text(row: &picker::PickerRow) -> String {
    if row.workspace.source == picker::PickerWorkspaceSource::Session
        || row.workspace.label == row.location.session_name
    {
        row.location_tag.clone()
    } else {
        format!("{} {}", row.workspace.label, row.location_tag)
    }
}

fn print_providers_text(providers: &[ProviderSummary], icon_mode: IconMode) -> Result<()> {
    let mut lines = vec![format!("icons: {icon_mode}")];
    for provider in providers {
        let marker = provider_marker(provider.provider, icon_mode);
        let codepoints = marker_codepoints(marker).join(" ");
        lines.push(format!("{} {} ({codepoints})", marker, provider.name));
    }

    let mut text = lines.join("\n");
    text.push('\n');
    write_stdout(&text)
}

pub(super) fn print_inspect_text(pane: &PaneRecord) -> Result<()> {
    write_stdout(&inspect_text(pane))
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
                .map(ClassificationMatchKind::as_str)
                .unwrap_or("none")
        ),
        format!(
            "provider_confidence: {}",
            pane.classification
                .confidence
                .map(ClassificationConfidence::as_str)
                .unwrap_or("none")
        ),
        format!("display_label: {}", pane.display.label),
    ];
    if let Some(activity_label) = pane.display.activity_label.as_deref() {
        lines.push(format!("activity_label: {activity_label}"));
    }
    lines.extend([
        format!("status: {}", pane.status.kind.as_str()),
        format!("status_source: {}", pane.status.source.as_str()),
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
            pane.diagnostics.proc_fallback.outcome.as_str()
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

pub(super) fn print_json<T: Serialize + ?Sized>(value: &T) -> Result<()> {
    let mut text =
        serde_json::to_string_pretty(value).context("failed to serialize JSON output")?;
    text.push('\n');
    write_stdout(&text)
}

pub(super) fn print_snapshot_summary_text(snapshot: &SnapshotEnvelope) -> Result<()> {
    let summary = snapshot::summarize_snapshot(snapshot)?;

    let lines = [
        format!("schema_version: {}", snapshot.schema_version),
        format!("generated_at: {}", snapshot.generated_at),
        format!("source: {:?}", snapshot.source.kind),
        format!(
            "tmux_version: {}",
            snapshot
                .source
                .tmux_version
                .as_deref()
                .unwrap_or("<unknown>")
        ),
        format!("pane_count: {}", summary.pane_count),
        format!("agent_pane_count: {}", summary.agent_pane_count),
        format!(
            "providers: {}",
            format_provider_counts(&summary.provider_counts)
        ),
        format!("statuses: {}", format_status_counts(&summary.status_counts)),
    ];

    let mut text = lines.join("\n");
    text.push('\n');
    write_stdout(&text)
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
        .map(|(status, count)| format!("{}={count}", status.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}
