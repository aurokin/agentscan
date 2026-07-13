#[test]
fn derived_pane_format_matches_frozen_literal() {
    // Contract: `PANE_FORMAT` is derived from the ordered pane-field table in
    // `src/app/pane_field.rs`. This frozen literal is the byte-for-byte string
    // every `list-panes -F` call and snapshot fixture depends on; the derivation
    // must reproduce it exactly. `\\037` here is the escaped `\037` unit
    // separator, matching the original `concat!(..., r"\037", ...)` layout.
    const FROZEN_PANE_FORMAT: &str = "#{session_name}\\037#{window_index}\\037#{pane_index}\\037#{pane_id}\\037#{pane_pid}\\037#{pane_current_command}\\037#{pane_title}\\037#{pane_tty}\\037#{pane_current_path}\\037#{window_name}\\037#{session_id}\\037#{window_id}\\037#{@agent.provider}\\037#{@agent.label}\\037#{@agent.cwd}\\037#{@agent.state}\\037#{@agent.session_id}\\037#{pane_active}\\037#{window_active}";
    assert_eq!(PANE_FORMAT, FROZEN_PANE_FORMAT);
}

#[test]
fn derived_subscription_format_matches_frozen_literal() {
    // Contract: `DAEMON_SUBSCRIPTION_FORMAT` is derived from the same table.
    // This frozen literal is the exact single-brace payload sent to tmux.
    const FROZEN_SUBSCRIPTION_FORMAT: &str = "agentscan:%*:#{pane_id}:#{pane_current_command}:#{pane_title}:#{@agent.provider}:#{@agent.label}:#{@agent.cwd}:#{@agent.state}:#{@agent.session_id}:#{pane_active}:#{window_active}";
    assert_eq!(DAEMON_SUBSCRIPTION_FORMAT, FROZEN_SUBSCRIPTION_FORMAT);
}

#[test]
fn parses_tmux_output_into_rows() {
    let input = concat!(
        "dotfiles\x1f1\x1f1\x1f%50\x1f438455\x1fcodex\x1f(bront) .dotfiles: codex\x1f/dev/pts/55\x1f/home/auro/.dotfiles\x1feditor\x1f1\x1f1\n",
        "notes\x1f4\x1f1\x1f%41\x1f324026\x1fclaude\x1fClaude Code\x1f/dev/pts/44\x1f/home/auro/notes\x1fquery\x1f0\x1f1\n"
    );

    let rows = tmux::parse_pane_rows(input).expect("tmux output should parse");

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].pane_id, "%50");
    assert!(rows[0].pane_active);
    assert!(rows[0].window_active);
    assert_eq!(rows[1].pane_title_raw, "Claude Code");
    assert!(!rows[1].pane_active);
    assert!(rows[1].window_active);
}

#[test]
fn parses_tmux_output_with_session_and_window_ids() {
    let input = "notes\x1f4\x1f1\x1f%41\x1f324026\x1fclaude\x1fClaude Code\x1f/dev/pts/44\x1f/home/auro/notes\x1fquery\x1f$7\x1f@9\x1f1\x1f1\n";

    let rows = tmux::parse_pane_rows(input).expect("tmux output with ids should parse");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].session_id.as_deref(), Some("$7"));
    assert_eq!(rows[0].window_id.as_deref(), Some("@9"));
    assert!(rows[0].pane_active);
    assert!(rows[0].window_active);
}

#[test]
fn parses_tmux_output_with_escaped_delimiters() {
    let input = r"notes\0374\0371\037%41\037324026\037claude\037Claude Code\037/dev/pts/44\037/home/auro/notes\037query\037$7\037@9\037codex\037Task\037/home/auro/notes\037busy\037session-1\0371\0370
";

    let rows = tmux::parse_pane_rows(input).expect("escaped tmux output should parse");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].pane_id, "%41");
    assert_eq!(rows[0].session_id.as_deref(), Some("$7"));
    assert_eq!(rows[0].agent_provider.as_deref(), Some("codex"));
    assert_eq!(rows[0].agent_state.as_deref(), Some("busy"));
    assert!(rows[0].pane_active);
    assert!(!rows[0].window_active);
}

#[test]
fn tmux_output_does_not_split_on_printable_field_content() {
    let input = r"notes\0374\0371\037%41\037324026\037claude\037Task ||AGENTSCAN|| Review\037/dev/pts/44\037/home/auro/notes\037query\037$7\037@9\037codex\037Task ||AGENTSCAN|| Review\037/home/auro/notes\037busy\037session-1\0371\0371
";

    let rows = tmux::parse_pane_rows(input).expect("tmux output with printable token should parse");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].pane_id, "%41");
    assert_eq!(rows[0].pane_title_raw, "Task ||AGENTSCAN|| Review");
    assert_eq!(rows[0].agent_provider.as_deref(), Some("codex"));
    assert!(rows[0].pane_active);
    assert!(rows[0].window_active);
}

