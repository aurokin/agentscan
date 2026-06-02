#[test]
fn classifies_from_command() {
    let matched = classify::classify_provider(None, "codex", "").expect("should match codex");
    assert_eq!(matched.provider, Provider::Codex);
    assert_eq!(
        matched.matched_by,
        super::ClassificationMatchKind::PaneCurrentCommand
    );
    assert_eq!(
        matched.confidence,
        super::ClassificationConfidence::High,
        "exact canonical binary match should be high confidence"
    );

    let suffixed = classify::classify_provider(None, "codex-exec", "")
        .expect("suffixed codex binary should still classify");
    assert_eq!(suffixed.provider, Provider::Codex);
    assert_eq!(
        suffixed.confidence,
        super::ClassificationConfidence::Medium,
        "suffixed binary match should stay medium confidence"
    );

    let gemini_cli = classify::classify_provider(None, "gemini-cli", "")
        .expect("gemini-cli should classify as Gemini");
    assert_eq!(gemini_cli.provider, Provider::Gemini);
    assert_eq!(
        gemini_cli.matched_by,
        super::ClassificationMatchKind::PaneCurrentCommand
    );
    assert_eq!(
        gemini_cli.confidence,
        super::ClassificationConfidence::Medium,
        "suffixed gemini binary should stay medium confidence"
    );

    let agy = classify::classify_provider(None, "agy", "koopa.home.arpa")
        .expect("agy should classify as Antigravity");
    assert_eq!(agy.provider, Provider::Antigravity);
    assert_eq!(
        agy.matched_by,
        super::ClassificationMatchKind::PaneCurrentCommand
    );
    assert_eq!(
        agy.confidence,
        super::ClassificationConfidence::High,
        "exact agy binary match should be high confidence"
    );
}

#[test]
fn rejects_suffixed_binaries_for_generic_word_providers() {
    assert!(
        classify::classify_provider(None, "copilot-backend", "").is_none(),
        "copilot suffix should not classify as Copilot"
    );
    assert!(
        classify::classify_provider(None, "cursor-agent-beta", "").is_none(),
        "cursor-agent suffix should not classify as CursorCli"
    );
    assert!(
        classify::classify_provider(None, "pi-coding-agent-foo", "").is_none(),
        "pi-coding-agent suffix should not classify as Pi"
    );
    assert!(
        classify::classify_provider(None, "agy-beta", "").is_none(),
        "agy suffix should not classify as Antigravity"
    );
    assert!(
        classify::classify_provider(None, "droid-helper", "").is_none(),
        "droid suffix should not classify as Droid"
    );
}

#[test]
fn classifies_copilot_and_cursor_cli_from_command() {
    let copilot = classify::classify_provider(None, "copilot", "").expect("should match copilot");
    assert_eq!(copilot.provider, Provider::Copilot);
    assert_eq!(
        copilot.matched_by,
        super::ClassificationMatchKind::PaneCurrentCommand
    );

    let github_copilot = classify::classify_provider(None, "github-copilot", "")
        .expect("should match github-copilot");
    assert_eq!(github_copilot.provider, Provider::Copilot);
    assert_eq!(
        github_copilot.matched_by,
        super::ClassificationMatchKind::PaneCurrentCommand
    );

    let cursor =
        classify::classify_provider(None, "cursor-agent", "").expect("should match cursor cli");
    assert_eq!(cursor.provider, Provider::CursorCli);
    assert_eq!(
        cursor.matched_by,
        super::ClassificationMatchKind::PaneCurrentCommand
    );

    let plain_cursor = classify::classify_provider(None, "cursor", "");
    assert!(
        plain_cursor.is_none(),
        "plain cursor launcher should not match cursor cli"
    );
}

#[test]
fn classifies_droid_from_command_and_metadata_aliases() {
    let command = classify::classify_provider(None, "droid", "").expect("should match droid");
    assert_eq!(command.provider, Provider::Droid);
    assert_eq!(
        command.matched_by,
        super::ClassificationMatchKind::PaneCurrentCommand
    );
    assert_eq!(command.confidence, super::ClassificationConfidence::High);

    let metadata = classify::classify_provider(Some("factory-droid"), "zsh", "custom title")
        .expect("metadata should match droid");
    assert_eq!(metadata.provider, Provider::Droid);
    assert_eq!(
        metadata.matched_by,
        super::ClassificationMatchKind::PaneMetadata
    );
}

