use super::*;

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

fn parse_u32(value: &str, field_name: &str, line_number: usize) -> Result<u32> {
    value.parse::<u32>().with_context(|| {
        format!("failed to parse {field_name} as u32 on tmux output line {line_number}")
    })
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
