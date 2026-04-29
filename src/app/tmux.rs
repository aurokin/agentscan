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

pub(crate) fn tmux_list_panes() -> Result<Vec<TmuxPaneRow>> {
    let output = run_tmux_output(&["list-panes", "-a", "-F", PANE_FORMAT], "tmux")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        if stderr.is_empty() {
            bail!("tmux list-panes failed with status {}", output.status);
        }
        bail!("tmux list-panes failed: {stderr}");
    }

    let stdout = String::from_utf8(output.stdout).context("tmux output was not valid UTF-8")?;
    parse_pane_rows(&stdout)
}

pub(crate) fn tmux_list_panes_target(target: &str) -> Result<Option<Vec<TmuxPaneRow>>> {
    let output = run_tmux_output(
        &["list-panes", "-t", target, "-F", PANE_FORMAT],
        &format!("tmux list-panes for target {target}"),
    )?;

    if !output.status.success() {
        let stderr = tmux_stderr(&output);
        if stderr.contains("can't find window")
            || stderr.contains("can't find session")
            || stderr.contains("can't find pane")
        {
            return Ok(None);
        }
        if stderr.is_empty() {
            bail!(
                "tmux list-panes -t {target} failed with status {}",
                output.status
            );
        }
        bail!("tmux list-panes -t {target} failed: {stderr}");
    }

    let stdout = String::from_utf8(output.stdout).context("tmux output was not valid UTF-8")?;
    let rows = parse_pane_rows(&stdout)?;
    Ok(Some(rows))
}

pub(crate) fn tmux_list_pane(pane_id: &str) -> Result<Option<TmuxPaneRow>> {
    let output = run_tmux_output(
        &["list-panes", "-t", pane_id, "-F", PANE_FORMAT],
        &format!("tmux list-panes for {pane_id}"),
    )?;

    if !output.status.success() {
        let stderr = tmux_stderr(&output);
        if stderr.contains("can't find pane") || stderr.contains("can't find window") {
            return Ok(None);
        }
        if stderr.is_empty() {
            bail!(
                "tmux list-panes -t {pane_id} failed with status {}",
                output.status
            );
        }
        bail!("tmux list-panes -t {pane_id} failed: {stderr}");
    }

    let stdout = String::from_utf8(output.stdout).context("tmux output was not valid UTF-8")?;
    let mut rows = parse_pane_rows(&stdout)?;
    Ok(rows.pop())
}

pub(crate) fn tmux_capture_pane_tail(pane_id: &str, line_count: usize) -> Result<Option<String>> {
    let start = format!("-{}", line_count.max(1));
    let output = run_tmux_output(
        &["capture-pane", "-t", pane_id, "-p", "-S", &start],
        &format!("tmux capture-pane for {pane_id}"),
    )?;

    if !output.status.success() {
        let stderr = tmux_stderr(&output);
        if stderr.contains("can't find pane") || stderr.contains("can't find window") {
            return Ok(None);
        }
        if stderr.is_empty() {
            bail!(
                "tmux capture-pane -t {pane_id} failed with status {}",
                output.status
            );
        }
        bail!("tmux capture-pane -t {pane_id} failed: {stderr}");
    }

    String::from_utf8(output.stdout)
        .map(Some)
        .context("tmux capture-pane output was not valid UTF-8")
}

fn run_tmux_output(args: &[&str], context: &str) -> Result<std::process::Output> {
    Command::new("tmux")
        .args(args)
        .output()
        .with_context(|| format!("failed to execute {context}"))
}

fn tmux_stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_string()
}

pub(crate) fn parse_pane_rows(input: &str) -> Result<Vec<TmuxPaneRow>> {
    let mut panes = Vec::new();

    for (line_number, line) in input.lines().enumerate() {
        if line.is_empty() {
            continue;
        }

        let fields = split_tmux_fields(line);
        if fields.len() != 10 && fields.len() != 12 && fields.len() != 15 && fields.len() != 17 {
            bail!(
                "unexpected tmux pane field count on line {}: expected 10, 12, 15, or 17, got {}",
                line_number + 1,
                fields.len()
            );
        }

        let (session_id, window_id, agent_fields_start) = match fields.len() {
            12 => (empty_to_none(fields[10]), empty_to_none(fields[11]), None),
            17 => (
                empty_to_none(fields[10]),
                empty_to_none(fields[11]),
                Some(12),
            ),
            10 | 15 => (None, None, (fields.len() == 15).then_some(10)),
            _ => unreachable!("unexpected tmux field count already validated"),
        };

        let (agent_provider, agent_label, agent_cwd, agent_state, agent_session_id) =
            if let Some(start) = agent_fields_start {
                (
                    empty_to_none(fields[start]),
                    empty_to_none(fields[start + 1]),
                    empty_to_none(fields[start + 2]),
                    empty_to_none(fields[start + 3]),
                    empty_to_none(fields[start + 4]),
                )
            } else {
                (None, None, None, None, None)
            };

        panes.push(TmuxPaneRow {
            session_name: fields[0].to_string(),
            window_index: parse_u32(fields[1], "window_index", line_number + 1)?,
            pane_index: parse_u32(fields[2], "pane_index", line_number + 1)?,
            pane_id: fields[3].to_string(),
            pane_pid: parse_u32(fields[4], "pane_pid", line_number + 1)?,
            pane_current_command: fields[5].to_string(),
            pane_title_raw: fields[6].to_string(),
            pane_tty: fields[7].to_string(),
            pane_current_path: fields[8].to_string(),
            window_name: fields[9].to_string(),
            session_id,
            window_id,
            agent_provider,
            agent_label,
            agent_cwd,
            agent_state,
            agent_session_id,
        });
    }

    Ok(panes)
}

