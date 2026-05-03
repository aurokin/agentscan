use super::*;

mod command;
mod focus;
mod metadata;
mod parse;

pub(crate) use command::{
    default_session_target, tmux_command, tmux_target_is_missing, tmux_version,
};
use command::{
    run_tmux_status, run_tmux_text_output, tmux_pane_target_is_missing,
    tmux_scope_target_is_missing,
};
#[cfg(test)]
pub(crate) use focus::select_best_client_tty;
pub(crate) use focus::{
    FocusTmuxPaneResult, display_tmux_message, focus_tmux_pane, resolve_focus_target,
    resolve_tmux_target_pane, switch_tmux_client_to_prefix,
};
pub(crate) use metadata::{
    set_tmux_pane_option, tmux_metadata_fields_to_clear, tmux_metadata_updates,
    unset_tmux_pane_option,
};
pub(crate) use parse::{parse_pane_rows, parse_tmux_client_rows};

pub(crate) fn tmux_list_panes() -> Result<Vec<TmuxPaneRow>> {
    let stdout = run_tmux_text_output(
        &["list-panes", "-a", "-F", PANE_FORMAT],
        "tmux",
        "tmux list-panes",
        |_| false,
        "tmux output was not valid UTF-8",
    )?
    .context("tmux list-panes unexpectedly returned no output")?;
    parse_pane_rows(&stdout)
}

pub(crate) fn tmux_list_panes_target(target: &str) -> Result<Option<Vec<TmuxPaneRow>>> {
    let Some(stdout) = run_tmux_text_output(
        &["list-panes", "-t", target, "-F", PANE_FORMAT],
        &format!("tmux list-panes for target {target}"),
        &format!("tmux list-panes -t {target}"),
        tmux_scope_target_is_missing,
        "tmux output was not valid UTF-8",
    )?
    else {
        return Ok(None);
    };

    let rows = parse_pane_rows(&stdout)?;
    Ok(Some(rows))
}

pub(crate) fn tmux_list_pane(pane_id: &str) -> Result<Option<TmuxPaneRow>> {
    let Some(stdout) = run_tmux_text_output(
        &["list-panes", "-t", pane_id, "-F", PANE_FORMAT],
        &format!("tmux list-panes for {pane_id}"),
        &format!("tmux list-panes -t {pane_id}"),
        tmux_pane_target_is_missing,
        "tmux output was not valid UTF-8",
    )?
    else {
        return Ok(None);
    };

    let mut rows = parse_pane_rows(&stdout)?;
    Ok(rows.pop())
}

pub(crate) fn tmux_capture_pane_tail(pane_id: &str, line_count: usize) -> Result<Option<String>> {
    let start = format!("-{}", line_count.max(1));
    run_tmux_text_output(
        &["capture-pane", "-t", pane_id, "-p", "-S", &start],
        &format!("tmux capture-pane for {pane_id}"),
        &format!("tmux capture-pane -t {pane_id}"),
        tmux_pane_target_is_missing,
        "tmux capture-pane output was not valid UTF-8",
    )
}