#[test]
fn droid_title_text_alone_does_not_classify_provider() {
    assert!(classify::classify_provider(None, "zsh", "⛬ New Session").is_none());
    assert!(classify::classify_provider(None, "zsh", "Droid").is_none());
}

#[test]
fn classifies_pi_from_specific_command_and_title() {
    let command = classify::classify_provider(None, "pi-coding-agent", "")
        .expect("should match pi coding agent");
    assert_eq!(command.provider, Provider::Pi);
    assert_eq!(
        command.matched_by,
        super::ClassificationMatchKind::PaneCurrentCommand
    );

    let title = classify::classify_provider(None, "pi", "pi - refactor - agentscan")
        .expect("bare pi command plus task title should match");
    assert_eq!(title.provider, Provider::Pi);
    assert_eq!(
        title.matched_by,
        super::ClassificationMatchKind::PaneCurrentCommand
    );
}

#[test]
fn does_not_classify_bare_pi_command_without_other_signal() {
    let bare = classify::classify_provider(None, "pi", "");
    let generic_title = classify::classify_provider(None, "pi", "pi - agentscan");

    assert!(bare.is_none(), "bare pi command should not match");
    assert!(
        generic_title.is_none(),
        "bare pi command plus generic title should not match"
    );
}

#[test]
fn stale_pi_glyph_title_over_foreign_agent_runtime_defers_to_process_evidence() {
    // A `π - ` OSC title left by a prior pi session persists after a *different* agent launches in
    // the same shell — e.g. hermes, whose pty foreground is `python3.11`. The residual glyph title
    // must not shadow the real provider: with no live pi runtime foreground and no spinner, this
    // matcher returns None so process evidence (which finds hermes) can take over.
    assert!(
        classify::classify_provider(None, "python3.11", "π - agentscan").is_none(),
        "stale pi glyph title over a foreign agent runtime must not classify as pi"
    );

    // A genuine live pi session holds the pty foreground as a pi runtime (`node`/`bun`/`pi`) and
    // still classifies by its glyph title.
    let live = classify::classify_provider(None, "node", "π - refactor - agentscan")
        .expect("live pi runtime under a glyph title should classify as pi");
    assert_eq!(live.provider, Provider::Pi);
    assert_eq!(live.matched_by, super::ClassificationMatchKind::PaneTitle);

    // A spinner glyph is live evidence the title is being actively repainted, so it classifies
    // even when tmux momentarily reports the shell as the foreground command.
    let spinning = classify::classify_provider(None, "zsh", "⠋ π - refactor - agentscan")
        .expect("spinner glyph over a pi title should classify as pi");
    assert_eq!(spinning.provider, Provider::Pi);
    assert_eq!(
        spinning.matched_by,
        super::ClassificationMatchKind::PaneTitle
    );
}

#[test]
fn classifies_from_title_when_command_is_generic() {
    let matched = classify::classify_provider(None, "zsh", "Claude Code | Working")
        .expect("should match claude");
    assert_eq!(matched.provider, Provider::Claude);
    assert_eq!(
        matched.matched_by,
        super::ClassificationMatchKind::PaneTitle
    );

    let gemini = classify::classify_provider(None, "zsh", "◇  Ready (workspace)")
        .expect("should match gemini");
    assert_eq!(gemini.provider, Provider::Gemini);
    assert_eq!(gemini.matched_by, super::ClassificationMatchKind::PaneTitle);

    let opencode_default =
        classify::classify_provider(None, "zsh", "OpenCode").expect("should match opencode");
    assert_eq!(opencode_default.provider, Provider::Opencode);
    assert_eq!(
        opencode_default.matched_by,
        super::ClassificationMatchKind::PaneTitle
    );

    let opencode_session = classify::classify_provider(None, "zsh", "OC | Query planner")
        .expect("should match opencode session title");
    assert_eq!(opencode_session.provider, Provider::Opencode);
    assert_eq!(
        opencode_session.matched_by,
        super::ClassificationMatchKind::PaneTitle
    );

    assert!(
        classify::classify_provider(None, "zsh", "GitHub Copilot").is_none(),
        "Copilot title text alone should not classify closed-source panes"
    );
}

