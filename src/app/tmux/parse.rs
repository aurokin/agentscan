use super::*;

pub(crate) fn parse_pane_rows(input: &str) -> Result<Vec<TmuxPaneRow>> {
    let mut panes = Vec::new();

    for (line_number, line) in input.lines().enumerate() {
        if line.is_empty() {
            continue;
        }

        let fields = split_tmux_fields(line);
        // Field count by optional sections: core(10) + active(2) is always
        // present; session/window ids (+2) and the @agent block (+5) are
        // optional. The two active flags (#{pane_active}, #{window_active})
        // are always the trailing two fields, so the id/agent offsets are
        // unchanged from the pre-active layout — only the totals grew by 2.
        let field_count = fields.len();
        if field_count != 12 && field_count != 14 && field_count != 17 && field_count != 19 {
            bail!(
                "unexpected tmux pane field count on line {}: expected 12, 14, 17, or 19, got {}",
                line_number + 1,
                field_count
            );
        }

        let (session_id, window_id, agent_fields_start) = match field_count {
            14 => (empty_to_none(fields[10]), empty_to_none(fields[11]), None),
            19 => (
                empty_to_none(fields[10]),
                empty_to_none(fields[11]),
                Some(12),
            ),
            12 | 17 => (None, None, (field_count == 17).then_some(10)),
            _ => unreachable!("unexpected tmux field count already validated"),
        };

        // Active flags trail every variant.
        let pane_active = parse_bool_flag(fields[field_count - 2]);
        let window_active = parse_bool_flag(fields[field_count - 1]);

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
