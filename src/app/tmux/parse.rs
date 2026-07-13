use super::*;

pub(crate) fn parse_pane_rows(input: &str) -> Result<Vec<TmuxPaneRow>> {
    let mut panes = Vec::new();

    for (line_number, line) in input.lines().enumerate() {
        if line.is_empty() {
            continue;
        }

        let fields = split_tmux_fields(line);
        // Row width by optional section: the core and active blocks are always
        // present; the session/window ids and the `@agent` block are optional,
        // so each accepted width is one (ids?, agent?) combination (see
        // `pane_field`). The active flags always trail, so an absent optional
        // block simply shortens the row without shifting the blocks before it.
        let field_count = fields.len();
        if !matches!(
            field_count,
            PANE_ROW_MINIMAL | PANE_ROW_WITH_IDS | PANE_ROW_WITH_AGENT | PANE_ROW_FULL
        ) {
            bail!(
                "unexpected tmux pane field count on line {}: expected {}, {}, {}, or {}, got {}",
                line_number + 1,
                PANE_ROW_MINIMAL,
                PANE_ROW_WITH_IDS,
                PANE_ROW_WITH_AGENT,
                PANE_ROW_FULL,
                field_count
            );
        }

        let has_ids = field_count == PANE_ROW_WITH_IDS || field_count == PANE_ROW_FULL;
        let has_agent = field_count == PANE_ROW_WITH_AGENT || field_count == PANE_ROW_FULL;

        // The ids block, when present, immediately follows the core block and so
        // never shifts.
        let (session_id, window_id) = if has_ids {
            (
                empty_to_none(fields[IDX_SESSION_ID]),
                empty_to_none(fields[IDX_WINDOW_ID]),
            )
        } else {
            (None, None)
        };

        // The agent block follows core (and ids when present), so its start
        // shifts left by the ids width when ids are absent.
        let (agent_provider, agent_label, agent_cwd, agent_state, agent_session_id) = if has_agent {
            let start = CORE_FIELD_COUNT + if has_ids { IDS_FIELD_COUNT } else { 0 };
            (
                empty_to_none(fields[start + AGENT_PROVIDER_OFFSET]),
                empty_to_none(fields[start + AGENT_LABEL_OFFSET]),
                empty_to_none(fields[start + AGENT_CWD_OFFSET]),
                empty_to_none(fields[start + AGENT_STATE_OFFSET]),
                empty_to_none(fields[start + AGENT_SESSION_ID_OFFSET]),
            )
        } else {
            (None, None, None, None, None)
        };

        // The active flags are always the trailing `ACTIVE_FIELD_COUNT` fields.
        let active_start = field_count - ACTIVE_FIELD_COUNT;
        let pane_active = parse_bool_flag(fields[active_start + ACTIVE_PANE_OFFSET]);
        let window_active = parse_bool_flag(fields[active_start + ACTIVE_WINDOW_OFFSET]);

        panes.push(TmuxPaneRow {
            session_name: fields[IDX_SESSION_NAME].to_string(),
            window_index: parse_u32(fields[IDX_WINDOW_INDEX], "window_index", line_number + 1)?,
            pane_index: parse_u32(fields[IDX_PANE_INDEX], "pane_index", line_number + 1)?,
            pane_id: fields[IDX_PANE_ID].to_string(),
            pane_pid: parse_u32(fields[IDX_PANE_PID], "pane_pid", line_number + 1)?,
            pane_current_command: fields[IDX_PANE_CURRENT_COMMAND].to_string(),
            pane_title_raw: fields[IDX_PANE_TITLE].to_string(),
            pane_tty: fields[IDX_PANE_TTY].to_string(),
            pane_current_path: fields[IDX_PANE_CURRENT_PATH].to_string(),
            window_name: fields[IDX_WINDOW_NAME].to_string(),
            session_id,
            window_id,
            agent_provider,
            agent_label,
            agent_cwd,
            agent_state,
            agent_session_id,
            pane_active,
            window_active,
        });
    }

    Ok(panes)
}

pub(crate) fn parse_tmux_client_rows(input: &str) -> Result<Vec<TmuxClientRow>> {
    let mut clients = Vec::new();

    for (line_number, line) in input.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }

        let fields = split_tmux_fields(line);
        // Require the stable core columns: tty + activity. The session column is
        // read defensively (`fields.get(2)`) so it can't turn a tmux quirk into a
        // hard parse failure that loses every client and breaks focus-action tty
        // resolution. Fewer than two columns is genuine format corruption.
        if fields.len() < 2 {
            bail!(
                "unexpected tmux client field count on line {}: expected at least 2, got {}",
                line_number + 1,
                fields.len()
            );
        }

        // Drop clients without a controlling terminal. tmux reports an empty
        // `#{client_tty}` for programmatic, control-mode attachments that have no
        // pty — and the agentscan daemon itself attaches exactly one such
        // control-mode client per session for snapshotting (confirmed via
        // `list-clients`: each shows `control-mode` with an empty tty). These are
        // server plumbing, not humans watching panes, so excluding them keeps the
        // attached-client count and focused-session resolution scoped to real
        // interactive clients. A human client always has a tty — including a human
        // control-mode client such as iTerm2 `tmux -CC`, which keeps its pty and
        // is therefore retained and still able to anchor focus.
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
            client_session: fields.get(2).and_then(|session| empty_to_none(session)),
        });
    }

    Ok(clients)
}

fn parse_u32(value: &str, field_name: &str, line_number: usize) -> Result<u32> {
    value.parse::<u32>().with_context(|| {
        format!("failed to parse {field_name} as u32 on tmux output line {line_number}")
    })
}

/// tmux flag formats (`#{pane_active}`, `#{window_active}`) render `1` when set
/// and `0` otherwise. Treat anything but `1` (including an empty value) as false.
fn parse_bool_flag(value: &str) -> bool {
    value.trim() == "1"
}

fn empty_to_none(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

pub(super) fn split_tmux_fields(line: &str) -> Vec<&str> {
    let fields: Vec<_> = line.split(PANE_DELIM).collect();
    if fields.len() > 1 {
        return fields;
    }

    line.split(TMUX_FORMAT_DELIM).collect()
}
