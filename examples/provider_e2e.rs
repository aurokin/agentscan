use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use clap::Parser;
use serde::{Deserialize, Serialize};
use serde_json::Value;
// This is a Cargo example target, not shipped runtime code; examples are built
// with dev-dependencies, so `tempfile` intentionally stays out of [dependencies].
use tempfile::TempDir;

const DEFAULT_CATALOG_PATH: &str = "tests/provider_e2e/catalog.toml";
const DEFAULT_LOCAL_CATALOG_PATH: &str = "tests/provider_e2e/local.toml";
const DEFAULT_ARTIFACTS_DIR: &str = "target/provider-e2e";
const AGENTSCAN_TMUX_SOCKET_ENV_VAR: &str = "AGENTSCAN_TMUX_SOCKET";
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const SUBSCRIBE_RECV_INTERVAL: Duration = Duration::from_millis(250);
const PANE_CAPTURE_INTERVAL: Duration = Duration::from_millis(500);
const DEFAULT_STARTUP_TIMEOUT_MS: u64 = 120_000;
const DEFAULT_READY_TIMEOUT_MS: u64 = 120_000;
const DEFAULT_BUSY_TIMEOUT_MS: u64 = 45_000;
const DEFAULT_COMPLETION_TIMEOUT_MS: u64 = 240_000;
const DEFAULT_READY_STATUS: &str = "idle";
const DEFAULT_BUSY_STATUS: &str = "busy";
const DEFAULT_PROMPT: &str = "Print exactly the marker formed by joining these parts with underscores and nothing else: AGENTSCAN, E2E, DONE, {provider}, {run_id}";
const DEFAULT_COMPLETION_MARKER: &str = "AGENTSCAN_E2E_DONE_{provider}_{run_id}";

#[derive(Parser, Debug)]
#[command(
    name = "provider_e2e",
    about = "Run local opt-in real-agent lifecycle e2e checks for agentscan provider detection"
)]
struct Args {
    /// List configured provider names and exit.
    #[arg(long)]
    list_providers: bool,

    /// Provider to run. Repeat to run multiple providers.
    #[arg(long = "provider")]
    providers: Vec<String>,

    /// Run every configured provider. This is always explicit.
    #[arg(long)]
    all: bool,

    /// Allow prompt submission to real agents. Without this, runs stop after ready detection.
    #[arg(long)]
    spend_ok: bool,

    /// Only validate startup/provider/ready state, even when --spend-ok is present.
    #[arg(long)]
    startup_only: bool,

    /// Base provider catalog.
    #[arg(long)]
    catalog: Option<PathBuf>,

    /// Local machine override catalog. Ignored when missing.
    #[arg(long)]
    local_catalog: Option<PathBuf>,

    /// Do not load the local override catalog.
    #[arg(long)]
    no_local_catalog: bool,

    /// Override one provider model as provider:model. Repeatable.
    #[arg(long = "model")]
    model_overrides: Vec<String>,

    /// Override one provider effort/reasoning level as provider:effort. Repeatable.
    #[arg(long = "effort")]
    effort_overrides: Vec<String>,

    /// Override the prompt for every selected provider.
    #[arg(long)]
    prompt: Option<String>,

    /// Path to an agentscan binary. Defaults to target/debug/agentscan, auto-building when missing.
    #[arg(long)]
    agentscan_bin: Option<PathBuf>,

    /// Artifact root for run diagnostics.
    #[arg(long)]
    artifacts_dir: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct CatalogPatch {
    #[serde(default)]
    providers: BTreeMap<String, ProviderPatch>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct ProviderPatch {
    command: Option<String>,
    args: Option<Vec<String>>,
    env: Option<BTreeMap<String, String>>,
    expected_provider: Option<String>,
    expected_match_kinds: Option<Vec<String>>,
    default_model: Option<String>,
    default_effort: Option<String>,
    prompt: Option<String>,
    completion_marker: Option<String>,
    pre_prompt_keys: Option<Vec<String>>,
    submit_keys: Option<Vec<String>>,
    startup_wait_text: Option<String>,
    startup_keys: Option<Vec<String>>,
    startup_steps: Option<Vec<StartupStepPatch>>,
    ready_status: Option<String>,
    busy_status: Option<String>,
    startup_timeout_ms: Option<u64>,
    ready_timeout_ms: Option<u64>,
    busy_timeout_ms: Option<u64>,
    completion_timeout_ms: Option<u64>,
    needs_spend: Option<bool>,
}

impl ProviderPatch {
    fn merge(&mut self, other: ProviderPatch) {
        merge_option(&mut self.command, other.command);
        merge_option(&mut self.args, other.args);
        merge_option(&mut self.env, other.env);
        merge_option(&mut self.expected_provider, other.expected_provider);
        merge_option(&mut self.expected_match_kinds, other.expected_match_kinds);
        merge_option(&mut self.default_model, other.default_model);
        merge_option(&mut self.default_effort, other.default_effort);
        merge_option(&mut self.prompt, other.prompt);
        merge_option(&mut self.completion_marker, other.completion_marker);
        merge_option(&mut self.pre_prompt_keys, other.pre_prompt_keys);
        merge_option(&mut self.submit_keys, other.submit_keys);
        merge_option(&mut self.startup_wait_text, other.startup_wait_text);
        merge_option(&mut self.startup_keys, other.startup_keys);
        merge_option(&mut self.startup_steps, other.startup_steps);
        merge_option(&mut self.ready_status, other.ready_status);
        merge_option(&mut self.busy_status, other.busy_status);
        merge_option(&mut self.startup_timeout_ms, other.startup_timeout_ms);
        merge_option(&mut self.ready_timeout_ms, other.ready_timeout_ms);
        merge_option(&mut self.busy_timeout_ms, other.busy_timeout_ms);
        merge_option(&mut self.completion_timeout_ms, other.completion_timeout_ms);
        merge_option(&mut self.needs_spend, other.needs_spend);
    }
}

fn merge_option<T>(target: &mut Option<T>, source: Option<T>) {
    if source.is_some() {
        *target = source;
    }
}

#[derive(Clone, Debug, Deserialize)]
struct StartupStepPatch {
    wait_text: Option<String>,
    keys: Vec<String>,
    timeout_ms: Option<u64>,
    optional: Option<bool>,
}

#[derive(Clone, Debug)]
struct StartupStep {
    wait_text: Option<String>,
    keys: Vec<String>,
    timeout: Duration,
    optional: bool,
}

impl StartupStep {
    fn from_patch(patch: StartupStepPatch, default_timeout: Duration) -> Self {
        Self {
            wait_text: patch.wait_text,
            keys: patch.keys,
            timeout: Duration::from_millis(
                patch.timeout_ms.unwrap_or_else(|| {
                    u64::try_from(default_timeout.as_millis()).unwrap_or(u64::MAX)
                }),
            ),
            optional: patch.optional.unwrap_or(false),
        }
    }
}

#[derive(Clone, Debug)]
struct ProviderConfig {
    name: String,
    command: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    expected_provider: String,
    expected_match_kinds: Vec<String>,
    default_model: String,
    default_effort: String,
    prompt: String,
    completion_marker: String,
    pre_prompt_keys: Vec<String>,
    submit_keys: Vec<String>,
    startup_wait_text: Option<String>,
    startup_keys: Vec<String>,
    startup_steps: Vec<StartupStep>,
    ready_status: String,
    busy_status: String,
    startup_timeout: Duration,
    ready_timeout: Duration,
    busy_timeout: Duration,
    completion_timeout: Duration,
    needs_spend: bool,
}

impl ProviderConfig {
    fn from_patch(name: &str, patch: ProviderPatch) -> Result<Self> {
        let command = patch
            .command
            .with_context(|| format!("provider {name} is missing `command`"))?;
        let startup_timeout = Duration::from_millis(
            patch
                .startup_timeout_ms
                .unwrap_or(DEFAULT_STARTUP_TIMEOUT_MS),
        );
        let startup_steps = patch
            .startup_steps
            .unwrap_or_default()
            .into_iter()
            .map(|step| StartupStep::from_patch(step, startup_timeout))
            .collect();
        Ok(Self {
            name: name.to_string(),
            command,
            args: patch.args.unwrap_or_default(),
            env: patch.env.unwrap_or_default(),
            expected_provider: patch
                .expected_provider
                .unwrap_or_else(|| normalize_provider_name(name)),
            expected_match_kinds: patch.expected_match_kinds.unwrap_or_default(),
            default_model: patch.default_model.unwrap_or_else(|| "default".to_string()),
            default_effort: patch
                .default_effort
                .unwrap_or_else(|| "default".to_string()),
            prompt: patch.prompt.unwrap_or_else(|| DEFAULT_PROMPT.to_string()),
            completion_marker: patch
                .completion_marker
                .unwrap_or_else(|| DEFAULT_COMPLETION_MARKER.to_string()),
            pre_prompt_keys: patch.pre_prompt_keys.unwrap_or_default(),
            submit_keys: patch
                .submit_keys
                .unwrap_or_else(|| vec!["Enter".to_string()]),
            startup_wait_text: patch.startup_wait_text,
            startup_keys: patch.startup_keys.unwrap_or_default(),
            startup_steps,
            ready_status: patch
                .ready_status
                .unwrap_or_else(|| DEFAULT_READY_STATUS.to_string()),
            busy_status: patch
                .busy_status
                .unwrap_or_else(|| DEFAULT_BUSY_STATUS.to_string()),
            startup_timeout,
            ready_timeout: Duration::from_millis(
                patch.ready_timeout_ms.unwrap_or(DEFAULT_READY_TIMEOUT_MS),
            ),
            busy_timeout: Duration::from_millis(
                patch.busy_timeout_ms.unwrap_or(DEFAULT_BUSY_TIMEOUT_MS),
            ),
            completion_timeout: Duration::from_millis(
                patch
                    .completion_timeout_ms
                    .unwrap_or(DEFAULT_COMPLETION_TIMEOUT_MS),
            ),
            needs_spend: patch.needs_spend.unwrap_or(true),
        })
    }
}

#[derive(Debug)]
struct Catalog {
    providers: BTreeMap<String, ProviderConfig>,
}

#[derive(Clone, Debug)]
struct ProviderRunConfig {
    provider: ProviderConfig,
    model: String,
    effort: String,
    prompt_template: String,
    completion_marker_template: String,
    prompt: String,
    completion_marker: String,
}

impl ProviderRunConfig {
    fn resolve_for_workspace(&self, workspace: &Path, run_id: &str) -> Self {
        let completion_marker = expand_template(
            &self.completion_marker_template,
            &self.provider,
            &self.model,
            &self.effort,
            workspace,
            "",
            run_id,
        );
        let prompt = expand_template(
            &self.prompt_template,
            &self.provider,
            &self.model,
            &self.effort,
            workspace,
            &completion_marker,
            run_id,
        );

        Self {
            provider: self.provider.clone(),
            model: self.model.clone(),
            effort: self.effort.clone(),
            prompt_template: self.prompt_template.clone(),
            completion_marker_template: self.completion_marker_template.clone(),
            prompt,
            completion_marker,
        }
    }
}

#[derive(Debug)]
struct RunContext {
    run_id: String,
    agentscan_bin: PathBuf,
    artifacts_dir: PathBuf,
    spend_ok: bool,
    startup_only: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ProviderOutcomeKind {
    Unknown,
    Blocked,
    Failed,
    Success,
}

impl ProviderOutcomeKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
            Self::Success => "success",
        }
    }
}

