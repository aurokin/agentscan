#[derive(Debug, serde::Deserialize)]
struct PaneCorpusMeta {
    provider: String,
    cli_version: String,
    captured: String,
    cols: usize,
    rows: usize,
    expected_status: String,
    expected_source: String,
    origin: String,
    corroborators: Vec<String>,
    #[serde(default)]
    allow_other_providers: Vec<String>,
}

struct PaneCorpusFixture {
    path: std::path::PathBuf,
    screen: String,
    meta: PaneCorpusMeta,
    provider: Provider,
    expected: StatusKind,
}

const PANE_OUTPUT_PROVIDERS: [Provider; 12] = [
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
    Provider::KimiCode,
];

#[test]
fn pane_snapshot_corpus_matches_provider_classifiers() {
    let corpus_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/pane_corpus");
    let fixture_paths = pane_corpus_fixture_paths(&corpus_root);
    assert!(
        !fixture_paths.is_empty(),
        "pane corpus is empty: {}",
        corpus_root.display()
    );

    for fixture_path in fixture_paths {
        let fixture = load_pane_corpus_fixture(&corpus_root, fixture_path);
        assert_pane_corpus_dimensions(&fixture);
        assert_pane_corpus_owner(&fixture);
        assert_pane_corpus_cross_provider_guards(&fixture);
        assert_pane_corpus_mutations(&fixture);
    }
}

fn pane_corpus_fixture_paths(corpus_root: &Path) -> Vec<std::path::PathBuf> {
    let mut fixtures = Vec::new();
    for provider_entry in read_dir_or_panic(corpus_root, corpus_root) {
        let provider_path = provider_entry.path();
        if !provider_path.is_dir() {
            continue;
        }
        let provider_name = file_name_or_panic(&provider_path);
        parse_corpus_provider(provider_name, &provider_path);

        for version_entry in read_dir_or_panic(&provider_path, &provider_path) {
            let version_path = version_entry.path();
            if !version_path.is_dir() {
                continue;
            }
            for fixture_entry in read_dir_or_panic(&version_path, &version_path) {
                let fixture_path = fixture_entry.path();
                if fixture_path.extension().is_some_and(|ext| ext == "txt") {
                    fixtures.push(fixture_path);
                }
            }
        }
    }
    fixtures.sort();
    fixtures
}

fn read_dir_or_panic(path: &Path, fixture_path: &Path) -> Vec<std::fs::DirEntry> {
    std::fs::read_dir(path)
        .unwrap_or_else(|error| panic!("{}: {error}", fixture_path.display()))
        .map(|entry| entry.unwrap_or_else(|error| panic!("{}: {error}", fixture_path.display())))
        .collect()
}