#[test]
fn parses_tmux_client_rows_and_selects_most_recent_tty_and_session() {
    // The third row has no tty: tmux reports an empty `#{client_tty}` for the
    // daemon's control-mode plumbing clients. Despite being the most recently
    // active, it must be skipped so it drives neither the tty/session selection
    // nor the attached-client count.
    let input = concat!(
        "/dev/pts/5\x1f1711671000\x1fnotes\n",
        "/dev/pts/7\x1f1711672000\x1fagentscan\n",
        "\x1f1711673000\x1fghost\n"
    );

    let clients = tmux::parse_tmux_client_rows(input).expect("tmux client output should parse");
    assert_eq!(clients.len(), 2);
    assert_eq!(clients[0].client_tty, "/dev/pts/5");
    assert_eq!(clients[0].client_session.as_deref(), Some("notes"));
    assert_eq!(
        tmux::select_best_client_tty(&clients),
        Some("/dev/pts/7".to_string())
    );
    // The most-recently-active *interactive* client's session is the focused one;
    // the more-recent tty-less plumbing client is ignored.
    assert_eq!(
        tmux::select_focused_session(&clients),
        Some("agentscan".to_string())
    );
}

#[test]
fn parses_tmux_client_rows_skips_ttyless_plumbing_clients() {
    // Reproduces the live daemon shape: one human client plus a tty-less
    // control-mode client per session, each tied at the same activity on a
    // different session. Counting the daemon's clients would inflate the attached
    // count and make focus permanently ambiguous (top activity tied across
    // sessions); excluding tty-less clients keeps a single, correct focus.
    let input = concat!(
        "/dev/ttys000\x1f200\x1fagentscan\n",
        "\x1f300\x1fnotes\n",
        "\x1f300\x1fdotfiles\n",
        "\x1f300\x1fagentchat\n"
    );

    let clients = tmux::parse_tmux_client_rows(input).expect("tmux client output should parse");
    assert_eq!(clients.len(), 1);
    assert_eq!(clients[0].client_tty, "/dev/ttys000");
    assert_eq!(
        tmux::select_focused_session(&clients),
        Some("agentscan".to_string())
    );
}

#[test]
fn parses_tmux_client_rows_without_session_column() {
    // Defensive: a client row with only the core tty + activity columns still
    // parses (session unknown) rather than failing the whole client list.
    let input = "/dev/pts/9\x1f400\n";

    let clients = tmux::parse_tmux_client_rows(input).expect("tmux client output should parse");
    assert_eq!(clients.len(), 1);
    assert_eq!(clients[0].client_tty, "/dev/pts/9");
    assert_eq!(clients[0].client_session, None);
    assert_eq!(
        tmux::select_best_client_tty(&clients),
        Some("/dev/pts/9".to_string())
    );
    // No known session → focus is undeterminable rather than guessed.
    assert_eq!(tmux::select_focused_session(&clients), None);
}

#[test]
fn focused_session_optimizes_for_one_client_and_degrades_gracefully() {
    let parse = |input: &str| {
        tmux::parse_tmux_client_rows(input).expect("tmux client output should parse")
    };

    // No clients attached → no focused pane.
    assert_eq!(tmux::select_focused_session(&[]), None);

    // Single client → its session (the case we optimize for).
    assert_eq!(
        tmux::select_focused_session(&parse("/dev/pts/1\x1f100\x1fagentscan\n")),
        Some("agentscan".to_string())
    );

    // Several clients with a clear most-recent winner → that client's session.
    assert_eq!(
        tmux::select_focused_session(&parse(
            "/dev/pts/1\x1f100\x1fnotes\n/dev/pts/2\x1f200\x1fagentscan\n"
        )),
        Some("agentscan".to_string())
    );

    // Tie at the top, but the tied clients agree on the session → use it.
    assert_eq!(
        tmux::select_focused_session(&parse(
            "/dev/pts/1\x1f200\x1fagentscan\n/dev/pts/2\x1f200\x1fagentscan\n"
        )),
        Some("agentscan".to_string())
    );

    // Tie at the top across different sessions → ambiguous, decline to guess.
    assert_eq!(
        tmux::select_focused_session(&parse(
            "/dev/pts/1\x1f200\x1fnotes\n/dev/pts/2\x1f200\x1fagentscan\n"
        )),
        None
    );

    // A tied top client with an unknown session is treated as disagreement, not
    // silently skipped, so focus stays ambiguous rather than guessing the known one.
    assert_eq!(
        tmux::select_focused_session(&parse(
            "/dev/pts/1\x1f200\x1fagentscan\n/dev/pts/2\x1f200\x1f\n"
        )),
        None
    );
}