#[derive(Default)]
struct OutcomeCounts {
    unknown: usize,
    blocked: usize,
    failed: usize,
    success: usize,
}

impl OutcomeCounts {
    fn record(&mut self, outcome: ProviderOutcomeKind) {
        match outcome {
            ProviderOutcomeKind::Unknown => self.unknown = self.unknown.saturating_add(1),
            ProviderOutcomeKind::Blocked => self.blocked = self.blocked.saturating_add(1),
            ProviderOutcomeKind::Failed => self.failed = self.failed.saturating_add(1),
            ProviderOutcomeKind::Success => self.success = self.success.saturating_add(1),
        }
    }
}

#[derive(Debug, Serialize)]
struct ProviderOutcome {
    provider: String,
    outcome: ProviderOutcomeKind,
    reason: String,
    artifact_dir: PathBuf,
}

impl ProviderOutcome {
    fn new(
        provider: &str,
        outcome: ProviderOutcomeKind,
        reason: impl Into<String>,
        artifact_dir: &Path,
    ) -> Self {
        Self {
            provider: provider.to_string(),
            outcome,
            reason: reason.into(),
            artifact_dir: artifact_dir.to_path_buf(),
        }
    }
}

#[derive(Debug)]
struct Harness {
    tempdir: TempDir,
    tmux_tmpdir: PathBuf,
    tmux_socket_path: PathBuf,
    agentscan_socket_path: PathBuf,
    agentscan_home: PathBuf,
    agentscan_cache_home: PathBuf,
    agentscan_config_home: PathBuf,
    agentscan_bin: PathBuf,
}

#[derive(Debug)]
struct DaemonHandle {
    child: Child,
}

#[derive(Debug)]
struct SubscribeHandle {
    child: Child,
    rx: Receiver<String>,
}

#[derive(Clone, Debug, Serialize)]
struct PaneObservation {
    index: usize,
    provider: Option<String>,
    status_kind: Option<String>,
    status_source: Option<String>,
    matched_by: Option<String>,
    display_label: Option<String>,
}

#[derive(Clone, Copy, Debug)]
enum LifecycleExpectation<'a> {
    ProviderIdentity {
        expected_provider: &'a str,
    },
    Status {
        expected_provider: &'a str,
        expected_status: &'a str,
    },
}

impl<'a> LifecycleExpectation<'a> {
    fn matches(self, pane: &Value) -> bool {
        match self {
            Self::ProviderIdentity { expected_provider } => {
                pane_provider(pane).as_deref() == Some(expected_provider)
            }
            Self::Status {
                expected_provider,
                expected_status,
            } => {
                pane_provider(pane).as_deref() == Some(expected_provider)
                    && pane_status_kind(pane).as_deref() == Some(expected_status)
            }
        }
    }

    fn expected_observation(self) -> ExpectedObservation {
        match self {
            Self::ProviderIdentity { expected_provider } => ExpectedObservation {
                provider: expected_provider.to_string(),
                status_kind: None,
            },
            Self::Status {
                expected_provider,
                expected_status,
            } => ExpectedObservation {
                provider: expected_provider.to_string(),
                status_kind: Some(expected_status.to_string()),
            },
        }
    }

    fn failure_kind(self, observed: &ObservedObservation) -> LifecycleFailureKind {
        match observed.provider.as_deref() {
            None => {
                return if observed.pane_observed {
                    LifecycleFailureKind::ProviderIdentityMissing
                } else {
                    LifecycleFailureKind::TargetPaneNotObserved
                };
            }
            Some(provider) if provider != self.expected_provider() => {
                return LifecycleFailureKind::ProviderIdentityMismatch;
            }
            Some(_) => {}
        }

        match self {
            Self::ProviderIdentity { .. } => LifecycleFailureKind::ProviderIdentityMissing,
            Self::Status { .. } => {
                if observed.status_needs_matcher_update() {
                    LifecycleFailureKind::ProviderStatusMatcherUpdateNeeded
                } else {
                    LifecycleFailureKind::ProviderStatusMismatch
                }
            }
        }
    }