#[test]
fn closed_source_branding_titles_do_not_classify_when_command_is_generic() {
    for title in [
        "GitHub Copilot",
        "Copilot | Working",
        "Cursor Agent",
        "Cursor CLI",
        "Cursor | Query planner",
    ] {
        assert!(
            classify::classify_provider(None, "zsh", title).is_none(),
            "closed-source title text alone should not classify: {title}"
        );
    }
}

#[test]
fn closed_source_titles_remain_labels_after_strong_provider_identity() {
    let copilot =
        classify::classify_provider(None, "copilot", "Copilot | Working").expect("command matches");
    assert_eq!(copilot.provider, Provider::Copilot);
    assert_eq!(
        copilot.matched_by,
        super::ClassificationMatchKind::PaneCurrentCommand
    );

    let cursor = classify::classify_provider(None, "cursor-agent", "Cursor CLI | Query planner")
        .expect("command matches");
    assert_eq!(cursor.provider, Provider::CursorCli);
    assert_eq!(
        cursor.matched_by,
        super::ClassificationMatchKind::PaneCurrentCommand
    );
}

#[test]
fn opencode_title_matches_stay_exact() {
    assert!(
        classify::classify_provider(None, "zsh", "OpenCoder").is_none(),
        "nearby product names should not classify as opencode"
    );
    assert!(
        classify::classify_provider(None, "zsh", "Review opencode implementation").is_none(),
        "generic mentions should not classify as opencode"
    );
}

#[test]
fn gemini_mentions_in_titles_do_not_classify_generic_panes() {
    assert!(
        classify::classify_provider(None, "zsh", "Clone Gemini CLI open source library").is_none()
    );
    assert!(
        classify::classify_provider(None, "2.1.119", "✳ Clone Gemini CLI open source library")
            .is_none()
    );
    assert!(
        classify::classify_provider(None, "zsh", "✦ Process deployment").is_none(),
        "arbitrary sparkle titles without Gemini context should not classify"
    );
    assert!(
        classify::classify_provider(None, "zsh", "✦  Process deployment").is_none(),
        "two-space sparkle titles without Gemini context should not classify"
    );
    assert!(
        classify::classify_provider(None, "zsh", "◇ Ready for deploy").is_none(),
        "Gemini ready glyph must match the upstream title shape"
    );
    assert!(
        classify::classify_provider(None, "zsh", "◇ Ready").is_none(),
        "Gemini ready glyph without upstream context should not classify"
    );
}

#[test]
fn classifies_from_command_before_conflicting_title() {
    let cursor_title = classify::classify_provider(None, "codex", "Cursor | Working")
        .expect("command should beat conflicting cursor title");
    assert_eq!(cursor_title.provider, Provider::Codex);
    assert_eq!(
        cursor_title.matched_by,
        super::ClassificationMatchKind::PaneCurrentCommand
    );

    let pi_title = classify::classify_provider(None, "codex", "pi - refactor - agentscan")
        .expect("command should beat conflicting pi title");
    assert_eq!(pi_title.provider, Provider::Codex);
    assert_eq!(
        pi_title.matched_by,
        super::ClassificationMatchKind::PaneCurrentCommand
    );
}

#[test]
fn codex_shaped_titles_win_before_pi_heuristic() {
    let matched = classify::classify_provider(None, "zsh", "pi - refactor - agentscan: codex")
        .expect("codex-shaped title should still classify as codex");
    assert_eq!(matched.provider, Provider::Codex);
    assert_eq!(
        matched.matched_by,
        super::ClassificationMatchKind::PaneTitle
    );
}

#[test]
fn codex_shaped_titles_win_before_copilot_and_cursor_prefixes() {
    let copilot = classify::classify_provider(None, "zsh", "Copilot | review patch: codex")
        .expect("codex-shaped copilot wrapper title should classify as codex");
    assert_eq!(copilot.provider, Provider::Codex);
    assert_eq!(
        copilot.matched_by,
        super::ClassificationMatchKind::PaneTitle
    );

    let cursor = classify::classify_provider(None, "zsh", "Cursor CLI | parser work: codex")
        .expect("codex-shaped cursor wrapper title should classify as codex");
    assert_eq!(cursor.provider, Provider::Codex);
    assert_eq!(cursor.matched_by, super::ClassificationMatchKind::PaneTitle);
}