#[test]
fn parses_tmux_client_rows_with_escaped_delimiters() {
    let input = "/dev/pts/5\\0371711671000\\037notes\n/dev/pts/7\\0371711672000\\037agentscan\n";

    let clients =
        tmux::parse_tmux_client_rows(input).expect("escaped tmux client output should parse");

    assert_eq!(clients.len(), 2);
    assert_eq!(clients[0].client_tty, "/dev/pts/5");
    assert_eq!(
        tmux::select_best_client_tty(&clients),
        Some("/dev/pts/7".to_string())
    );
    assert_eq!(
        tmux::select_focused_session(&clients),
        Some("agentscan".to_string())
    );
}

proptest! {
    #[test]
    fn parse_pane_rows_roundtrips_generated_rows(
        session_name in safe_tmux_field(),
        window_index in 0_u32..1000,
        pane_index in 0_u32..1000,
        pane_pid in 1_u32..u32::MAX,
        pane_current_command in safe_tmux_field(),
        pane_title_raw in safe_tmux_field(),
        pane_tty in safe_tmux_field(),
        pane_current_path in safe_tmux_field(),
        window_name in safe_tmux_field(),
        pane_active in any::<bool>(),
        window_active in any::<bool>(),
    ) {
        let pane_id = format!("%{pane_pid}");
        let line = format!(
            "{session_name}\u{1f}{window_index}\u{1f}{pane_index}\u{1f}{pane_id}\u{1f}{pane_pid}\u{1f}{pane_current_command}\u{1f}{pane_title_raw}\u{1f}{pane_tty}\u{1f}{pane_current_path}\u{1f}{window_name}\u{1f}{}\u{1f}{}",
            pane_active as u8,
            window_active as u8
        );

        let rows = tmux::parse_pane_rows(&line).expect("generated tmux row should parse");
        prop_assert_eq!(rows.len(), 1);

        let row = &rows[0];
        prop_assert_eq!(&row.session_name, &session_name);
        prop_assert_eq!(row.window_index, window_index);
        prop_assert_eq!(row.pane_index, pane_index);
        prop_assert_eq!(&row.pane_id, &pane_id);
        prop_assert_eq!(row.pane_pid, pane_pid);
        prop_assert_eq!(&row.pane_current_command, &pane_current_command);
        prop_assert_eq!(&row.pane_title_raw, &pane_title_raw);
        prop_assert_eq!(&row.pane_tty, &pane_tty);
        prop_assert_eq!(&row.pane_current_path, &pane_current_path);
        prop_assert_eq!(&row.window_name, &window_name);
        prop_assert_eq!(row.pane_active, pane_active);
        prop_assert_eq!(row.window_active, window_active);
    }

    #[test]
    fn known_status_glyphs_strip_to_trimmed_tail(
        glyph in known_status_glyph(),
        padding in 0_usize..4,
        tail in any::<String>(),
    ) {
        let input = format!("{glyph}{}{tail}", " ".repeat(padding));
        prop_assert_eq!(classify::strip_known_status_glyph(&input), tail.trim_start());
    }
}

#[test]
fn parses_tmux_version_strings_seen_in_the_wild() {
    // tmux uses letter suffixes for point releases and a `next-` prefix for
    // pre-releases; both must reduce to a comparable major.minor.
    assert_eq!(tmux::parse_tmux_version("3.2"), Some((3, 2)));
    assert_eq!(tmux::parse_tmux_version("3.2a"), Some((3, 2)));
    assert_eq!(tmux::parse_tmux_version("3.6b"), Some((3, 6)));
    assert_eq!(tmux::parse_tmux_version("next-3.4"), Some((3, 4)));
    assert_eq!(tmux::parse_tmux_version("3"), Some((3, 0)));
    assert_eq!(tmux::parse_tmux_version("10.12"), Some((10, 12)));
    assert_eq!(tmux::parse_tmux_version("master"), None);
    assert_eq!(tmux::parse_tmux_version(""), None);
}

#[test]
fn flags_pre_3_2_tmux_as_lacking_subscription_support() {
    // 3.2 is the floor where `refresh-client -B` subscriptions exist.
    assert_eq!(tmux::tmux_version_supports_subscriptions("3.0"), Some(false));
    assert_eq!(tmux::tmux_version_supports_subscriptions("3.1c"), Some(false));
    assert_eq!(tmux::tmux_version_supports_subscriptions("2.9a"), Some(false));
    assert_eq!(tmux::tmux_version_supports_subscriptions("3.2"), Some(true));
    assert_eq!(tmux::tmux_version_supports_subscriptions("3.2a"), Some(true));
    assert_eq!(tmux::tmux_version_supports_subscriptions("3.6b"), Some(true));
    assert_eq!(tmux::tmux_version_supports_subscriptions("4.0"), Some(true));
    // Unparseable versions are "unknown", not "too old".
    assert_eq!(tmux::tmux_version_supports_subscriptions("master"), None);
}
