use super::*;
use std::sync::mpsc;
use std::time::Duration;
use std::time::Instant;

const RECONCILE_INTERVAL: Duration = Duration::from_secs(1);

pub(super) fn daemon_run() -> Result<()> {
    let mut snapshot = cache::daemon_snapshot_from_tmux()?;
    cache::write_snapshot_to_cache(&snapshot)?;

    let session_target = tmux::default_session_target()?;
    let mut child = tmux::tmux_command()
        .args(["-C", "attach-session", "-t", &session_target])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("failed to start tmux control-mode client")?;

    let mut stdin = child
        .stdin
        .take()
        .context("tmux control-mode client did not provide stdin")?;
    writeln!(stdin, "refresh-client -B '{DAEMON_SUBSCRIPTION_FORMAT}'")
        .context("failed to subscribe to pane and metadata updates")?;
    stdin
        .flush()
        .context("failed to flush tmux control commands")?;

    let stdout = child
        .stdout
        .take()
        .context("tmux control-mode client did not provide stdout")?;
    let (line_tx, line_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            match read_control_mode_line(&mut reader) {
                Ok(Some(line)) => {
                    if line_tx.send(Ok(line)).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = line_tx.send(Err(error));
                    break;
                }
            }
        }
    });

    let mut next_reconcile_at = Instant::now() + RECONCILE_INTERVAL;

    loop {
        let now = Instant::now();
        if now >= next_reconcile_at {
            reconcile_full_snapshot(&mut snapshot)?;
            cache::write_snapshot_to_cache(&snapshot)?;
            next_reconcile_at = Instant::now() + RECONCILE_INTERVAL;
        }

        let timeout = next_reconcile_at.saturating_duration_since(Instant::now());
        match line_rx.recv_timeout(timeout) {
            Ok(line) => {
                let line = line?;
                let event = control_event_from_line(&line);
                let should_exit = event == ControlEvent::Exit;
                if apply_control_event(&mut snapshot, &line, &event)? {
                    cache::write_snapshot_to_cache(&snapshot)?;
                }

                if should_exit {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                reconcile_full_snapshot(&mut snapshot)?;
                cache::write_snapshot_to_cache(&snapshot)?;
                next_reconcile_at = Instant::now() + RECONCILE_INTERVAL;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let status = child
        .wait()
        .context("failed while waiting for tmux control-mode client to exit")?;
    if !status.success() {
        bail!("tmux control-mode client exited with status {status}");
    }

    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
enum ControlEvent {
    PaneChanged(String),
    TitleChanged { pane_id: String, title: String },
    WindowChanged(String),
    SessionChanged(String),
    Resnapshot,
    Exit,
    Ignored,
}

fn control_event_from_line(line: &str) -> ControlEvent {
    if line.starts_with("%exit") {
        return ControlEvent::Exit;
    }

    if let Some(pane_id) = subscription_changed_pane_id(line) {
        return ControlEvent::PaneChanged(pane_id.to_string());
    }

    if let Some(change) = output_title_change(line) {
        return ControlEvent::TitleChanged {
            pane_id: change.pane_id.to_string(),
            title: change.title,
        };
    }

    if let Some(window_id) = window_notification_target(line) {
        return ControlEvent::WindowChanged(window_id.to_string());
    }

    if let Some(session_id) = session_notification_target(line) {
        return ControlEvent::SessionChanged(session_id.to_string());
    }

    if should_resnapshot_from_notification(line) {
        return ControlEvent::Resnapshot;
    }

    ControlEvent::Ignored
}

fn apply_control_event(
    snapshot: &mut SnapshotEnvelope,
    line: &str,
    event: &ControlEvent,
) -> Result<bool> {
    match event {
        ControlEvent::PaneChanged(pane_id) => {
            refresh_snapshot_pane(snapshot, pane_id)?;
            merge_cached_panes(snapshot, Some(pane_id));
            Ok(true)
        }
        ControlEvent::TitleChanged { pane_id, title } => {
            refresh_snapshot_pane_with_title(snapshot, pane_id, Some(title.as_str()))?;
            merge_cached_panes(snapshot, Some(pane_id));
            Ok(true)
        }
        ControlEvent::WindowChanged(window_id) => {
            refresh_snapshot_window(snapshot, window_id)
                .or_else(|error| fallback_to_full_resnapshot(snapshot, line, error))?;
            Ok(true)
        }
        ControlEvent::SessionChanged(session_id) => {
            refresh_snapshot_session(snapshot, session_id)
                .or_else(|error| fallback_to_full_resnapshot(snapshot, line, error))?;
            Ok(true)
        }
        ControlEvent::Resnapshot => {
            reconcile_full_snapshot(snapshot)?;
            Ok(true)
        }
        ControlEvent::Exit | ControlEvent::Ignored => Ok(false),
    }
}

pub(crate) fn read_control_mode_line(reader: &mut impl BufRead) -> Result<Option<String>> {
    let mut bytes = Vec::new();
    let bytes_read = reader
        .read_until(b'\n', &mut bytes)
        .context("failed to read tmux control-mode output")?;
    if bytes_read == 0 {
        return Ok(None);
    }

    if bytes.ends_with(b"\n") {
        bytes.pop();
    }
    if bytes.ends_with(b"\r") {
        bytes.pop();
    }

    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

pub(crate) fn should_resnapshot_from_notification(line: &str) -> bool {
    matches!(
        notification_name(line),
        Some(
            "%sessions-changed"
                | "%session-changed"
                | "%session-renamed"
                | "%session-window-changed"
                | "%layout-change"
                | "%window-add"
                | "%window-close"
                | "%unlinked-window-close"
                | "%window-pane-changed"
                | "%window-renamed"
        )
    )
}

pub(crate) fn subscription_changed_pane_id(line: &str) -> Option<&str> {
    let mut fields = line.split_whitespace();
    if fields.next()? != "%subscription-changed" {
        return None;
    }
    let _subscription_name = fields.next()?;
    let _session = fields.next()?;
    let _window = fields.next()?;
    let _flags = fields.next()?;
    let pane_id = fields.next()?;
    pane_id.starts_with('%').then_some(pane_id)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn output_title_change_pane_id(line: &str) -> Option<&str> {
    output_title_change(line).map(|change| change.pane_id)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn output_title_change_title(line: &str) -> Option<String> {
    output_title_change(line).map(|change| change.title)
}

struct OutputTitleChange<'a> {
    pane_id: &'a str,
    title: String,
}

fn output_title_change(line: &str) -> Option<OutputTitleChange<'_>> {
    let mut fields = line.splitn(3, ' ');
    if fields.next()? != "%output" {
        return None;
    }

    let pane_id = fields.next()?;
    let payload = fields.next()?;
    let title = terminal_title_from_control_payload(payload)?;
    if !pane_id.starts_with('%') {
        return None;
    }

    Some(OutputTitleChange { pane_id, title })
}

fn terminal_title_from_control_payload(payload: &str) -> Option<String> {
    let decoded = decode_tmux_control_payload(payload);
    terminal_title_from_decoded_output(&decoded)
}

fn decode_tmux_control_payload(payload: &str) -> String {
    let bytes = payload.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'\\'
            && index + 3 < bytes.len()
            && is_octal_digit(bytes[index + 1])
            && is_octal_digit(bytes[index + 2])
            && is_octal_digit(bytes[index + 3])
        {
            let value = ((bytes[index + 1] - b'0') << 6)
                | ((bytes[index + 2] - b'0') << 3)
                | (bytes[index + 3] - b'0');
            decoded.push(value);
            index += 4;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }

    String::from_utf8_lossy(&decoded).into_owned()
}

const fn is_octal_digit(byte: u8) -> bool {
    byte >= b'0' && byte <= b'7'
}

fn terminal_title_from_decoded_output(output: &str) -> Option<String> {
    let bytes = output.as_bytes();
    let mut index = 0;
    let mut title = None;

    while index + 4 <= bytes.len() {
        if bytes[index] == 0x1b
            && bytes[index + 1] == b']'
            && matches!(bytes[index + 2], b'0' | b'2')
            && bytes[index + 3] == b';'
        {
            let title_start = index + 4;
            let mut title_end = title_start;
            while title_end < bytes.len() {
                if bytes[title_end] == 0x07 {
                    title =
                        Some(String::from_utf8_lossy(&bytes[title_start..title_end]).into_owned());
                    index = title_end + 1;
                    break;
                }
                if title_end + 1 < bytes.len()
                    && bytes[title_end] == 0x1b
                    && bytes[title_end + 1] == b'\\'
                {
                    title =
                        Some(String::from_utf8_lossy(&bytes[title_start..title_end]).into_owned());
                    index = title_end + 2;
                    break;
                }
                title_end += 1;
            }

            if title_end == bytes.len() {
                break;
            }
        } else {
            index += 1;
        }
    }

    title
}

fn refresh_snapshot_pane(snapshot: &mut SnapshotEnvelope, pane_id: &str) -> Result<()> {
    refresh_snapshot_pane_with_title(snapshot, pane_id, None)
}

fn refresh_snapshot_pane_with_title(
    snapshot: &mut SnapshotEnvelope,
    pane_id: &str,
    title_override: Option<&str>,
) -> Result<()> {
    let pane = tmux::tmux_list_pane(pane_id)?.map(|mut row| {
        if let Some(title) = title_override {
            row.pane_title_raw = title.to_string();
        }
        let mut pane = classify::pane_from_row(row);
        let proc_inspector = proc::ProcProcessInspector;
        classify::apply_proc_fallback(&mut pane, &proc_inspector);
        scanner::apply_pane_output_status_fallbacks(std::slice::from_mut(&mut pane));
        pane.diagnostics.cache_origin = "daemon_update".to_string();
        pane
    });

    if let Some(index) = snapshot
        .panes
        .iter()
        .position(|existing| existing.pane_id == pane_id)
    {
        if let Some(pane) = pane {
            snapshot.panes[index] = pane;
        } else {
            snapshot.panes.remove(index);
        }
    } else if let Some(pane) = pane {
        snapshot.panes.push(pane);
    }

    cache::sort_snapshot_panes(snapshot);
    cache::mark_snapshot_as_daemon(snapshot)
}

fn refresh_snapshot_window(snapshot: &mut SnapshotEnvelope, window_id: &str) -> Result<()> {
    refresh_snapshot_scope(snapshot, TargetScope::Window, window_id)
}

fn refresh_snapshot_session(snapshot: &mut SnapshotEnvelope, session_id: &str) -> Result<()> {
    refresh_snapshot_scope(snapshot, TargetScope::Session, session_id)
}

fn refresh_snapshot_scope(
    snapshot: &mut SnapshotEnvelope,
    scope: TargetScope,
    target_id: &str,
) -> Result<()> {
    let rows = tmux::tmux_list_panes_target(target_id)?;

    snapshot
        .panes
        .retain(|pane| !scope.matches(pane, target_id));

    if let Some(rows) = rows {
        let proc_inspector = proc::ProcProcessInspector;
        let mut panes = classify::panes_from_rows_with_proc_fallback(rows, &proc_inspector);
        scanner::apply_pane_output_status_fallbacks(&mut panes);
        snapshot.panes.extend(panes.into_iter().map(|mut pane| {
            pane.diagnostics.cache_origin = "daemon_update".to_string();
            pane
        }));
    }

    merge_cached_panes(snapshot, None);
    cache::sort_snapshot_panes(snapshot);
    cache::mark_snapshot_as_daemon(snapshot)
}

fn fallback_to_full_resnapshot(
    snapshot: &mut SnapshotEnvelope,
    line: &str,
    error: anyhow::Error,
) -> Result<()> {
    eprintln!(
        "agentscan: targeted refresh failed for control-mode line {:?}: {error:#}",
        line
    );
    reconcile_full_snapshot(snapshot)
}

fn reconcile_full_snapshot(snapshot: &mut SnapshotEnvelope) -> Result<()> {
    *snapshot = cache::daemon_snapshot_from_tmux()?;
    merge_cached_panes(snapshot, None);
    Ok(())
}

fn merge_cached_panes(snapshot: &mut SnapshotEnvelope, excluded_pane_id: Option<&str>) {
    let Some(existing) = cache::read_existing_snapshot_if_valid() else {
        return;
    };

    for pane in &mut snapshot.panes {
        if excluded_pane_id.is_some_and(|pane_id| pane.pane_id == pane_id) {
            continue;
        }

        if let Some(existing_pane) = existing
            .panes
            .iter()
            .find(|cached| cached.pane_id == pane.pane_id)
            && has_more_recent_helper_state(existing_pane, pane)
        {
            *pane = existing_pane.clone();
        }
    }
}

pub(crate) fn window_notification_target(line: &str) -> Option<&str> {
    match notification_name(line) {
        Some(
            "%layout-change"
            | "%window-add"
            | "%window-close"
            | "%unlinked-window-close"
            | "%unlinked-window-renamed"
            | "%window-pane-changed"
            | "%window-renamed",
        ) => line
            .split_whitespace()
            .nth(1)
            .filter(|value| value.starts_with('@')),
        _ => None,
    }
}

pub(crate) fn session_notification_target(line: &str) -> Option<&str> {
    match notification_name(line) {
        Some("%session-renamed") => line
            .split_whitespace()
            .nth(1)
            .filter(|value| value.starts_with('$')),
        _ => None,
    }
}

fn has_more_recent_helper_state(existing: &PaneRecord, current: &PaneRecord) -> bool {
    existing.agent_metadata.provider != current.agent_metadata.provider
        || existing.agent_metadata.label != current.agent_metadata.label
        || existing.agent_metadata.cwd != current.agent_metadata.cwd
        || existing.agent_metadata.state != current.agent_metadata.state
        || existing.agent_metadata.session_id != current.agent_metadata.session_id
}

pub(crate) fn notification_name(line: &str) -> Option<&str> {
    line.split_whitespace()
        .next()
        .filter(|token| token.starts_with('%'))
}

#[derive(Clone, Copy)]
enum TargetScope {
    Window,
    Session,
}

impl TargetScope {
    fn matches(self, pane: &PaneRecord, target_id: &str) -> bool {
        match self {
            Self::Window => pane.tmux.window_id.as_deref() == Some(target_id),
            Self::Session => pane.tmux.session_id.as_deref() == Some(target_id),
        }
    }
}
