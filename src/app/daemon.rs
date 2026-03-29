use super::*;

pub(super) fn daemon_run() -> Result<()> {
    let mut snapshot = cache::daemon_snapshot_from_tmux()?;
    cache::write_snapshot_to_cache(&snapshot)?;

    let session_target = tmux::default_session_target()?;
    let mut child = Command::new("tmux")
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
    let reader = BufReader::new(stdout);

    for line in reader.lines() {
        let line = line.context("failed to read tmux control-mode output")?;
        if let Some(pane_id) = subscription_changed_pane_id(&line) {
            refresh_snapshot_pane(&mut snapshot, pane_id)?;
            merge_cached_panes(&mut snapshot, Some(pane_id));
            cache::write_snapshot_to_cache(&snapshot)?;
        } else if let Some(pane_id) = output_title_change_pane_id(&line) {
            refresh_snapshot_pane(&mut snapshot, pane_id)?;
            merge_cached_panes(&mut snapshot, Some(pane_id));
            cache::write_snapshot_to_cache(&snapshot)?;
        } else if should_resnapshot_from_notification(&line) {
            snapshot = cache::daemon_snapshot_from_tmux()?;
            merge_cached_panes(&mut snapshot, None);
            cache::write_snapshot_to_cache(&snapshot)?;
        }

        if line.starts_with("%exit") {
            break;
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

pub(crate) fn output_title_change_pane_id(line: &str) -> Option<&str> {
    let mut fields = line.splitn(3, ' ');
    if fields.next()? != "%output" {
        return None;
    }

    let pane_id = fields.next()?;
    let payload = fields.next()?;
    if !pane_id.starts_with('%') || !contains_title_escape(payload) {
        return None;
    }

    Some(pane_id)
}

fn contains_title_escape(payload: &str) -> bool {
    payload.contains("\u{1b}]0;")
        || payload.contains("\u{1b}]2;")
        || payload.contains("\\033]0;")
        || payload.contains("\\033]2;")
}

fn refresh_snapshot_pane(snapshot: &mut SnapshotEnvelope, pane_id: &str) -> Result<()> {
    let pane = tmux::tmux_list_pane(pane_id)?.map(|row| {
        let mut pane = classify::pane_from_row(row);
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