    fn expected_provider(self) -> &'a str {
        match self {
            Self::ProviderIdentity { expected_provider }
            | Self::Status {
                expected_provider, ..
            } => expected_provider,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum LifecycleFailureKind {
    TargetPaneNotObserved,
    ProviderIdentityMissing,
    ProviderIdentityMismatch,
    ProviderStatusMatcherUpdateNeeded,
    ProviderStatusMismatch,
}

impl LifecycleFailureKind {
    fn owner_hint(self) -> &'static str {
        match self {
            Self::TargetPaneNotObserved => {
                "harness or daemon reporting: the target pane was not present in snapshots"
            }
            Self::ProviderIdentityMissing => {
                "provider identity detection: agentscan did not identify the launched pane"
            }
            Self::ProviderIdentityMismatch => {
                "provider identity detection or catalog setup: agentscan reported a different provider"
            }
            Self::ProviderStatusMatcherUpdateNeeded => {
                "provider status detection rules: provider identity matched but status stayed unknown or unchecked"
            }
            Self::ProviderStatusMismatch => {
                "lifecycle mismatch: inspect the pane tail to decide whether the agent behaved differently or status reporting is stale"
            }
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::TargetPaneNotObserved => "target_pane_not_observed",
            Self::ProviderIdentityMissing => "provider_identity_missing",
            Self::ProviderIdentityMismatch => "provider_identity_mismatch",
            Self::ProviderStatusMatcherUpdateNeeded => "provider_status_matcher_update_needed",
            Self::ProviderStatusMismatch => "provider_status_mismatch",
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct ExpectedObservation {
    provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    status_kind: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ObservedObservation {
    pane_observed: bool,
    provider: Option<String>,
    status_kind: Option<String>,
    status_source: Option<String>,
    matched_by: Option<String>,
    display_label: Option<String>,
}

impl ObservedObservation {
    fn from_pane(pane: Option<&Value>) -> Self {
        match pane {
            Some(pane) => Self {
                pane_observed: true,
                provider: pane_provider(pane),
                status_kind: pane_status_kind(pane),
                status_source: pane_status_source(pane),
                matched_by: pane_matched_by(pane),
                display_label: pane
                    .pointer("/display/label")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
            },
            None => Self {
                pane_observed: false,
                provider: None,
                status_kind: None,
                status_source: None,
                matched_by: None,
                display_label: None,
            },
        }
    }

    fn status_needs_matcher_update(&self) -> bool {
        let status_unknown =
            !matches!(self.status_kind.as_deref(), Some(kind) if kind != "unknown");
        status_unknown || self.status_source.as_deref() == Some("not_checked")
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct LifecycleFailureReport {
    phase: String,
    failure_kind: LifecycleFailureKind,
    owner_hint: String,
    expected: ExpectedObservation,
    observed: ObservedObservation,
    pane_tail_excerpt: Vec<String>,
}

impl LifecycleFailureReport {
    fn new(
        phase: &str,
        expectation: LifecycleExpectation<'_>,
        observed_pane: Option<&Value>,
        pane_tail_excerpt: Vec<String>,
    ) -> Self {
        let observed = ObservedObservation::from_pane(observed_pane);
        let failure_kind = expectation.failure_kind(&observed);
        Self {
            phase: phase.to_string(),
            failure_kind,
            owner_hint: failure_kind.owner_hint().to_string(),
            expected: expectation.expected_observation(),
            observed,
            pane_tail_excerpt,
        }
    }

    fn summary(&self) -> String {
        let expected_status = self
            .expected
            .status_kind
            .as_deref()
            .map_or("any".to_string(), |status| status.to_string());
        format!(
            "timed out waiting for {}; failure_kind={}; expected provider `{}` status `{}`; observed provider `{}` status `{}` source `{}`; {}",
            self.phase,
            self.failure_kind.as_str(),
            self.expected.provider,
            expected_status,
            display_optional(self.observed.provider.as_deref()),
            display_optional(self.observed.status_kind.as_deref()),
            display_optional(self.observed.status_source.as_deref()),
            self.owner_hint
        )
    }
}

struct TimelineRecorder {
    file: File,
    target_pane_id: String,
    expected_provider: String,
    expected_match_kinds: Vec<String>,
    observations: Vec<PaneObservation>,
    identity_locked: bool,
    last_pane: Option<Value>,
}

struct ActiveRun<'a> {
    harness: &'a Harness,
    daemon: &'a mut DaemonHandle,
    subscriber: &'a mut SubscribeHandle,
    timeline: &'a mut TimelineRecorder,
    artifact_dir: &'a Path,
    pane_id: &'a str,
}

#[derive(Serialize)]
struct RunMetadata<'a> {
    provider: &'a str,
    expected_provider: &'a str,
    model: &'a str,
    effort: &'a str,
    prompt_submitted: bool,
    completion_marker: &'a str,
    command: &'a str,
    args: &'a [String],
    env_keys: Vec<&'a String>,
    pane_id: &'a str,
    workspace: &'a Path,
    tmux_socket: &'a Path,
    agentscan_socket: &'a Path,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let catalog_path = args
        .catalog
        .clone()
        .unwrap_or_else(|| repo_path(DEFAULT_CATALOG_PATH));
    let local_catalog_path = args
        .local_catalog
        .clone()
        .unwrap_or_else(|| repo_path(DEFAULT_LOCAL_CATALOG_PATH));
    let artifacts_dir = args
        .artifacts_dir
        .clone()
        .unwrap_or_else(|| repo_path(DEFAULT_ARTIFACTS_DIR));
    let catalog = load_catalog(
        &catalog_path,
        (!args.no_local_catalog).then_some(local_catalog_path.as_path()),
    )?;

    if args.list_providers {
        print_providers(&catalog);
        return Ok(());
    }

    let selected = selected_providers(&args, &catalog)?;
    if selected.is_empty() {
        print_providers(&catalog);
        println!("\nSelect one or more providers with `--provider <name>`, or use `--all`.");
        return Ok(());
    }

    let model_overrides = parse_overrides(&args.model_overrides, "--model")?;
    let effort_overrides = parse_overrides(&args.effort_overrides, "--effort")?;
    validate_override_keys("--model", &model_overrides, &catalog, &selected)?;
    validate_override_keys("--effort", &effort_overrides, &catalog, &selected)?;
    let agentscan_bin = resolve_agentscan_bin(args.agentscan_bin.as_deref())?;
    let run_id = new_run_id()?;
    let run_root = artifacts_dir.join(&run_id);
    fs::create_dir_all(&run_root)
        .with_context(|| format!("failed to create {}", run_root.display()))?;

    let context = RunContext {
        run_id,
        agentscan_bin,
        artifacts_dir: run_root,
        spend_ok: args.spend_ok,
        startup_only: args.startup_only,
    };

    let mut counts = OutcomeCounts::default();
    let mut runner_errors = 0usize;
    for provider_name in selected {
        let Some(provider) = catalog.providers.get(&provider_name).cloned() else {
            bail!("selected unknown provider {provider_name}");
        };
        let run_config = provider_run_config(
            provider,
            model_overrides.get(&provider_name),
            effort_overrides.get(&provider_name),
            args.prompt.as_deref(),
        )?;

        println!("running provider e2e: {}", run_config.provider.name);
        match run_provider(&context, &run_config) {
            Ok(outcome) => {
                counts.record(outcome.outcome);
                println!(
                    "{}: {}: {}",
                    outcome.outcome.as_str(),
                    outcome.provider,
                    outcome.reason
                );
            }
            Err(error) => {
                runner_errors = runner_errors.saturating_add(1);
                counts.record(ProviderOutcomeKind::Failed);
                eprintln!(
                    "failed: {}: runner error: {error:#}",
                    run_config.provider.name
                );
            }
        }
    }

    println!(
        "provider e2e summary: success={} blocked={} unknown={} failed={}",
        counts.success, counts.blocked, counts.unknown, counts.failed
    );

    if counts.failed > 0 {
        bail!(
            "{} provider e2e run(s) failed; runner_errors={runner_errors}",
            counts.failed
        );
    }
    Ok(())
}

fn load_catalog(catalog_path: &Path, local_path: Option<&Path>) -> Result<Catalog> {
    let mut merged = read_catalog_patch(catalog_path)?;
    if let Some(local_path) = local_path
        && local_path.exists()
    {
        let local = read_catalog_patch(local_path)?;
        for (name, patch) in local.providers {
            merged.providers.entry(name).or_default().merge(patch);
        }
    }

    let providers = merged
        .providers
        .into_iter()
        .map(|(name, patch)| ProviderConfig::from_patch(&name, patch).map(|config| (name, config)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    Ok(Catalog { providers })
}

fn read_catalog_patch(path: &Path) -> Result<CatalogPatch> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read provider e2e catalog {}", path.display()))?;
    toml::from_str(&text)
        .with_context(|| format!("failed to parse provider e2e catalog {}", path.display()))
}

fn print_providers(catalog: &Catalog) {
    println!("configured providers:");
    for (name, provider) in &catalog.providers {
        println!(
            "- {name}: command={} default_model={} default_effort={}",
            provider.command, provider.default_model, provider.default_effort
        );
    }
}

fn selected_providers(args: &Args, catalog: &Catalog) -> Result<Vec<String>> {
    if args.all && !args.providers.is_empty() {
        bail!("use either --all or one or more --provider flags, not both");
    }
    if args.all {
        return Ok(catalog.providers.keys().cloned().collect());
    }

    let mut selected = Vec::new();
    for provider in &args.providers {
        let normalized = normalize_provider_name(provider);
        if !catalog.providers.contains_key(&normalized) {
            bail!(
                "unknown provider `{provider}`; run with --list-providers to see configured names"
            );
        }
        if !selected.iter().any(|existing| existing == &normalized) {
            selected.push(normalized);
        }
    }
    Ok(selected)
}

fn parse_overrides(values: &[String], flag: &str) -> Result<HashMap<String, String>> {
    let mut overrides = HashMap::new();
    for value in values {
        let Some((provider, override_value)) = value.split_once(':') else {
            bail!("{flag} override must be formatted as provider:value, got `{value}`");
        };
        if provider.trim().is_empty() || override_value.trim().is_empty() {
            bail!("{flag} override must have non-empty provider and value, got `{value}`");
        }
        overrides.insert(
            normalize_provider_name(provider),
            override_value.trim().to_string(),
        );
    }
    Ok(overrides)
}

fn validate_override_keys(
    flag: &str,
    overrides: &HashMap<String, String>,
    catalog: &Catalog,
    selected: &[String],
) -> Result<()> {
    for provider in overrides.keys() {
        if !catalog.providers.contains_key(provider) {
            bail!(
                "{flag} override targets unknown provider `{provider}`; run with --list-providers to see configured names"
            );
        }
        if !selected.iter().any(|selected| selected == provider) {
            bail!(
                "{flag} override targets provider `{provider}`, but that provider is not selected"
            );
        }
    }
    Ok(())
}

fn provider_run_config(
    provider: ProviderConfig,
    model_override: Option<&String>,
    effort_override: Option<&String>,
    prompt_override: Option<&str>,
) -> Result<ProviderRunConfig> {
    let model = model_override
        .cloned()
        .unwrap_or_else(|| provider.default_model.clone());
    let effort = effort_override
        .cloned()
        .unwrap_or_else(|| provider.default_effort.clone());
    let prompt_template = prompt_override
        .map(str::to_string)
        .unwrap_or_else(|| provider.prompt.clone());
    let completion_marker_template = provider.completion_marker.clone();

    Ok(ProviderRunConfig {
        provider,
        model,
        effort,
        prompt_template,
        completion_marker_template,
        prompt: String::new(),
        completion_marker: String::new(),
    })
}

fn command_is_available(command: &str) -> bool {
    let path = Path::new(command);
    if path.components().count() > 1 {
        return is_executable_file(path);
    }

    env::var_os("PATH").is_some_and(|paths| {
        env::split_paths(&paths)
            .map(|path_dir| path_dir.join(command))
            .any(|candidate| is_executable_file(&candidate))
    })
}

fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

fn classify_provider_error(
    provider: &str,
    artifact_dir: &Path,
    timeline: &TimelineRecorder,
    error: &anyhow::Error,
) -> ProviderOutcome {
    let error_text = format!("{error:#}");
    let (outcome, reason) = if provider_launch_exited_before_snapshot(&error_text, timeline)
        || artifact_launch_exited_before_snapshot(artifact_dir, timeline)
        || lifecycle_reports_indicate_blocked(artifact_dir)
        || artifact_text_indicates_blocked(artifact_dir)
        || text_indicates_environment_blocker(&error_text)
    {
        (
            ProviderOutcomeKind::Blocked,
            format!(
                "provider could not complete the lifecycle because auth, install, or environment state blocked it: {}",
                first_error_line(&error_text)
            ),
        )
    } else {
        (
            ProviderOutcomeKind::Failed,
            format!(
                "provider lifecycle or detection assertion failed: {}",
                first_error_line(&error_text)
            ),
        )
    };

    ProviderOutcome::new(provider, outcome, reason, artifact_dir)
}

fn provider_launch_exited_before_snapshot(error_text: &str, timeline: &TimelineRecorder) -> bool {
    timeline.observations.is_empty()
        && error_text.contains("initial snapshot failed before daemon socket readiness")
        && error_text.contains("no server running")
}

fn artifact_launch_exited_before_snapshot(
    artifact_dir: &Path,
    timeline: &TimelineRecorder,
) -> bool {
    timeline.observations.is_empty()
        && fs::read_to_string(artifact_dir.join("daemon.stderr.log")).is_ok_and(|text| {
            text.contains("initial snapshot failed before daemon socket readiness")
                && text.contains("no server running")
        })
}

fn artifact_text_indicates_blocked(artifact_dir: &Path) -> bool {
    [
        "pane-tail-final.txt",
        "pane-tail-before.txt",
        "pane-tail-busy.txt",
        "pane-tail-after.txt",
    ]
    .iter()
    .any(|name| {
        fs::read_to_string(artifact_dir.join(name))
            .is_ok_and(|text| text_indicates_environment_blocker(&text))
    })
}

fn lifecycle_reports_indicate_blocked(artifact_dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(artifact_dir) else {
        return false;
    };

    entries.flatten().any(|entry| {
        let path = entry.path();
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("failure-") && name.ends_with(".json"))
            && fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<LifecycleFailureReport>(&bytes).ok())
                .is_some_and(|report| {
                    report
                        .pane_tail_excerpt
                        .iter()
                        .any(|line| text_indicates_environment_blocker(line))
                })
    })
}

fn text_indicates_environment_blocker(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "failed to sign in",
        "login required",
        "not logged in",
        "not authenticated",
        "consent could not be obtained",
        "interactive terminal to authenticate",
        "api key",
        "rate limit",
        "usage limit",
        "quota exceeded",
        "quota limit",
        "postinstall",
        "not configured",
        "permission denied",
        "no browser",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn first_error_line(error_text: &str) -> &str {
    error_text
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(str::trim)
        .unwrap_or("unknown error")
}

fn write_provider_outcome(artifact_dir: &Path, outcome: &ProviderOutcome) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(outcome).context("failed to encode provider outcome")?;
    fs::write(artifact_dir.join("outcome.json"), bytes).context("failed to write outcome artifact")
}

fn run_provider(context: &RunContext, run_config: &ProviderRunConfig) -> Result<ProviderOutcome> {
    let artifact_dir = context.artifacts_dir.join(&run_config.provider.name);
    fs::create_dir_all(&artifact_dir)
        .with_context(|| format!("failed to create {}", artifact_dir.display()))?;

    if !command_is_available(&run_config.provider.command) {
        let outcome = ProviderOutcome::new(
            &run_config.provider.name,
            ProviderOutcomeKind::Unknown,
            format!(
                "provider command `{}` is not installed or not on PATH",
                run_config.provider.command
            ),
            &artifact_dir,
        );
        write_provider_outcome(&artifact_dir, &outcome)?;
        return Ok(outcome);
    }

    let harness = Harness::new(&context.agentscan_bin)?;
    let workspace = prepare_workspace(&harness, &run_config.provider.name)?;
    let run_config = run_config.resolve_for_workspace(&workspace, &context.run_id);
    let launch_command = agent_launch_command(&run_config, &workspace, &context.run_id, false);
    let redacted_launch_command =
        agent_launch_command(&run_config, &workspace, &context.run_id, true);
    fs::write(
        artifact_dir.join("agent-launch.sh"),
        format!("{redacted_launch_command}\n"),
    )
    .context("failed to write launch command artifact")?;

    let session_name = format!(
        "agentscan-e2e-{}",
        sanitize_identifier(&run_config.provider.name)
    );
    let pane_id = harness.start_session(&session_name, &launch_command)?;
    let mut timeline = TimelineRecorder::new(
        artifact_dir.join("timeline.jsonl"),
        pane_id.clone(),
        run_config.provider.expected_provider.clone(),
        run_config.provider.expected_match_kinds.clone(),
    )?;
    if let Err(error) = harness.apply_startup_keys_if_needed(&pane_id, &run_config) {
        let _ = collect_common_artifacts(&harness, &artifact_dir, &pane_id, &timeline);
        let outcome =
            classify_provider_error(&run_config.provider.name, &artifact_dir, &timeline, &error);
        write_provider_outcome(&artifact_dir, &outcome)?;
        return Ok(outcome);
    }
    let mut daemon = harness.start_daemon(&artifact_dir)?;

    let prompt_submitted =
        (!run_config.provider.needs_spend || context.spend_ok) && !context.startup_only;
    if prompt_submitted && run_config.prompt.contains(&run_config.completion_marker) {
        bail!(
            "expanded prompt for provider `{}` contains the exact completion marker; ask for the marker indirectly so an echoed prompt cannot satisfy completion",
            run_config.provider.name
        );
    }
    write_metadata(
        &artifact_dir,
        &run_config,
        prompt_submitted,
        &pane_id,
        &workspace,
        &harness,
    )?;

    let mut subscriber = None;
    let result = (|| {
        harness.wait_for_daemon_ready(&mut daemon, run_config.provider.startup_timeout)?;
        let active_subscriber = subscriber.insert(harness.start_subscribe(&artifact_dir)?);
        let mut active = ActiveRun {
            harness: &harness,
            daemon: &mut daemon,
            subscriber: active_subscriber,
            timeline: &mut timeline,
            artifact_dir: &artifact_dir,
            pane_id: &pane_id,
        };
        run_provider_lifecycle(&mut active, &run_config, prompt_submitted)
    })();

    let _ = collect_common_artifacts(&harness, &artifact_dir, &pane_id, &timeline);
    if let Some(mut subscriber) = subscriber {
        let _ = subscriber.shutdown();
    }
    let _ = daemon.shutdown();
    let outcome = match result {
        Ok(()) => ProviderOutcome::new(
            &run_config.provider.name,
            ProviderOutcomeKind::Success,
            "lifecycle completed: detected -> ready -> busy -> ready",
            &artifact_dir,
        ),
        Err(error) => {
            classify_provider_error(&run_config.provider.name, &artifact_dir, &timeline, &error)
        }
    };
    write_provider_outcome(&artifact_dir, &outcome)?;
    Ok(outcome)
}

fn run_provider_lifecycle(
    active: &mut ActiveRun<'_>,
    run_config: &ProviderRunConfig,
    prompt_submitted: bool,
) -> Result<()> {
    wait_for_pane_condition(
        active,
        run_config.provider.startup_timeout,
        "provider detection",
        LifecycleExpectation::ProviderIdentity {
            expected_provider: run_config.provider.expected_provider.as_str(),
        },
    )?;

    wait_for_pane_condition(
        active,
        run_config.provider.ready_timeout,
        "ready status before prompt",
        LifecycleExpectation::Status {
            expected_provider: run_config.provider.expected_provider.as_str(),
            expected_status: run_config.provider.ready_status.as_str(),
        },
    )?;

    write_command_output(
        active.artifact_dir.join("inspect-before.json"),
        active.harness.agentscan_output([
            "inspect",
            active.pane_id,
            "--format",
            "json",
            "--no-auto-start",
        ]),
    );
    write_text_artifact(
        active.artifact_dir.join("pane-tail-before.txt"),
        &active.harness.capture_pane_tail(active.pane_id, 200)?,
    )?;

    if !prompt_submitted {
        if run_config.provider.needs_spend {
            println!(
                "startup/ready validated for {}; prompt skipped because --spend-ok was not set",
                run_config.provider.name
            );
        }
        return Ok(());
    }

    active.harness.send_agent_prompt(
        active.pane_id,
        &run_config.provider.pre_prompt_keys,
        &run_config.prompt,
        &run_config.provider.submit_keys,
    )?;

    wait_for_pane_condition(
        active,
        run_config.provider.busy_timeout,
        "busy status after prompt",
        LifecycleExpectation::Status {
            expected_provider: run_config.provider.expected_provider.as_str(),
            expected_status: run_config.provider.busy_status.as_str(),
        },
    )?;
    write_text_artifact(
        active.artifact_dir.join("pane-tail-busy.txt"),
        &active.harness.capture_pane_tail(active.pane_id, 200)?,
    )?;

    wait_for_completion_marker_without_false_ready(
        active.daemon,
        active.subscriber,
        active.timeline,
        active.harness,
        run_config,
        active.pane_id,
    )?;

    wait_for_pane_condition(
        active,
        run_config.provider.ready_timeout,
        "ready status after completion",
        LifecycleExpectation::Status {
            expected_provider: run_config.provider.expected_provider.as_str(),
            expected_status: run_config.provider.ready_status.as_str(),
        },
    )?;

    write_command_output(
        active.artifact_dir.join("inspect-after.json"),
        active.harness.agentscan_output([
            "inspect",
            active.pane_id,
            "--format",
            "json",
            "--no-auto-start",
        ]),
    );
    write_text_artifact(
        active.artifact_dir.join("pane-tail-after.txt"),
        &active.harness.capture_pane_tail(active.pane_id, 200)?,
    )?;

    Ok(())
}

fn wait_for_pane_condition(
    active: &mut ActiveRun<'_>,
    timeout: Duration,
    label: &str,
    expectation: LifecycleExpectation<'_>,
) -> Result<Value> {
    let deadline = Instant::now() + timeout;
    let mut next_snapshot_poll = Instant::now();
    loop {
        active.daemon.ensure_running()?;
        active.subscriber.ensure_running()?;

        if let Some(pane) = active.timeline.last_pane.as_ref()
            && expectation.matches(pane)
        {
            return Ok(pane.clone());
        }

        if Instant::now() >= next_snapshot_poll {
            let output = active.harness.agentscan_output([
                "snapshot",
                "--format",
                "json",
                "--no-auto-start",
            ])?;
            if let Some(pane) = active.timeline.record_snapshot_poll(&output)?
                && expectation.matches(&pane)
            {
                return Ok(pane);
            }
            next_snapshot_poll = Instant::now() + Duration::from_millis(1_000);
        }

        if Instant::now() >= deadline {
            let report = LifecycleFailureReport::new(
                label,
                expectation,
                active.timeline.last_pane.as_ref(),
                pane_tail_excerpt(active.harness, &active.timeline.target_pane_id, 8),
            );
            let report_artifact =
                write_lifecycle_failure_report(active.artifact_dir, label, &report);
            let artifact_note = report_artifact.map_or_else(
                |error| format!("; failed to write failure report: {error:#}"),
                |path| format!("; failure report: {}", path.display()),
            );
            bail!("{}{}", report.summary(), artifact_note);
        }

        let remaining = deadline.saturating_duration_since(Instant::now());
        match active
            .subscriber
            .recv_line(remaining.min(SUBSCRIBE_RECV_INTERVAL))?
        {
            Some(line) => {
                if let Some(pane) = active.timeline.record_line(&line)?
                    && expectation.matches(&pane)
                {
                    return Ok(pane);
                }
            }
            None => thread::sleep(POLL_INTERVAL),
        }
    }
}

fn write_lifecycle_failure_report(
    artifact_dir: &Path,
    label: &str,
    report: &LifecycleFailureReport,
) -> Result<PathBuf> {
    let path = artifact_dir.join(format!("failure-{}.json", sanitize_identifier(label)));
    let bytes = serde_json::to_vec_pretty(report).context("failed to encode failure report")?;
    fs::write(&path, bytes).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

fn wait_for_completion_marker_without_false_ready(
    daemon: &mut DaemonHandle,
    subscriber: &mut SubscribeHandle,
    timeline: &mut TimelineRecorder,
    harness: &Harness,
    run_config: &ProviderRunConfig,
    pane_id: &str,
) -> Result<()> {
    let deadline = Instant::now() + run_config.provider.completion_timeout;
    let mut next_capture = Instant::now();
    loop {
        daemon.ensure_running()?;
        subscriber.ensure_running()?;

        while let Some(line) = subscriber.try_recv_line()? {
            if let Some(pane) = timeline.record_line(&line)? {
                if completion_marker_visible(harness, pane_id, run_config)? {
                    return Ok(());
                }
                ensure_pane_still_busy(&pane, harness, run_config, pane_id)?;
            }
        }

        if Instant::now() >= next_capture {
            if completion_marker_visible(harness, pane_id, run_config)? {
                return Ok(());
            }
            next_capture = Instant::now() + PANE_CAPTURE_INTERVAL;
        }

        if Instant::now() >= deadline {
            bail!(
                "timed out waiting for completion marker `{}`",
                run_config.completion_marker
            );
        }

        if let Some(line) = subscriber.recv_line(SUBSCRIBE_RECV_INTERVAL)?
            && let Some(pane) = timeline.record_line(&line)?
        {
            if completion_marker_visible(harness, pane_id, run_config)? {
                return Ok(());
            }
            ensure_pane_still_busy(&pane, harness, run_config, pane_id)?;
        }
    }
}

fn ensure_pane_still_busy(
    pane: &Value,
    harness: &Harness,
    run_config: &ProviderRunConfig,
    pane_id: &str,
) -> Result<()> {
    let observed_status = pane_status_kind(pane);
    if observed_status.as_deref() == Some(run_config.provider.busy_status.as_str()) {
        return Ok(());
    }

    bail!(
        "pane left {} before completion marker `{}` was observed; observed status `{}` source `{}`{}",
        run_config.provider.busy_status,
        run_config.completion_marker,
        display_optional(observed_status.as_deref()),
        display_optional(pane_status_source(pane).as_deref()),
        completion_failure_context(harness, pane_id)
    );
}

fn completion_failure_context(harness: &Harness, pane_id: &str) -> String {
    match harness.capture_pane_tail(pane_id, 80) {
        Ok(tail) => {
            let excerpt = pane_tail_excerpt_from_text(&tail, 6);
            if excerpt.is_empty() {
                "; pane output was empty".to_string()
            } else if excerpt.iter().any(|line| is_interesting_error_line(line)) {
                format!("; pane error context: {}", excerpt.join(" | "))
            } else {
                format!("; recent pane output: {}", excerpt.join(" | "))
            }
        }
        Err(error) => format!("; failed to capture pane context: {error:#}"),
    }
}

fn pane_tail_excerpt(harness: &Harness, pane_id: &str, max_lines: usize) -> Vec<String> {
    harness.capture_pane_tail(pane_id, 80).map_or_else(
        |error| vec![format!("failed to capture pane tail: {error:#}")],
        |tail| pane_tail_excerpt_from_text(&tail, max_lines),
    )
}

fn pane_tail_excerpt_from_text(tail: &str, max_lines: usize) -> Vec<String> {
    let interesting = tail
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            is_interesting_error_line(trimmed).then_some(trimmed.to_string())
        })
        .take(max_lines)
        .collect::<Vec<_>>();

    if !interesting.is_empty() {
        return interesting;
    }

    let mut recent = tail
        .lines()
        .rev()
        .filter_map(|line| {
            let trimmed = line.trim();
            (!trimmed.is_empty()).then_some(trimmed.to_string())
        })
        .take(max_lines)
        .collect::<Vec<_>>();
    recent.reverse();
    recent
}

fn is_interesting_error_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("error")
        || lower.contains("failed")
        || lower.contains("invalid")
        || lower.contains("unsupported")
        || lower.contains("authentication")
        || lower.contains("usage limit")
        || lower.contains("rate limit")
}

fn display_optional(value: Option<&str>) -> &str {
    value.unwrap_or("<none>")
}

fn completion_marker_visible(
    harness: &Harness,
    pane_id: &str,
    run_config: &ProviderRunConfig,
) -> Result<bool> {
    Ok(harness
        .capture_pane_history(pane_id)?
        .contains(&run_config.completion_marker))
}

impl Harness {
    fn new(agentscan_bin: &Path) -> Result<Self> {
        let tempdir = tempfile::tempdir().context("failed to create provider e2e tempdir")?;
        fs::set_permissions(tempdir.path(), fs::Permissions::from_mode(0o700))
            .with_context(|| format!("failed to chmod {}", tempdir.path().display()))?;

        let tmux_tmpdir = tempdir.path().join("tmux-tmp");
        fs::create_dir_all(&tmux_tmpdir)
            .with_context(|| format!("failed to create {}", tmux_tmpdir.display()))?;
        fs::set_permissions(&tmux_tmpdir, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("failed to chmod {}", tmux_tmpdir.display()))?;

        Ok(Self {
            tmux_socket_path: tempdir.path().join("tmux.sock"),
            agentscan_socket_path: tempdir.path().join("agentscan.sock"),
            agentscan_home: tempdir.path().join("agentscan-home"),
            agentscan_cache_home: tempdir.path().join("agentscan-cache"),
            agentscan_config_home: tempdir.path().join("agentscan-config"),
            tmux_tmpdir,
            agentscan_bin: agentscan_bin.to_path_buf(),
            tempdir,
        })
    }

    fn tmux_command(&self) -> Command {
        let mut command = Command::new("tmux");
        command.arg("-S").arg(&self.tmux_socket_path);
        command.env_remove("TMUX");
        command.env("TMUX_TMPDIR", &self.tmux_tmpdir);
        // Do not override HOME/XDG here: provider CLIs launched inside this tmux server must
        // inherit the user's normal auth state. Only agentscan itself is isolated below.
        command
    }

    fn agentscan_command(&self) -> Command {
        let mut command = Command::new(&self.agentscan_bin);
        command.env_remove("TMUX");
        command.env("TMUX_TMPDIR", &self.tmux_tmpdir);
        command.env(AGENTSCAN_TMUX_SOCKET_ENV_VAR, &self.tmux_socket_path);
        command.env("AGENTSCAN_SOCKET_PATH", &self.agentscan_socket_path);
        command.env(
            "AGENTSCAN_CONTROL_MODE_ACTIVE_RECONCILE_INTERVAL_MS",
            "1000",
        );
        command.env("AGENTSCAN_CONTROL_MODE_SELF_HEAL_INTERVAL_MS", "1000");
        command.env("HOME", &self.agentscan_home);
        command.env("XDG_CACHE_HOME", &self.agentscan_cache_home);
        command.env("XDG_CONFIG_HOME", &self.agentscan_config_home);
        command
    }

    fn agentscan_output<I, S>(&self, args: I) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut command = self.agentscan_command();
        for arg in args {
            command.arg(arg.as_ref());
        }
        let output = command
            .output()
            .context("failed to execute agentscan command")?;
        if !output.status.success() {
            bail!(
                "agentscan command failed with status {}\nstdout:\n{}\nstderr:\n{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        String::from_utf8(output.stdout).context("agentscan stdout was not valid UTF-8")
    }

    fn start_session(&self, session_name: &str, command: &str) -> Result<String> {
        let status = self
            .tmux_command()
            .arg("-f")
            .arg("/dev/null")
            .args([
                "new-session",
                "-d",
                "-x",
                "120",
                "-y",
                "40",
                "-s",
                session_name,
                command,
            ])
            .status()
            .with_context(|| format!("failed to start tmux session {session_name}"))?;
        if !status.success() {
            bail!("tmux new-session failed with status {status}");
        }

        Ok(self
            .tmux_output([
                "display-message",
                "-p",
                "-t",
                &format!("{session_name}:0.0"),
                "#{pane_id}",
            ])?
            .trim()
            .to_string())
    }

    fn start_daemon(&self, artifact_dir: &Path) -> Result<DaemonHandle> {
        let stdout_path = artifact_dir.join("daemon.stdout.log");
        let stderr_path = artifact_dir.join("daemon.stderr.log");
        let stdout = File::create(&stdout_path)
            .with_context(|| format!("failed to create {}", stdout_path.display()))?;
        let stderr = File::create(&stderr_path)
            .with_context(|| format!("failed to create {}", stderr_path.display()))?;

        let child = self
            .agentscan_command()
            .args(["daemon", "run"])
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .context("failed to start agentscan daemon")?;
        Ok(DaemonHandle { child })
    }

    fn wait_for_daemon_ready(&self, daemon: &mut DaemonHandle, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        let mut last_error: String;
        loop {
            daemon.ensure_running()?;
            match self.agentscan_output(["snapshot", "--format", "json", "--no-auto-start"]) {
                Ok(_) => return Ok(()),
                Err(error) => last_error = format!("{error:#}"),
            }
            if Instant::now() >= deadline {
                bail!("timed out waiting for daemon snapshot; last error: {last_error}");
            }
            thread::sleep(POLL_INTERVAL);
        }
    }

    fn start_subscribe(&self, artifact_dir: &Path) -> Result<SubscribeHandle> {
        let stderr_path = artifact_dir.join("subscribe.stderr.log");
        let stderr = File::create(&stderr_path)
            .with_context(|| format!("failed to create {}", stderr_path.display()))?;

        let mut child = self.agentscan_command();
        child
            .args(["subscribe", "--format", "json", "--no-auto-start"])
            .stdout(Stdio::piped())
            .stderr(Stdio::from(stderr));
        let mut child = child
            .spawn()
            .context("failed to start agentscan subscribe")?;
        let stdout = child
            .stdout
            .take()
            .context("subscribe child did not expose stdout")?;
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else {
                    break;
                };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(SubscribeHandle { child, rx })
    }

    fn send_agent_prompt(
        &self,
        pane_id: &str,
        pre_prompt_keys: &[String],
        prompt: &str,
        submit_keys: &[String],
    ) -> Result<()> {
        if !pre_prompt_keys.is_empty() {
            self.send_keys(pane_id, pre_prompt_keys)?;
        }
        self.send_literal_text(pane_id, prompt)?;
        thread::sleep(Duration::from_millis(500));
        self.send_keys(pane_id, submit_keys)
    }

    fn apply_startup_keys_if_needed(
        &self,
        pane_id: &str,
        run_config: &ProviderRunConfig,
    ) -> Result<()> {
        for step in &run_config.provider.startup_steps {
            if let Some(wait_text) = step.wait_text.as_deref()
                && !self.wait_for_pane_text(pane_id, wait_text, step.timeout)?
            {
                if step.optional {
                    continue;
                }
                bail!("timed out waiting for pane {pane_id} to contain `{wait_text}`");
            }

            if !step.keys.is_empty() {
                self.send_keys(pane_id, &step.keys)?;
            }
        }

        if run_config.provider.startup_keys.is_empty() {
            return Ok(());
        }

        if let Some(wait_text) = run_config.provider.startup_wait_text.as_deref()
            && !self.wait_for_pane_text(pane_id, wait_text, run_config.provider.startup_timeout)?
        {
            bail!("timed out waiting for pane {pane_id} to contain `{wait_text}`");
        }

        self.send_keys(pane_id, &run_config.provider.startup_keys)
    }

    fn wait_for_pane_text(&self, pane_id: &str, text: &str, timeout: Duration) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            let contents = self.capture_pane_tail(pane_id, 200)?;
            if contents.contains(text) {
                return Ok(true);
            }

            if Instant::now() >= deadline {
                return Ok(false);
            }

            thread::sleep(POLL_INTERVAL);
        }
    }

    fn send_keys(&self, pane_id: &str, keys: &[String]) -> Result<()> {
        let mut command = self.tmux_command();
        command.args(["send-keys", "-t", pane_id]);
        for key in keys {
            command.arg(key);
        }
        let status = command.status().context("failed to send tmux keys")?;
        if !status.success() {
            bail!("tmux send-keys failed with status {status}");
        }
        Ok(())
    }

    fn send_literal_text(&self, pane_id: &str, text: &str) -> Result<()> {
        let status = self
            .tmux_command()
            .args(["send-keys", "-l", "-t", pane_id, text])
            .status()
            .context("failed to send tmux literal text")?;
        if !status.success() {
            bail!("tmux send-keys -l failed with status {status}");
        }
        Ok(())
    }

    fn capture_pane_tail(&self, pane_id: &str, lines: usize) -> Result<String> {
        let start = format!("-{lines}");
        self.tmux_output(["capture-pane", "-p", "-S", &start, "-t", pane_id])
    }

    fn capture_pane_history(&self, pane_id: &str) -> Result<String> {
        self.tmux_output(["capture-pane", "-p", "-S", "-", "-t", pane_id])
    }

    fn list_panes_raw(&self) -> Result<String> {
        self.tmux_output([
            "list-panes",
            "-a",
            "-F",
            "#{session_name}\t#{window_index}\t#{pane_index}\t#{pane_id}\t#{pane_pid}\t#{pane_current_command}\t#{pane_title}\t#{pane_current_path}",
        ])
    }

    fn tmux_output<I, S>(&self, args: I) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut command = self.tmux_command();
        for arg in args {
            command.arg(arg.as_ref());
        }
        let output = command.output().context("failed to execute tmux command")?;
        if !output.status.success() {
            bail!(
                "tmux command failed with status {}\nstderr:\n{}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        String::from_utf8(output.stdout).context("tmux stdout was not valid UTF-8")
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .arg("-S")
            .arg(&self.tmux_socket_path)
            .arg("kill-server")
            .env_remove("TMUX")
            .env("TMUX_TMPDIR", &self.tmux_tmpdir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

impl DaemonHandle {
    fn ensure_running(&mut self) -> Result<()> {
        if let Some(status) = self
            .child
            .try_wait()
            .context("failed to poll daemon child")?
        {
            bail!("agentscan daemon exited unexpectedly with status {status}");
        }
        Ok(())
    }

    fn shutdown(&mut self) -> Result<()> {
        if self.child.try_wait()?.is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        Ok(())
    }
}

impl Drop for DaemonHandle {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

impl SubscribeHandle {
    fn recv_line(&mut self, timeout: Duration) -> Result<Option<String>> {
        match self.rx.recv_timeout(timeout) {
            Ok(line) => Ok(Some(line)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => {
                self.ensure_running()?;
                bail!("subscribe stdout closed unexpectedly")
            }
        }
    }

    fn try_recv_line(&mut self) -> Result<Option<String>> {
        match self.rx.try_recv() {
            Ok(line) => Ok(Some(line)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                self.ensure_running()?;
                bail!("subscribe stdout closed unexpectedly")
            }
        }
    }

    fn ensure_running(&mut self) -> Result<()> {
        if let Some(status) = self
            .child
            .try_wait()
            .context("failed to poll subscribe child")?
        {
            bail!("agentscan subscribe exited unexpectedly with status {status}");
        }
        Ok(())
    }

    fn shutdown(&mut self) -> Result<()> {
        if self.child.try_wait()?.is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        Ok(())
    }
}

impl Drop for SubscribeHandle {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

impl TimelineRecorder {
    fn new(
        path: PathBuf,
        target_pane_id: String,
        expected_provider: String,
        expected_match_kinds: Vec<String>,
    ) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("failed to create {}", path.display()))?;
        Ok(Self {
            file,
            target_pane_id,
            expected_provider,
            expected_match_kinds,
            observations: Vec::new(),
            identity_locked: false,
            last_pane: None,
        })
    }

    fn record_line(&mut self, line: &str) -> Result<Option<Value>> {
        writeln!(self.file, "{line}").context("failed to append timeline frame")?;
        self.file
            .flush()
            .context("failed to flush timeline frame")?;

        let frame: Value = serde_json::from_str(line)
            .with_context(|| format!("subscribe frame was not JSON: {line}"))?;
        match frame["type"].as_str() {
            Some("fatal") => {
                let message = frame["message"].as_str().unwrap_or("fatal subscribe frame");
                bail!("agentscan subscribe emitted fatal frame: {message}");
            }
            Some("snapshot") => {
                let Some(pane) = pane_from_snapshot(&frame["snapshot"], &self.target_pane_id)
                else {
                    return Ok(None);
                };
                let pane = pane.clone();
                self.record_pane(&pane)?;
                self.last_pane = Some(pane.clone());
                Ok(Some(pane))
            }
            _ => Ok(None),
        }
    }

    fn record_snapshot_poll(&mut self, output: &str) -> Result<Option<Value>> {
        let snapshot: Value =
            serde_json::from_str(output).context("polled daemon snapshot was not JSON")?;
        let frame = serde_json::json!({
            "type": "snapshot_poll",
            "snapshot": snapshot,
        });
        writeln!(self.file, "{frame}").context("failed to append snapshot poll frame")?;
        self.file
            .flush()
            .context("failed to flush snapshot poll frame")?;

        let Some(pane) = pane_from_snapshot(&frame["snapshot"], &self.target_pane_id) else {
            return Ok(None);
        };
        let pane = pane.clone();
        self.record_pane(&pane)?;
        self.last_pane = Some(pane.clone());
        Ok(Some(pane))
    }

    fn record_pane(&mut self, pane: &Value) -> Result<()> {
        let provider = pane_provider(pane);
        let status_kind = pane_status_kind(pane);
        let status_source = pane_status_source(pane);
        let matched_by = pane_matched_by(pane);
        let display_label = pane
            .pointer("/display/label")
            .and_then(Value::as_str)
            .map(ToString::to_string);

        if let Some(provider) = provider.as_deref() {
            if provider != self.expected_provider {
                bail!(
                    "target pane provider changed to `{provider}`, expected `{}`",
                    self.expected_provider
                );
            }
            if !self.identity_locked {
                self.validate_match_kind(matched_by.as_deref())?;
                self.identity_locked = true;
            }
        } else if self.identity_locked {
            bail!(
                "target pane lost provider identity after being detected as `{}`",
                self.expected_provider
            );
        }

        self.observations.push(PaneObservation {
            index: self.observations.len(),
            provider,
            status_kind,
            status_source,
            matched_by,
            display_label,
        });
        Ok(())
    }

    fn validate_match_kind(&self, matched_by: Option<&str>) -> Result<()> {
        if self.expected_match_kinds.is_empty() {
            return Ok(());
        }
        let Some(matched_by) = matched_by else {
            bail!(
                "provider detected without classification matched_by; expected one of {:?}",
                self.expected_match_kinds
            );
        };
        if !self
            .expected_match_kinds
            .iter()
            .any(|expected| expected == matched_by)
        {
            bail!(
                "provider detected by `{matched_by}`, expected one of {:?}",
                self.expected_match_kinds
            );
        }
        Ok(())
    }
}

fn pane_from_snapshot<'a>(snapshot: &'a Value, pane_id: &str) -> Option<&'a Value> {
    snapshot["panes"]
        .as_array()?
        .iter()
        .find(|pane| pane["pane_id"].as_str() == Some(pane_id))
}

fn pane_provider(pane: &Value) -> Option<String> {
    pane["provider"].as_str().map(ToString::to_string)
}

fn pane_status_kind(pane: &Value) -> Option<String> {
    pane.pointer("/status/kind")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn pane_status_source(pane: &Value) -> Option<String> {
    pane.pointer("/status/source")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn pane_matched_by(pane: &Value) -> Option<String> {
    pane.pointer("/classification/matched_by")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn prepare_workspace(harness: &Harness, provider: &str) -> Result<PathBuf> {
    let workspace = harness
        .tempdir
        .path()
        .join("workspaces")
        .join(sanitize_identifier(provider));
    fs::create_dir_all(&workspace)
        .with_context(|| format!("failed to create {}", workspace.display()))?;
    fs::write(
        workspace.join("README.md"),
        "# agentscan provider e2e\n\nThis temporary workspace is created by the local provider e2e runner.\n",
    )
    .context("failed to write e2e README")?;
    let _ = Command::new("git")
        .arg("init")
        .current_dir(&workspace)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    Ok(workspace)
}

fn agent_launch_command(
    run_config: &ProviderRunConfig,
    workspace: &Path,
    run_id: &str,
    redact_env_values: bool,
) -> String {
    let provider = &run_config.provider;
    let command = expand_template(
        &provider.command,
        provider,
        &run_config.model,
        &run_config.effort,
        workspace,
        &run_config.completion_marker,
        run_id,
    );
    let args = provider
        .args
        .iter()
        .map(|arg| {
            expand_template(
                arg,
                provider,
                &run_config.model,
                &run_config.effort,
                workspace,
                &run_config.completion_marker,
                run_id,
            )
        })
        .collect::<Vec<_>>();
    let envs = provider
        .env
        .iter()
        .map(|(key, value)| {
            let value = if redact_env_values {
                "<redacted>".to_string()
            } else {
                expand_template(
                    value,
                    provider,
                    &run_config.model,
                    &run_config.effort,
                    workspace,
                    &run_config.completion_marker,
                    run_id,
                )
            };
            format!("{}={}", shell_quote(key), shell_quote(&value))
        })
        .collect::<Vec<_>>();

    let mut parts = vec![
        "cd".to_string(),
        shell_quote_path(workspace),
        "&&".to_string(),
        "exec".to_string(),
    ];
    if !envs.is_empty() {
        parts.push("env".to_string());
        parts.extend(envs);
    }
    parts.push(shell_quote(&command));
    parts.extend(args.iter().map(|arg| shell_quote(arg)));
    parts.join(" ")
}

fn expand_template(
    value: &str,
    provider: &ProviderConfig,
    model: &str,
    effort: &str,
    workspace: &Path,
    completion_marker: &str,
    run_id: &str,
) -> String {
    value
        .replace("{provider}", &provider.name)
        .replace("{model}", model)
        .replace("{effort}", effort)
        .replace("{workspace}", &workspace.display().to_string())
        .replace("{completion_marker}", completion_marker)
        .replace("{run_id}", run_id)
}

fn write_metadata(
    artifact_dir: &Path,
    run_config: &ProviderRunConfig,
    prompt_submitted: bool,
    pane_id: &str,
    workspace: &Path,
    harness: &Harness,
) -> Result<()> {
    let env_keys = run_config.provider.env.keys().collect::<Vec<_>>();
    let metadata = RunMetadata {
        provider: &run_config.provider.name,
        expected_provider: &run_config.provider.expected_provider,
        model: &run_config.model,
        effort: &run_config.effort,
        prompt_submitted,
        completion_marker: &run_config.completion_marker,
        command: &run_config.provider.command,
        args: &run_config.provider.args,
        env_keys,
        pane_id,
        workspace,
        tmux_socket: &harness.tmux_socket_path,
        agentscan_socket: &harness.agentscan_socket_path,
    };
    let bytes = serde_json::to_vec_pretty(&metadata).context("failed to encode run metadata")?;
    fs::write(artifact_dir.join("runner-metadata.json"), bytes)
        .context("failed to write run metadata")
}

fn collect_common_artifacts(
    harness: &Harness,
    artifact_dir: &Path,
    pane_id: &str,
    timeline: &TimelineRecorder,
) -> Result<()> {
    if let Some(pane) = &timeline.last_pane {
        fs::write(
            artifact_dir.join("last-pane.json"),
            serde_json::to_vec_pretty(pane).context("failed to encode last pane")?,
        )
        .context("failed to write last-pane artifact")?;
    }
    fs::write(
        artifact_dir.join("observations.json"),
        serde_json::to_vec_pretty(&timeline.observations)
            .context("failed to encode observations")?,
    )
    .context("failed to write observations artifact")?;
    write_text_artifact(
        artifact_dir.join("tmux-list-panes.txt"),
        &harness
            .list_panes_raw()
            .unwrap_or_else(|error| format!("{error:#}")),
    )?;
    write_text_artifact(
        artifact_dir.join("pane-tail-final.txt"),
        &harness
            .capture_pane_tail(pane_id, 200)
            .unwrap_or_else(|error| format!("{error:#}")),
    )?;
    Ok(())
}

fn repo_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn write_command_output(path: PathBuf, output: Result<String>) {
    let text = output.unwrap_or_else(|error| format!("{error:#}\n"));
    let _ = fs::write(path, text);
}

fn write_text_artifact(path: PathBuf, text: &str) -> Result<()> {
    fs::write(&path, text).with_context(|| format!("failed to write {}", path.display()))
}

fn resolve_agentscan_bin(override_path: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = override_path {
        if path.is_file() {
            return Ok(path.to_path_buf());
        }
        bail!("--agentscan-bin path does not exist: {}", path.display());
    }
    if let Some(path) = std::env::var_os("AGENTSCAN_E2E_BIN").map(PathBuf::from) {
        if path.is_file() {
            return Ok(path);
        }
        bail!("AGENTSCAN_E2E_BIN path does not exist: {}", path.display());
    }

    let current_exe = std::env::current_exe().context("failed to resolve current executable")?;
    let profile_dir = current_exe
        .parent()
        .and_then(Path::parent)
        .with_context(|| {
            format!(
                "failed to derive Cargo profile directory from {}",
                current_exe.display()
            )
        })?;
    let candidate = profile_dir.join(format!("agentscan{}", std::env::consts::EXE_SUFFIX));
    if candidate.is_file() {
        return Ok(candidate);
    }

    let mut build_args = vec!["build"];
    let release_profile = profile_dir
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "release");
    if release_profile {
        build_args.push("--release");
    }
    build_args.extend(["--bin", "agentscan"]);

    let status = Command::new("cargo")
        .args(&build_args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .status()
        .with_context(|| format!("failed to run cargo {}", build_args.join(" ")))?;
    if !status.success() {
        bail!("cargo {} failed with status {status}", build_args.join(" "));
    }
    if candidate.is_file() {
        return Ok(candidate);
    }
    bail!(
        "failed to find agentscan binary at {}; pass --agentscan-bin",
        candidate.display()
    );
}

fn normalize_provider_name(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace('-', "_")
}

fn sanitize_identifier(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn new_run_id() -> Result<String> {
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time is before unix epoch")?
        .as_secs();
    Ok(format!("{}_{}", epoch, std::process::id()))
}

fn shell_quote_path(path: &Path) -> String {
    shell_quote(&path.display().to_string())
}

fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | ':' | '='))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_and_marker_templates_resolve_with_workspace() {
        let provider = ProviderConfig::from_patch(
            "codex",
            ProviderPatch {
                command: Some("codex".to_string()),
                prompt: Some("use {workspace} then print {completion_marker}".to_string()),
                completion_marker: Some("done-{workspace}-{run_id}".to_string()),
                ..ProviderPatch::default()
            },
        )
        .expect("provider config should be valid");
        let run_config =
            provider_run_config(provider, None, None, None).expect("run config should be valid");
        let workspace = Path::new("/tmp/agentscan e2e workspace");

        let resolved = run_config.resolve_for_workspace(workspace, "run-123");

        assert_eq!(
            resolved.completion_marker,
            "done-/tmp/agentscan e2e workspace-run-123"
        );
        assert_eq!(
            resolved.prompt,
            "use /tmp/agentscan e2e workspace then print done-/tmp/agentscan e2e workspace-run-123"
        );
    }
}