#[test]
fn classifies_from_pane_metadata_before_title_and_command() {
    let matched = classify::classify_provider(Some("codex"), "zsh", "Claude Code | Working")
        .expect("pane metadata should match codex");
    assert_eq!(matched.provider, Provider::Codex);
    assert_eq!(
        matched.matched_by,
        super::ClassificationMatchKind::PaneMetadata
    );
}

#[test]
fn classifies_copilot_and_cursor_cli_from_metadata_aliases() {
    let copilot = classify::classify_provider(Some("github-copilot"), "zsh", "Cursor CLI | Ready")
        .expect("pane metadata should match copilot");
    assert_eq!(copilot.provider, Provider::Copilot);
    assert_eq!(
        copilot.matched_by,
        super::ClassificationMatchKind::PaneMetadata
    );

    let cursor = classify::classify_provider(Some("cursor cli"), "zsh", "Copilot | Working")
        .expect("pane metadata should match cursor cli");
    assert_eq!(cursor.provider, Provider::CursorCli);
    assert_eq!(
        cursor.matched_by,
        super::ClassificationMatchKind::PaneMetadata
    );

    let cursor_agent =
        classify::classify_provider(Some("cursor-agent"), "zsh", "Copilot | Working")
            .expect("cursor-agent metadata should match cursor cli");
    assert_eq!(cursor_agent.provider, Provider::CursorCli);
    assert_eq!(
        cursor_agent.matched_by,
        super::ClassificationMatchKind::PaneMetadata
    );
}

#[test]
fn classifies_pi_from_metadata_aliases() {
    let pi = classify::classify_provider(Some("pi-coding-agent"), "zsh", "Claude Code | Working")
        .expect("pane metadata should match pi");
    assert_eq!(pi.provider, Provider::Pi);
    assert_eq!(pi.matched_by, super::ClassificationMatchKind::PaneMetadata);
}

#[test]
fn provider_metadata_table_covers_aliases_commands_and_summary_order() {
    for (alias, expected) in [
        ("codex", Provider::Codex),
        ("claude", Provider::Claude),
        ("gemini", Provider::Gemini),
        ("antigravity", Provider::Antigravity),
        ("agy", Provider::Antigravity),
        ("google antigravity", Provider::Antigravity),
        ("opencode", Provider::Opencode),
        ("github copilot", Provider::Copilot),
        ("cursor-cli", Provider::CursorCli),
        ("cursor cli", Provider::CursorCli),
        ("pi coding agent", Provider::Pi),
        ("grok", Provider::Grok),
        ("grok build", Provider::Grok),
        ("hermes-agent", Provider::Hermes),
        ("hermes agent", Provider::Hermes),
        ("droid", Provider::Droid),
        ("factory-droid", Provider::Droid),
        ("factory droid", Provider::Droid),
    ] {
        assert_eq!(
            super::provider_from_metadata(Some(alias)),
            Some(expected),
            "metadata alias: {alias}"
        );
    }

    assert_eq!(super::provider_from_metadata(Some(" unknown ")), None);
    assert_eq!(super::provider_from_command("codex-exec"), Some((Provider::Codex, false)));
    assert_eq!(
        super::provider_from_command("agy"),
        Some((Provider::Antigravity, true))
    );
    assert_eq!(
        super::provider_from_command("agy-beta"),
        None,
        "agy should not accept suffixed binaries"
    );
    assert_eq!(
        super::provider_from_command("hermes"),
        Some((Provider::Hermes, true))
    );
    assert_eq!(
        super::provider_from_command("hermes-agent"),
        Some((Provider::Hermes, true))
    );
    assert_eq!(
        super::provider_from_command("grok"),
        Some((Provider::Grok, true))
    );
    assert_eq!(
        super::provider_from_command("grok-0.1.212-ma"),
        Some((Provider::Grok, false))
    );
    assert_eq!(
        super::provider_from_command("hermes-agent-beta"),
        None,
        "generic provider names should not accept suffixed binaries"
    );
    assert_eq!(
        super::provider_from_command("cursor-agent-beta"),
        None,
        "generic provider names should not accept suffixed binaries"
    );
    assert_eq!(
        super::provider_summary_order().collect::<Vec<_>>(),
        vec![
            Provider::Codex,
            Provider::Claude,
            Provider::Gemini,
            Provider::Antigravity,
            Provider::Opencode,
            Provider::Copilot,
            Provider::CursorCli,
            Provider::Pi,
            Provider::Grok,
            Provider::Hermes,
            Provider::Droid,
        ]
    );
}