fn parse_u32(value: &str, field_name: &str, line_number: usize) -> Result<u32> {
    value.parse::<u32>().with_context(|| {
        format!("failed to parse {field_name} as u32 on tmux output line {line_number}")
    })
}

fn empty_to_none(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

pub(crate) fn tmux_version() -> Option<String> {
    let output = Command::new("tmux").arg("-V").output().ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    stdout
        .trim()
        .strip_prefix("tmux ")
        .map(|version| version.to_string())
        .or_else(|| Some(stdout.trim().to_string()))
}

pub(crate) fn default_session_target() -> Result<String> {
    if env::var_os("TMUX").is_some() {
        let output = Command::new("tmux")
            .args(["display-message", "-p", "#{session_id}"])
            .output()
            .context("failed to query current tmux session")?;
        if output.status.success() {
            let stdout =
                String::from_utf8(output.stdout).context("current session was not UTF-8")?;
            let session = stdout.trim();
            if !session.is_empty() {
                return Ok(session.to_string());
            }
        }
    }

    let output = Command::new("tmux")
        .args(["list-sessions", "-F", "#{session_id}"])
        .output()
        .context("failed to list tmux sessions")?;
    if !output.status.success() {
        bail!("tmux list-sessions failed with status {}", output.status);
    }

    let stdout = String::from_utf8(output.stdout).context("tmux sessions output was not UTF-8")?;
    let session = stdout
        .lines()
        .find(|line| !line.trim().is_empty())
        .context("no tmux sessions available for daemon attach")?;
    Ok(session.trim().to_string())
}

fn current_pane_id() -> Result<Option<String>> {
    if env::var_os("TMUX").is_none() {
        return Ok(None);
    }

    let output = Command::new("tmux")
        .args(["display-message", "-p", "#{pane_id}"])
        .output()
        .context("failed to query current tmux pane id")?;
    if !output.status.success() {
        return Ok(None);
    }

    let stdout =
        String::from_utf8(output.stdout).context("current pane id output was not UTF-8")?;
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

    let output = Command::new("tmux")
        .args(["display-message", "-p", "#{client_tty}"])
        .output()
        .context("failed to query current tmux client tty")?;
    if !output.status.success() {
        return Ok(None);
    }

    let stdout =
        String::from_utf8(output.stdout).context("current client tty output was not UTF-8")?;
    let tty = stdout.trim();
    if tty.is_empty() {
        Ok(None)
    } else {
        Ok(Some(tty.to_string()))
    }
}

fn attached_client_tty() -> Result<Option<String>> {
    let output = Command::new("tmux")
        .args(["list-clients", "-F"])
        .arg(format!(
            "#{{client_tty}}{TMUX_FORMAT_DELIM}#{{client_activity}}"
        ))
        .output()
        .context("failed to list tmux clients")?;
    if !output.status.success() {
        return Ok(None);
    }

    let stdout = String::from_utf8(output.stdout).context("tmux client output was not UTF-8")?;
    let clients = parse_tmux_client_rows(&stdout)?;
    Ok(select_best_client_tty(&clients))
}

pub(crate) fn parse_tmux_client_rows(input: &str) -> Result<Vec<TmuxClientRow>> {
    let mut clients = Vec::new();

    for (line_number, line) in input.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }

        let fields = split_tmux_fields(line);
        if fields.len() != 2 {
            bail!(
                "unexpected tmux client field count on line {}: expected 2, got {}",
                line_number + 1,
                fields.len()
            );
        }

        let client_tty = fields[0].trim();
        if client_tty.is_empty() {
            continue;
        }

        clients.push(TmuxClientRow {
            client_tty: client_tty.to_string(),
            client_activity: fields[1].trim().parse::<i64>().with_context(|| {
                format!(
                    "failed to parse client_activity as i64 on tmux output line {}",
                    line_number + 1
                )
            })?,
        });
    }

    Ok(clients)
}