fn load_pane_corpus_fixture(corpus_root: &Path, path: std::path::PathBuf) -> PaneCorpusFixture {
    let relative = path
        .strip_prefix(corpus_root)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let parts = relative.components().collect::<Vec<_>>();
    assert_eq!(
        parts.len(),
        3,
        "{}: expected <provider>/<cli-version>/<state>.txt",
        path.display()
    );
    let provider_dir = parts[0].as_os_str().to_string_lossy();
    let version_dir = parts[1].as_os_str().to_string_lossy();
    let state = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_else(|| panic!("{}: fixture filename is not UTF-8", path.display()));
    let meta_path = path.with_extension("meta.toml");
    let meta_text = std::fs::read_to_string(&meta_path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let meta: PaneCorpusMeta = toml::from_str(&meta_text)
        .unwrap_or_else(|error| panic!("{}: malformed metadata: {error}", path.display()));

    assert_eq!(meta.provider, provider_dir, "{}: provider mismatch", path.display());
    assert_eq!(
        meta.cli_version,
        version_dir,
        "{}: cli_version mismatch",
        path.display()
    );
    assert_eq!(
        meta.expected_source,
        "pane_output",
        "{}: expected_source mismatch",
        path.display()
    );
    assert_eq!(
        meta.expected_status,
        state,
        "{}: expected_status must match the fixture filename",
        path.display()
    );
    assert!(
        !meta.captured.trim().is_empty(),
        "{}: captured must not be empty",
        path.display()
    );
    assert!(
        !meta.origin.trim().is_empty(),
        "{}: origin must not be empty",
        path.display()
    );

    let expected = parse_corpus_status(state, &path);
    let provider = parse_corpus_provider(&provider_dir, &path);
    let screen = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    PaneCorpusFixture {
        path,
        screen,
        meta,
        provider,
        expected,
    }
}

fn parse_corpus_provider(name: &str, fixture_path: &Path) -> Provider {
    <Provider as clap::ValueEnum>::from_str(name, false).unwrap_or_else(|error| {
        panic!(
            "{}: unknown provider directory {name:?}: {error}",
            fixture_path.display()
        )
    })
}

fn parse_corpus_status(name: &str, fixture_path: &Path) -> StatusKind {
    match name {
        "idle" => StatusKind::Idle,
        "busy" => StatusKind::Busy,
        "waiting" => StatusKind::Waiting,
        _ => panic!(
            "{}: state must be idle, busy, or waiting, got {name:?}",
            fixture_path.display()
        ),
    }
}

fn assert_pane_corpus_dimensions(fixture: &PaneCorpusFixture) {
    let rows = fixture.screen.lines().count();
    let cols = fixture
        .screen
        .lines()
        .map(UnicodeWidthStr::width)
        .max()
        .unwrap_or(0);
    assert_eq!(fixture.meta.rows, rows, "{}: rows mismatch", fixture.path.display());
    assert_eq!(fixture.meta.cols, cols, "{}: cols mismatch", fixture.path.display());
}

fn assert_pane_corpus_owner(fixture: &PaneCorpusFixture) {
    assert_eq!(
        classify::classify_output(fixture.provider, &fixture.screen),
        Some(fixture.expected),
        "{}: owning provider classification mismatch",
        fixture.path.display()
    );
}

fn assert_pane_corpus_cross_provider_guards(fixture: &PaneCorpusFixture) {
    let allowed = fixture
        .meta
        .allow_other_providers
        .iter()
        .map(|name| parse_corpus_provider(name, &fixture.path))
        .collect::<Vec<_>>();
    for &provider in &allowed {
        assert_ne!(
            provider,
            fixture.provider,
            "{}: owning provider cannot be allow-listed",
            fixture.path.display()
        );
        assert!(
            PANE_OUTPUT_PROVIDERS.contains(&provider),
            "{}: allow-listed provider has no pane-output matcher: {provider}",
            fixture.path.display()
        );
    }

    for provider in PANE_OUTPUT_PROVIDERS {
        if provider == fixture.provider {
            continue;
        }
        let actual = classify::classify_output(provider, &fixture.screen);
        if allowed.contains(&provider) {
            assert!(
                actual.is_some(),
                "{}: stale allow_other_providers entry for {provider}",
                fixture.path.display()
            );
        } else {
            assert_eq!(
                actual,
                None,
                "{}: unexpectedly classified by other provider {provider} as {actual:?}",
                fixture.path.display()
            );
        }
    }
}

fn assert_pane_corpus_mutations(fixture: &PaneCorpusFixture) {
    for corroborator in &fixture.meta.corroborators {
        assert!(
            !corroborator.is_empty() && fixture.screen.contains(corroborator),
            "{}: corroborator is absent from screen: {corroborator:?}",
            fixture.path.display()
        );
        let removed = fixture.screen.replace(corroborator, "");
        let spaces = " ".repeat(UnicodeWidthStr::width(corroborator.as_str()));
        let blanked = fixture.screen.replace(corroborator, &spaces);
        assert_safe_corpus_mutation(fixture, corroborator, "removed", &removed);
        assert_safe_corpus_mutation(fixture, corroborator, "blanked", &blanked);
    }
}

fn assert_safe_corpus_mutation(
    fixture: &PaneCorpusFixture,
    corroborator: &str,
    mutation: &str,
    output: &str,
) {
    let actual = classify::classify_output(fixture.provider, output);
    assert!(
        actual.is_none() || actual == Some(fixture.expected),
        "{}: {mutation} corroborator {corroborator:?} inverted {:?} to {actual:?}",
        fixture.path.display(),
        fixture.expected
    );
}

fn file_name_or_panic(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_else(|| panic!("{}: path component is not UTF-8", path.display()))
}