#[test]
fn provider_summaries_expose_display_markers_and_aliases() {
    let summaries = super::provider_summaries(IconMode::Emoji);

    assert_eq!(summaries.len(), super::provider_summary_order().count());
    assert_codex_provider_summary(&summaries);
    assert_droid_provider_summary(&summaries);
}

fn assert_codex_provider_summary(summaries: &[super::ProviderSummary]) {
    let codex = summaries
        .iter()
        .find(|summary| summary.provider == Provider::Codex)
        .expect("codex summary should be present");
    assert_eq!(codex.name, "codex");
    assert_eq!(codex.display_marker, "\u{f4ac}");
    assert_eq!(codex.display_marker_codepoints, ["U+F4AC"]);
    assert_eq!(codex.active_icon_mode, IconMode::Emoji);
    assert_eq!(codex.active_marker, "💭");
    assert_eq!(codex.active_marker_codepoints, ["U+1F4AD"]);
    assert_eq!(codex.icons.emoji.marker, "💭");
    assert_eq!(codex.icons.emoji.codepoints, ["U+1F4AD"]);
    assert_eq!(codex.icons.nerd_font.marker, "\u{f4ac}");
    assert_eq!(codex.icons.nerd_font.codepoints, ["U+F4AC"]);
    assert_eq!(codex.icons.nerd_font_patched.marker, "\u{100040}");
    assert_eq!(codex.icons.nerd_font_patched.codepoints, ["U+100040"]);
    assert_eq!(codex.metadata_aliases, ["codex"]);
    assert!(
        codex
            .command_aliases
            .iter()
            .any(|alias| alias.name == "codex" && alias.allow_suffix)
    );
}

fn assert_droid_provider_summary(summaries: &[super::ProviderSummary]) {
    let droid = summaries
        .iter()
        .find(|summary| summary.provider == Provider::Droid)
        .expect("droid summary should be present");
    assert_eq!(droid.display_marker, "\u{f020f}");
    assert_eq!(droid.display_marker_codepoints, ["U+F020F"]);
    assert_eq!(droid.active_icon_mode, IconMode::Emoji);
    assert_eq!(droid.active_marker, "🏭");
    assert_eq!(droid.active_marker_codepoints, ["U+1F3ED"]);
    assert_eq!(droid.icons.emoji.marker, "🏭");
    assert_eq!(droid.icons.emoji.codepoints, ["U+1F3ED"]);
    assert_eq!(droid.icons.nerd_font.marker, "\u{f020f}");
    assert_eq!(droid.icons.nerd_font.codepoints, ["U+F020F"]);
    assert_eq!(droid.icons.nerd_font_patched.marker, "\u{100056}");
    assert_eq!(droid.icons.nerd_font_patched.codepoints, ["U+100056"]);
    assert!(droid.metadata_aliases.contains(&"factory-droid"));
}

#[test]
fn patched_provider_icons_follow_agent_icons_v8_manifest() {
    let expected = [
        (Provider::Codex, "\u{100040}", ["U+100040"]),
        (Provider::Claude, "\u{100041}", ["U+100041"]),
        (Provider::Gemini, "\u{100044}", ["U+100044"]),
        (Provider::Antigravity, "\u{10004C}", ["U+10004C"]),
        (Provider::Opencode, "\u{100043}", ["U+100043"]),
        (Provider::Copilot, "\u{100049}", ["U+100049"]),
        (Provider::CursorCli, "\u{100042}", ["U+100042"]),
        (Provider::Pi, "\u{100052}", ["U+100052"]),
        (Provider::Grok, "\u{100051}", ["U+100051"]),
        (Provider::Hermes, "\u{100045}", ["U+100045"]),
        (Provider::Droid, "\u{100056}", ["U+100056"]),
    ];

    let summaries = super::provider_summaries(IconMode::NerdFontPatched);
    for (provider, marker, codepoints) in expected {
        let summary = summaries
            .iter()
            .find(|summary| summary.provider == provider)
            .expect("provider summary should be present");
        assert_eq!(summary.active_icon_mode, IconMode::NerdFontPatched);
        assert_eq!(summary.active_marker, marker);
        assert_eq!(summary.active_marker_codepoints, codepoints);
        assert_eq!(
            super::provider_marker(provider, IconMode::NerdFontPatched),
            marker
        );
    }
}