fn split_tmux_fields(line: &str) -> Vec<&str> {
    let fields: Vec<_> = line.split(PANE_DELIM).collect();
    if fields.len() > 1 {
        return fields;
    }

    line.split(TMUX_FORMAT_DELIM).collect()
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
    let mut command = Command::new("tmux");
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
    let status = if let Some(client_tty) = client_tty.as_deref() {
        Command::new("tmux")
            .args(["switch-client", "-c", client_tty, "-T", "prefix"])
            .status()
            .context("failed to execute tmux switch-client -T prefix with client tty")?
    } else {
        Command::new("tmux")
            .args(["switch-client", "-T", "prefix"])
            .status()
            .context("failed to execute tmux switch-client -T prefix")?
    };
    if !status.success() {
        bail!("tmux switch-client -T prefix failed with status {status}");
    }
    Ok(())
}

pub(crate) fn display_tmux_message(client_tty: Option<&str>, message: &str) -> Result<()> {
    let client_tty = resolve_focus_client_tty(client_tty)?;
    let status = if let Some(client_tty) = client_tty.as_deref() {
        Command::new("tmux")
            .args(["display-message", "-c", client_tty, message])
            .status()
            .context("failed to execute tmux display-message with client tty")?
    } else {
        Command::new("tmux")
            .args(["display-message", message])
            .status()
            .context("failed to execute tmux display-message")?
    };
    if !status.success() {
        bail!("tmux display-message failed with status {status}");
    }
    Ok(())
}

pub(crate) fn tmux_target_is_missing(stderr: &[u8]) -> bool {
    let stderr = String::from_utf8_lossy(stderr);
    let stderr = stderr.trim();
    stderr.contains("can't find pane") || stderr.contains("can't find window")
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

pub(crate) fn tmux_metadata_updates(args: &TmuxSetMetadataArgs) -> Vec<(&'static str, String)> {
    let mut updates = Vec::new();

    if let Some(provider) = args.provider {
        updates.push(("@agent.provider", provider.to_string()));
    }
    if let Some(label) = args
        .label
        .as_deref()
        .map(str::trim)
        .filter(|label| !label.is_empty())
    {
        updates.push(("@agent.label", label.to_string()));
    }
    if let Some(cwd) = args
        .cwd
        .as_deref()
        .map(str::trim)
        .filter(|cwd| !cwd.is_empty())
    {
        updates.push(("@agent.cwd", cwd.to_string()));
    }
    if let Some(state) = args.state {
        updates.push(("@agent.state", status_kind_name(state).to_string()));
    }
    if let Some(session_id) = args
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|session_id| !session_id.is_empty())
    {
        updates.push(("@agent.session_id", session_id.to_string()));
    }

    updates
}

pub(crate) fn tmux_metadata_fields_to_clear(fields: &[TmuxMetadataField]) -> Vec<&'static str> {
    if fields.is_empty() {
        return vec![
            "@agent.provider",
            "@agent.label",
            "@agent.cwd",
            "@agent.state",
            "@agent.session_id",
        ];
    }

    fields
        .iter()
        .map(|field| match field {
            TmuxMetadataField::Provider => "@agent.provider",
            TmuxMetadataField::Label => "@agent.label",
            TmuxMetadataField::Cwd => "@agent.cwd",
            TmuxMetadataField::State => "@agent.state",
            TmuxMetadataField::SessionId => "@agent.session_id",
        })
        .collect()
}

pub(crate) fn set_tmux_pane_option(pane_id: &str, option_name: &str, value: &str) -> Result<()> {
    let status = Command::new("tmux")
        .args(["set-option", "-p", "-t", pane_id, option_name, value])
        .status()
        .with_context(|| format!("failed to set tmux option {option_name} on {pane_id}"))?;
    if !status.success() {
        bail!("tmux set-option failed for {option_name} on {pane_id}");
    }

    Ok(())
}

pub(crate) fn unset_tmux_pane_option(pane_id: &str, option_name: &str) -> Result<()> {
    let status = Command::new("tmux")
        .args(["set-option", "-p", "-u", "-t", pane_id, option_name])
        .status()
        .with_context(|| format!("failed to clear tmux option {option_name} on {pane_id}"))?;
    if !status.success() {
        bail!("tmux set-option -u failed for {option_name} on {pane_id}");
    }

    Ok(())
}
