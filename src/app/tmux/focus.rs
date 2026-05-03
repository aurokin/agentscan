use super::*;

pub(crate) struct ResolvedFocusTarget {
    pub(crate) client_tty: Option<String>,
    pub(crate) pane_exists: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FocusTmuxPaneResult {
    Focused,
    Missing,
}

fn current_pane_id() -> Result<Option<String>> {
    if env::var_os("TMUX").is_none() {
        return Ok(None);
    }

    let Some(stdout) = run_tmux_text_output(
        &["display-message", "-p", "#{pane_id}"],
        "current tmux pane id",
        "tmux display-message for current pane id",
        |_| true,
        "current pane id output was not UTF-8",
    )?
    else {
        return Ok(None);
    };

    let pane_id = stdout.trim();
    if pane_id.is_empty() {
        Ok(None)
    } else {
        Ok(Some(pane_id.to_string()))
    }
}

pub(crate) fn resolve_tmux_target_pane(
    pane_id: Option<&str>,
    command_name: &str,
) -> Result<String> {
    match pane_id {
        Some(pane_id) if !pane_id.trim().is_empty() => Ok(pane_id.trim().to_string()),
        _ => current_pane_id()?
            .with_context(|| format!("`tmux {command_name}` requires --pane-id outside tmux")),
    }
}

fn current_client_tty() -> Result<Option<String>> {
    if env::var_os("TMUX").is_none() {
        return Ok(None);
    }

    let Some(stdout) = run_tmux_text_output(
        &["display-message", "-p", "#{client_tty}"],
        "current tmux client tty",
        "tmux display-message for current client tty",
        |_| true,
        "current client tty output was not UTF-8",
    )?
    else {
        return Ok(None);
    };

    let tty = stdout.trim();
    if tty.is_empty() {
        Ok(None)
    } else {
        Ok(Some(tty.to_string()))
    }
}

fn attached_client_tty() -> Result<Option<String>> {
    let format = format!("#{{client_tty}}{TMUX_FORMAT_DELIM}#{{client_activity}}");
    let Some(stdout) = run_tmux_text_output(
        &["list-clients", "-F", &format],
        "tmux list-clients",
        "tmux list-clients",
        |_| true,
        "tmux client output was not UTF-8",
    )?
    else {
        return Ok(None);
    };

    let clients = parse_tmux_client_rows(&stdout)?;
    Ok(select_best_client_tty(&clients))
}

pub(crate) fn select_best_client_tty(clients: &[TmuxClientRow]) -> Option<String> {
    clients
        .iter()
        .max_by_key(|client| client.client_activity)
        .map(|client| client.client_tty.clone())
}

pub(crate) fn resolve_focus_client_tty(client_tty: Option<&str>) -> Result<Option<String>> {
    match client_tty {
        Some(tty) if !tty.trim().is_empty() => Ok(Some(tty.trim().to_string())),
        _ => default_focus_client_tty(),
    }
}

fn default_focus_client_tty() -> Result<Option<String>> {
    if let Some(client_tty) = current_client_tty()? {
        return Ok(Some(client_tty));
    }

    attached_client_tty()
}

pub(crate) fn resolve_focus_target(
    pane_id: &str,
    client_tty: Option<&str>,
) -> Result<ResolvedFocusTarget> {
    Ok(ResolvedFocusTarget {
        client_tty: resolve_focus_client_tty(client_tty)?,
        pane_exists: tmux_list_pane(pane_id)?.is_some(),
    })
}

pub(crate) fn focus_tmux_pane(
    pane_id: &str,
    client_tty: Option<&str>,
) -> Result<FocusTmuxPaneResult> {
    let client_tty = resolve_focus_client_tty(client_tty)?;

    let primary = run_tmux_switch_client(pane_id, client_tty.as_deref(), true)?;
    if primary.status.success() {
        return Ok(FocusTmuxPaneResult::Focused);
    }
    if tmux_target_is_missing(&primary.stderr) {
        return Ok(FocusTmuxPaneResult::Missing);
    }

    let fallback = run_tmux_switch_client(pane_id, client_tty.as_deref(), false)?;
    if fallback.status.success() {
        Ok(FocusTmuxPaneResult::Focused)
    } else if tmux_target_is_missing(&fallback.stderr) {
        Ok(FocusTmuxPaneResult::Missing)
    } else {
        let context = if client_tty.is_some() {
            "tmux switch-client fallback with client tty"
        } else {
            "tmux switch-client fallback"
        };
        bail!(format_tmux_switch_client_error(context, &fallback));
    }
}

fn run_tmux_switch_client(
    pane_id: &str,
    client_tty: Option<&str>,
    zoom: bool,
) -> Result<std::process::Output> {
    let mut command = super::command::tmux_command();
    command.arg("switch-client");
    if zoom {
        command.arg("-Z");
    }
    if let Some(client_tty) = client_tty {
        command.args(["-c", client_tty]);
    }
    command.args(["-t", pane_id]);

    let context = match (zoom, client_tty.is_some()) {
        (true, true) => "tmux switch-client with client tty",
        (true, false) => "tmux switch-client",
        (false, true) => "tmux switch-client fallback with client tty",
        (false, false) => "tmux switch-client fallback",
    };

    command
        .output()
        .with_context(|| format!("failed to execute {context}"))
}

pub(crate) fn switch_tmux_client_to_prefix(client_tty: Option<&str>) -> Result<()> {
    let client_tty = resolve_focus_client_tty(client_tty)?;
    if let Some(client_tty) = client_tty.as_deref() {
        run_tmux_status(
            &["switch-client", "-c", client_tty, "-T", "prefix"],
            "tmux switch-client -T prefix with client tty",
            "tmux switch-client -T prefix",
        )
    } else {
        run_tmux_status(
            &["switch-client", "-T", "prefix"],
            "tmux switch-client -T prefix",
            "tmux switch-client -T prefix",
        )
    }
}

pub(crate) fn display_tmux_message(client_tty: Option<&str>, message: &str) -> Result<()> {
    let client_tty = resolve_focus_client_tty(client_tty)?;
    if let Some(client_tty) = client_tty.as_deref() {
        run_tmux_status(
            &["display-message", "-c", client_tty, message],
            "tmux display-message with client tty",
            "tmux display-message",
        )
    } else {
        run_tmux_status(
            &["display-message", message],
            "tmux display-message",
            "tmux display-message",
        )
    }
}

fn format_tmux_switch_client_error(context: &str, output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if stderr.is_empty() {
        format!("{context} failed with status {}", output.status)
    } else {
        format!("{context} failed: {stderr}")
    }
}
