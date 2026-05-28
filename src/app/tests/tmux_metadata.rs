#[test]
fn tmux_metadata_updates_emit_expected_option_values() {
    let args = super::TmuxSetMetadataArgs {
        pane_id: Some("%41".to_string()),
        provider: Some(Provider::Claude),
        label: Some("Review notes".to_string()),
        cwd: Some("/tmp/notes".to_string()),
        state: Some(StatusKind::Busy),
        session_id: Some("sess-123".to_string()),
    };

    let updates = tmux::tmux_metadata_updates(&args);
    assert_eq!(
        updates,
        vec![
            ("@agent.provider", "claude".to_string()),
            ("@agent.label", "Review notes".to_string()),
            ("@agent.cwd", "/tmp/notes".to_string()),
            ("@agent.state", "busy".to_string()),
            ("@agent.session_id", "sess-123".to_string()),
        ]
    );
}

#[test]
fn tmux_metadata_fields_to_clear_defaults_to_all_fields() {
    assert_eq!(
        tmux::tmux_metadata_fields_to_clear(&[]),
        vec![
            "@agent.provider",
            "@agent.label",
            "@agent.cwd",
            "@agent.state",
            "@agent.session_id",
        ]
    );
}

#[test]
fn tmux_metadata_fields_to_clear_maps_selected_fields() {
    assert_eq!(
        tmux::tmux_metadata_fields_to_clear(&[
            TmuxMetadataField::Provider,
            TmuxMetadataField::State,
            TmuxMetadataField::SessionId,
        ]),
        vec!["@agent.provider", "@agent.state", "@agent.session_id"]
    );
}

