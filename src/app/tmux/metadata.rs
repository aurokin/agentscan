use super::*;

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
        updates.push(("@agent.state", state.as_str().to_string()));
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
    run_tmux_status(
        &["set-option", "-p", "-t", pane_id, option_name, value],
        &format!("tmux set-option {option_name} on {pane_id}"),
        &format!("tmux set-option for {option_name} on {pane_id}"),
    )
}

pub(crate) fn unset_tmux_pane_option(pane_id: &str, option_name: &str) -> Result<()> {
    run_tmux_status(
        &["set-option", "-p", "-u", "-t", pane_id, option_name],
        &format!("tmux set-option -u {option_name} on {pane_id}"),
        &format!("tmux set-option -u for {option_name} on {pane_id}"),
    )
}
