use std::{
    env,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use crate::commands::short_host_label;
use crate::contract::{
    PickerRow, PickerRowsEnvelope, picker_rows_from_envelope, validate_picker_rows,
};

const PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(2);
// Diagnostic-only probe that runs an interactive remote login shell (sources
// rc files), so it gets a larger budget than the bare `--version` preflight. It
// runs at most once, on an SSH preflight that already failed as binary-not-found.
const REMOTE_PROBE_TIMEOUT: Duration = Duration::from_secs(8);
const HOTKEYS_TIMEOUT: Duration = Duration::from_secs(5);
const FOCUS_TIMEOUT: Duration = Duration::from_secs(5);
const DAEMON_STATUS_TIMEOUT: Duration = Duration::from_secs(2);
// Grace period to let a subscribe child exit on its own after its stdout signals
// termination (EOF or a terminal frame) before we kill it, so a child that
// lingers can't park the worker thread on an unbounded wait().
pub(crate) const LIVE_CHILD_EXIT_GRACE: Duration = Duration::from_millis(500);
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentscanPreflight {
    binary: String,
    ok: bool,
    version: Option<String>,
    error: Option<String>,
    // An absolute remote path the desktop can offer as a one-click fix when a
    // remote preflight fails because `agentscan` isn't on the SSH PATH but the
    // user's own shell can find it (see classify_preflight_failure). `None` for
    // success, local runners, and unresolvable failures.
    suggested_binary_path: Option<String>,
    // The remote machine's short hostname, probed inside the same SSH exec as the
    // version check (see remote_preflight_sh_script) so a successful remote
    // preflight can upgrade the source label from the configured host string.
    // `None` for local runners, failures, and when the remote `hostname` yields
    // nothing.
    remote_host_label: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocalRunnerSettings {
    pub(crate) binary_path: Option<String>,
    #[serde(default)]
    pub(crate) env: Vec<LocalEnvironmentVariable>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocalEnvironmentVariable {
    name: String,
    value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum DesktopRunnerSettings {
    Local {
        binary_path: Option<String>,
        #[serde(default)]
        env: Vec<LocalEnvironmentVariable>,
    },
    Ssh {
        host: String,
        client_tty: Option<String>,
        binary_path: Option<String>,
        #[serde(default)]
        env: Vec<LocalEnvironmentVariable>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AgentscanRunner {
    Local(LocalRunnerSettings),
    Ssh(SshRunnerSettings),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SshRunnerSettings {
    host: String,
    client_tty: Option<String>,
    binary_path: Option<String>,
    env: Vec<LocalEnvironmentVariable>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CommandOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}
fn agentscan_binary() -> OsString {
    env::var_os("AGENTSCAN_DESKTOP_AGENTSCAN_BIN")
        .or_else(|| find_known_agentscan_binary().map(PathBuf::into_os_string))
        .unwrap_or_else(|| OsString::from("agentscan"))
}

fn agentscan_binary_for_settings(settings: &LocalRunnerSettings) -> OsString {
    settings
        .binary_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(OsString::from)
        .unwrap_or_else(agentscan_binary)
}

// A directory where agentscan commonly lives but a non-interactive shell's PATH
// often omits.
enum AgentscanBinDir {
    // Resolved against `$HOME` (the local home for auto-detect, or `$HOME` in the
    // remote shell for the SSH PATH).
    Home(&'static str),
    // An absolute path.
    Abs(&'static str),
}

// Concrete install dirs in precedence order: a real `agentscan` binary lives
// directly in one of these. The original GUI-launch dirs come first (cargo,
// Homebrew, /usr/local/bin — unchanged), then `~/.local/bin`.
const AGENTSCAN_BIN_DIRS: &[AgentscanBinDir] = &[
    AgentscanBinDir::Home(".cargo/bin"),
    AgentscanBinDir::Abs("/opt/homebrew/bin"),
    AgentscanBinDir::Abs("/usr/local/bin"),
    AgentscanBinDir::Home(".local/bin"),
];

// Version-manager shim dirs. Shims are thin wrappers (mise/asdf symlinks), so a
// stale shim or an unavailable manager must never shadow a real binary. They are
// tried LAST everywhere: on the SSH PATH they are appended after both `$PATH` and
// the concrete dirs (remote_path_sh_script); in local auto-detect they are tried
// only after the concrete dirs *and* an explicit PATH lookup
// (resolve_local_agentscan), so an `agentscan` already resolvable via PATH always
// wins over a leftover shim.
const AGENTSCAN_SHIM_DIRS: &[AgentscanBinDir] = &[
    AgentscanBinDir::Home(".local/share/mise/shims"),
    AgentscanBinDir::Home(".asdf/shims"),
];

fn find_known_agentscan_binary() -> Option<PathBuf> {
    resolve_local_agentscan(
        env::var_os("HOME").as_deref(),
        env::var_os("PATH").as_deref(),
        is_executable_file,
    )
}

// A regular file with at least one execute bit. The PATH scan in
// resolve_local_agentscan needs this rather than a bare is_file so a
// non-executable `agentscan` stub on an early PATH entry can't shadow a real
// executable later on PATH — matching how the OS resolves a bare command name.
// Desktop builds target unix only (macOS release, Linux CI), so the unix
// permission check is safe.
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

// Local auto-detect precedence: concrete install dirs, then an explicit PATH
// lookup, then version-manager shims. The PATH step sits before shims so a real
// binary on the inherited PATH beats a stale shim, while shims still rescue the
// GUI-from-Finder case where PATH is minimal and agentscan is only installed via
// mise/asdf. Only executable files match (`is_executable`), so a non-executable
// entry is skipped just as the OS would. `is_executable` is injected so the
// precedence is unit-testable without touching the filesystem.
fn resolve_local_agentscan<F>(
    home: Option<&OsStr>,
    path_var: Option<&OsStr>,
    is_executable: F,
) -> Option<PathBuf>
where
    F: Fn(&Path) -> bool,
{
    agentscan_paths_in(AGENTSCAN_BIN_DIRS, home)
        .find(|path| is_executable(path.as_path()))
        .or_else(|| {
            path_var.and_then(|path_var| {
                env::split_paths(path_var)
                    .map(|dir| dir.join("agentscan"))
                    .find(|candidate| is_executable(candidate.as_path()))
            })
        })
        .or_else(|| {
            agentscan_paths_in(AGENTSCAN_SHIM_DIRS, home).find(|path| is_executable(path.as_path()))
        })
}

fn agentscan_paths_in(
    dirs: &'static [AgentscanBinDir],
    home: Option<&OsStr>,
) -> impl Iterator<Item = PathBuf> {
    let home = home
        .filter(|home| !home.is_empty())
        .map(|home| Path::new(home).to_owned());

    dirs.iter().filter_map(move |dir| match dir {
        AgentscanBinDir::Home(rel) => home.as_ref().map(|home| home.join(rel).join("agentscan")),
        AgentscanBinDir::Abs(abs) => Some(Path::new(abs).join("agentscan")),
    })
}

impl AgentscanRunner {
    pub(crate) fn from_settings(settings: Option<DesktopRunnerSettings>) -> Self {
        match settings {
            Some(DesktopRunnerSettings::Local { binary_path, env }) => {
                Self::Local(LocalRunnerSettings { binary_path, env })
            }
            Some(DesktopRunnerSettings::Ssh {
                host,
                client_tty,
                binary_path,
                env,
            }) => Self::Ssh(SshRunnerSettings {
                host: host.trim().to_owned(),
                client_tty: client_tty
                    .as_deref()
                    .map(str::trim)
                    .filter(|tty| !tty.is_empty())
                    .map(str::to_owned),
                binary_path,
                env,
            }),
            None => Self::Local(LocalRunnerSettings::default()),
        }
    }

    fn display_binary(&self) -> String {
        match self {
            Self::Local(settings) => agentscan_binary_for_settings(settings)
                .to_string_lossy()
                .into_owned(),
            Self::Ssh(settings) => {
                let binary = remote_agentscan_binary_for_settings(settings);
                format!("ssh {} -- {binary}", settings.host)
            }
        }
    }
}

fn remote_agentscan_binary_for_settings(settings: &SshRunnerSettings) -> String {
    settings
        .binary_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .unwrap_or("agentscan")
        .to_owned()
}

#[cfg(test)]
fn run_agentscan_preflight(binary: OsString) -> AgentscanPreflight {
    run_agentscan_preflight_with_timeout(binary, PREFLIGHT_TIMEOUT)
}

pub(crate) fn run_agentscan_preflight_with_runner(runner: &AgentscanRunner) -> AgentscanPreflight {
    let binary_display = runner.display_binary();

    let result = agentscan_preflight_command(runner)
        .and_then(|mut command| run_command_with_timeout(&mut command, PREFLIGHT_TIMEOUT));
    match result {
        Ok(output) if output.status.success() => {
            let (remote_host_label, version_output) =
                split_remote_host_marker(&String::from_utf8_lossy(&output.stdout));
            AgentscanPreflight {
                binary: binary_display,
                ok: true,
                version: Some(version_output.trim().to_owned()),
                error: None,
                suggested_binary_path: None,
                remote_host_label,
            }
        }
        Ok(output) => {
            let raw = stderr_or_status("agentscan", &output.stderr, output.status);
            let failure = classify_preflight_failure(runner, &raw);
            AgentscanPreflight {
                binary: binary_display,
                ok: false,
                version: None,
                error: Some(failure.message),
                suggested_binary_path: failure.suggested_binary_path,
                remote_host_label: None,
            }
        }
        Err(error) => {
            let failure = classify_preflight_failure(runner, &error);
            AgentscanPreflight {
                binary: binary_display,
                ok: false,
                version: None,
                error: Some(failure.message),
                suggested_binary_path: failure.suggested_binary_path,
                remote_host_label: None,
            }
        }
    }
}

// Split the host-probe marker line out of preflight stdout: the probed short
// hostname (None when the marker is absent or its value is empty) plus the
// remaining lines, which feed the existing version parsing. Local preflights
// never print the marker, so they fall through to (None, stdout).
fn split_remote_host_marker(stdout: &str) -> (Option<String>, String) {
    let mut label = None;
    let mut rest = Vec::new();
    for line in stdout.lines() {
        match line.strip_prefix(REMOTE_HOST_MARKER) {
            Some(value) => {
                let value = short_host_label(value.trim());
                if !value.is_empty() {
                    label = Some(value.to_owned());
                }
            }
            None => rest.push(line),
        }
    }
    (label, rest.join("\n"))
}

#[cfg(test)]
fn run_agentscan_preflight_with_timeout(binary: OsString, timeout: Duration) -> AgentscanPreflight {
    let binary_display = binary.to_string_lossy().into_owned();

    match run_agentscan_binary_command(&binary, ["--version"], timeout) {
        Ok(output) if output.status.success() => AgentscanPreflight {
            binary: binary_display,
            ok: true,
            version: Some(String::from_utf8_lossy(&output.stdout).trim().to_owned()),
            error: None,
            suggested_binary_path: None,
            remote_host_label: None,
        },
        Ok(output) => AgentscanPreflight {
            binary: binary_display,
            ok: false,
            version: None,
            error: Some(stderr_or_status("agentscan", &output.stderr, output.status)),
            suggested_binary_path: None,
            remote_host_label: None,
        },
        Err(error) => AgentscanPreflight {
            binary: binary_display,
            ok: false,
            version: None,
            error: Some(error.to_string()),
            suggested_binary_path: None,
            remote_host_label: None,
        },
    }
}

// Argv for the desktop's picker-row fetch. Always latch-only: the desktop never
// spawns a daemon from a row fetch — only the explicit "Start agentscan" subscribe
// may auto-start — so `--no-auto-start` is unconditional here. Without it, a daemon
// that exits between a subscribe snapshot and this fetch would be silently replaced
// by `hotkeys`' consumer auto-start, violating the latch-only policy
// (see docs/adr/desktop-latch-only-daemon-launch.md).
//
// This adds no new CLI version floor: `--no-auto-start` is the shared `AutoStartArgs`
// flattened onto both `subscribe` and `hotkeys` (introduced together), and the default
// latch flow already runs `subscribe --no-auto-start` before any row fetch. A binary too
// old to accept the flag therefore fails at subscribe first, not because of this argv —
// the latch-only desktop already requires `--no-auto-start` support regardless of hotkeys.
fn hotkeys_args() -> Vec<&'static str> {
    vec!["hotkeys", "--format", "json", "--no-auto-start"]
}

pub(crate) fn load_picker_rows_with_runner(
    runner: &AgentscanRunner,
) -> Result<Vec<PickerRow>, String> {
    load_picker_rows_from_runner(runner)
}

fn load_picker_rows_from_runner(runner: &AgentscanRunner) -> Result<Vec<PickerRow>, String> {
    load_picker_rows_from_runner_interruptible(runner, None)
}

fn load_picker_rows_from_runner_interruptible(
    runner: &AgentscanRunner,
    stop: Option<&AtomicBool>,
) -> Result<Vec<PickerRow>, String> {
    let mut command = agentscan_command(runner, &hotkeys_args())
        .map_err(|error| classify_desktop_failure(runner, "hotkeys", &error))?;
    let output = run_command_with_timeout_interruptible(&mut command, HOTKEYS_TIMEOUT, stop)
        .map_err(|error| {
            classify_desktop_failure(
                runner,
                "hotkeys",
                &format!("Unable to run agentscan hotkeys: {error}"),
            )
        })?;

    if !output.status.success() {
        let error = stderr_or_status("agentscan hotkeys", &output.stderr, output.status);
        return Err(classify_desktop_failure(runner, "hotkeys", &error));
    }

    let envelope: PickerRowsEnvelope = serde_json::from_slice(&output.stdout).map_err(|error| {
        classify_desktop_failure(
            runner,
            "hotkeys",
            &format!("Invalid agentscan hotkeys JSON: {error}"),
        )
    })?;
    let rows = picker_rows_from_envelope(runner, envelope)?;
    validate_picker_rows(&rows)
        .map_err(|error| classify_desktop_failure(runner, "hotkeys", &error))?;
    Ok(rows)
}

// Unwrap a picker-rows envelope after checking its schema version. An unexpected
// version means the host CLI changed the row shape under us, so treat it as an
// incompatible-binary failure (upgrade guidance) rather than trusting the rows.
pub(crate) fn focus_picker_row_with_runner(
    runner: &AgentscanRunner,
    pane_id: &str,
) -> Result<(), String> {
    focus_picker_row_with_runner_and_timeout(runner, pane_id, FOCUS_TIMEOUT)
}

#[cfg(test)]
fn focus_picker_row_with_binary(binary: OsString, pane_id: &str) -> Result<(), String> {
    focus_picker_row_with_runner(
        &AgentscanRunner::Local(LocalRunnerSettings {
            binary_path: Some(binary.to_string_lossy().into_owned()),
            env: Vec::new(),
        }),
        pane_id,
    )
}

pub(crate) fn focus_picker_row_with_runner_and_timeout(
    runner: &AgentscanRunner,
    pane_id: &str,
    timeout: Duration,
) -> Result<(), String> {
    if pane_id.trim().is_empty() {
        return Err("Cannot focus an empty pane id".to_owned());
    }

    let args = focus_args_for_runner(runner, pane_id)?;
    let output = run_agentscan_command(runner, &args, timeout).map_err(|error| {
        classify_desktop_failure(
            runner,
            "focus",
            &format!("Unable to run agentscan focus: {error}"),
        )
    })?;

    if output.status.success() {
        Ok(())
    } else {
        let error = stderr_or_status("agentscan focus", &output.stderr, output.status);
        Err(classify_desktop_failure(runner, "focus", &error))
    }
}

fn focus_args_for_runner<'a>(
    runner: &'a AgentscanRunner,
    pane_id: &'a str,
) -> Result<Vec<&'a str>, String> {
    let mut args = vec!["focus"];
    if let AgentscanRunner::Ssh(settings) = runner
        && let Some(client_tty) = settings.client_tty.as_deref()
    {
        validate_client_tty(client_tty)
            .map_err(|error| classify_desktop_failure(runner, "focus", &error))?;
        args.push("--client-tty");
        args.push(client_tty);
    }
    args.push(pane_id);
    Ok(args)
}

// Per-key stale-start gate: honor a start only when its epoch advances past the
// highest epoch already honored for that source key — and past the fence floor,
// which stands in for evicted keys. Keys gate independently, so one source's
// stale start can never block — or tear down — another's worker.
//
// The floor fallback is gate-equivalent for evicted keys, never weaker: a worker
// running at epoch E means E was committed as that key's entry (per-key entries
// are monotone), and an absent entry means it was evicted — eviction takes only
// the map minimum and raises the floor to at least that value, so floor >= E
// whenever a running key lacks an entry. `epoch > floor` then admits exactly the
// strictly-newer starts the entry would have admitted; no superseded start can
// slip between the floor and a running worker (see commit_start_epoch).
pub(crate) fn load_daemon_status(runner: &AgentscanRunner) -> Result<serde_json::Value, String> {
    let output = run_agentscan_command(
        runner,
        &["daemon", "status", "--format", "json"],
        DAEMON_STATUS_TIMEOUT,
    )
    .map_err(|error| {
        classify_desktop_failure(
            runner,
            "daemon status",
            &format!("Unable to run agentscan daemon status: {error}"),
        )
    })?;

    if !output.status.success() {
        let error = stderr_or_status("agentscan daemon status", &output.stderr, output.status);
        return Err(classify_desktop_failure(runner, "daemon status", &error));
    }

    serde_json::from_slice(&output.stdout).map_err(|error| {
        classify_desktop_failure(
            runner,
            "daemon status",
            &format!("Invalid agentscan daemon status JSON: {error}"),
        )
    })
}

// Reachability result for the AUR-518 latch poll: whether a daemon is present
// enough to escalate to a full subscribe re-arm.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DaemonPollResult {
    reachable: bool,
}

// The only confident "no daemon" signal is daemon_state == "not_running" (the core
// emits that with exit 0 — see daemon/lifecycle.rs). Any live state
// (ready/initializing/startup_failed/closing) or an unexpected/missing field counts
// as reachable, so we escalate to a full subscribe rather than silently cheap-poll
// forever; a command that failed outright (incompatible/busy/SSH/timeout) never
// reaches here — load_daemon_status returns Err for those.
fn daemon_status_reachable(status: &serde_json::Value) -> bool {
    status
        .get("daemon_state")
        .and_then(serde_json::Value::as_str)
        != Some("not_running")
}

// Cheap latch poll: run `agentscan daemon status --format json` and report whether a
// daemon is reachable. An Err (incompatible/busy/SSH/timeout) propagates so the
// frontend escalates to a full subscribe, matching the pre-AUR-518 behavior.
pub(crate) fn poll_daemon_status_with_runner(
    runner: &AgentscanRunner,
) -> Result<DaemonPollResult, String> {
    let status = load_daemon_status(runner)?;
    Ok(DaemonPollResult {
        reachable: daemon_status_reachable(&status),
    })
}

// Render collected stderr bytes into a compact message, dropping blank lines.
// Takes already-buffered bytes (from a pipe collector) so partial diagnostics
// survive even when the pipe never reaches EOF because a descendant holds it.
pub(crate) fn classify_desktop_failure(
    runner: &AgentscanRunner,
    operation: &str,
    message: &str,
) -> String {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return format!("agentscan {operation} failed");
    }

    let lower = trimmed.to_lowercase();

    if matches!(runner, AgentscanRunner::Ssh(_)) {
        if lower.contains("permission denied")
            || lower.contains("publickey")
            || lower.contains("authentication failed")
        {
            return format!("SSH authentication failed: {trimmed}");
        }

        if lower.contains("could not resolve hostname")
            || lower.contains("name or service not known")
            || lower.contains("nodename nor servname provided")
        {
            return format!("SSH host lookup failed: {trimmed}");
        }

        if lower.contains("connection timed out")
            || lower.contains("operation timed out")
            || lower.contains("connection refused")
            || lower.contains("no route to host")
            || lower.contains("network is unreachable")
        {
            return format!("SSH connection failed: {trimmed}");
        }

        if lower.contains("host key verification failed") {
            return format!("SSH host key verification failed: {trimmed}");
        }

        if lower.contains("client_tty") || lower.contains("client tty") {
            return format!("Remote client tty is invalid or unavailable: {trimmed}");
        }
    }

    // A `snapshot` subscribe frame without `rows` is the signature of a host
    // binary older than the picker-rows contract (see `SubscribeFrame::Snapshot`),
    // not corrupt output: name the fix instead of reporting generic bad JSON.
    // "Incompatible agentscan" also routes it onto the slow mismatch retry in
    // LiveConnection instead of a fast reconnect loop.
    if lower.contains("missing field `rows`") {
        return format!(
            "Incompatible agentscan {operation} output: the host's agentscan is older than this desktop and does not publish picker rows; update agentscan on the host: {trimmed}"
        );
    }

    if lower.contains("invalid agentscan")
        || lower.contains("invalid json")
        || lower.contains("expected value")
    {
        return format!("Invalid JSON from agentscan {operation}: {trimmed}");
    }

    if lower.contains("incompatible agentscan") || lower.contains("unsupported schema") {
        return format!("Incompatible agentscan {operation} output: {trimmed}");
    }

    if lower.contains("auto-start")
        || lower.contains("autostart")
        || lower.contains("trusted executable")
        || lower.contains("untrusted executable")
    {
        return format!("Daemon auto-start was refused: {trimmed}");
    }

    if lower.contains("tmux")
        && (lower.contains("not found")
            || lower.contains("no such file or directory")
            || lower.contains("no server running")
            || lower.contains("failed to connect")
            || lower.contains("can't find socket")
            || lower.contains("cannot find socket"))
    {
        return format!("tmux is unavailable: {trimmed}");
    }

    // The tmux server dropped a fresh client mid-handshake ("server exited
    // unexpectedly" / "lost server" are the tmux client's words for that).
    // Existing clients — the daemon's control-mode attach, the user's own
    // terminals — keep working, so rows keep streaming while every NEW client
    // (focus included) fails. The raw message reads like agentscan crashed the
    // server; it did not. Verified root cause (mander, 2026-06): a tmux
    // client/server VERSION SPLIT — the server ran linuxbrew tmux 3.6b while
    // non-interactive SSH resolved /usr/bin/tmux 3.4 (brew's PATH only loads
    // in interactive shells), and the newer server drops the older client
    // without even a version reply. Restarting the server does NOT clear it
    // (a fresh server showed the same symptom); aligning the installs does.
    if lower.contains("tmux")
        && (lower.contains("server exited unexpectedly") || lower.contains("lost server"))
    {
        // The same split happens locally (the desktop app's PATH vs the shell
        // that started tmux), so name the resolver this runner actually uses.
        let resolver = match runner {
            AgentscanRunner::Ssh(_) => "non-interactive SSH",
            AgentscanRunner::Local(_) => "the desktop app",
        };
        return format!(
            "The tmux server dropped a fresh client (running sessions are fine). \
             This usually means the server was started from a different tmux \
             install than the one {resolver} resolves — align them so both use \
             the same tmux: {trimmed}"
        );
    }

    // Match the binary's *configured* name, so an SSH profile with a custom name
    // (e.g. `scanctl`) is still recognized as not-found. The remote `env` error
    // echoes that name. Local keeps the literal "agentscan" — a local spawn error
    // ("No such file or directory (os error 2)") doesn't echo the resolved path.
    let binary_not_found = match runner {
        AgentscanRunner::Ssh(settings) => looks_like_binary_not_found(
            &lower,
            &remote_agentscan_binary_for_settings(settings).to_lowercase(),
        ),
        AgentscanRunner::Local(_) => looks_like_binary_not_found(&lower, "agentscan"),
    };
    if binary_not_found {
        return match runner {
            AgentscanRunner::Ssh(_) => {
                format!("Remote agentscan binary was not found: {trimmed}")
            }
            AgentscanRunner::Local(_) => format!("agentscan binary was not found: {trimmed}"),
        };
    }

    if operation == "focus"
        && (lower.contains("target pane")
            || lower.contains("can't find pane")
            || lower.contains("pane not found")
            || lower.contains("missing pane")
            || lower.contains("no such pane"))
    {
        return format!("Focus target is stale: {trimmed}");
    }

    trimmed.to_owned()
}

// Lowercased-message predicate for "the configured binary itself could not be
// found" (vs. auth/connectivity/tmux/pane failures). `binary_lower` is the
// lowercased configured command name/path, so a custom binary name is matched by
// its own name rather than a hard-coded "agentscan". Shared by the failure
// classifier and the SSH preflight hint so both agree on what counts.
fn looks_like_binary_not_found(lower: &str, binary_lower: &str) -> bool {
    (lower.contains("command not found")
        || lower.contains("not found")
        || lower.contains("no such file or directory"))
        && lower.contains(binary_lower)
}

// A classified preflight failure: the message to show, plus an optional remote
// path the desktop can offer as a one-click "use this path" fix.
struct PreflightFailure {
    message: String,
    suggested_binary_path: Option<String>,
}

// Classify a preflight failure, and for a remote not-found turn the dead-end
// into an actionable hint by probing where the user's own shell finds agentscan.
// The probe is gated to this case (binary missing on an otherwise-reachable
// host) so it runs at most once and never on connectivity/auth failures.
fn classify_preflight_failure(runner: &AgentscanRunner, raw: &str) -> PreflightFailure {
    let classified = classify_desktop_failure(runner, "preflight", raw);
    if let AgentscanRunner::Ssh(settings) = runner
        && looks_like_binary_not_found(
            &raw.to_lowercase(),
            &remote_agentscan_binary_for_settings(settings).to_lowercase(),
        )
        && let Some(probe) = remote_not_found_probe(settings)
    {
        let message = format!("{classified} {}", remote_not_found_hint_message(&probe));
        let suggested_binary_path = match probe {
            RemoteAgentscanProbe::Found(path) => Some(path),
            RemoteAgentscanProbe::Missing => None,
        };
        return PreflightFailure {
            message,
            suggested_binary_path,
        };
    }
    PreflightFailure {
        message: classified,
        suggested_binary_path: None,
    }
}

// Marker-delimited probe of the remote's login + interactive shell (`-lic`),
// which mirrors the SSH login session the user themselves get — and is exactly
// the environment whose PATH the desktop's own commands run under. `-l` sources
// `.profile`/`.zprofile`, `-i` sources `.zshrc`/`.bashrc`; together they cover
// the common cases. (A bash account whose `.bash_profile` doesn't source
// `.bashrc` and whose PATH lives only in `.bashrc` isn't covered — but such a
// setup isn't on the SSH login PATH either, so it's genuinely unreachable here;
// reporting not-found is correct, not a miss.)
//
// The probed name is the *configured* binary (forwarded as `$1`), so a custom
// command name or wrapper is resolved rather than a hard-coded "agentscan" — the
// "Use this path" action must never overwrite a profile with the wrong binary.
// Only an absolute, executable path is emitted as `ASFOUND=<path>`, so an
// alias/function/builtin (which `command -v` prints as non-path text) is reported
// as not-found rather than persisted as a bogus binary path. The `ASFOUND=`
// marker survives any rc-file stdout banner noise.
//
// Best-effort and POSIX-family-scoped: the snippet is POSIX `sh` syntax, so a
// fish/csh login shell rejects it and the probe yields no hint (the plain
// not-found error + "Open settings" still stand). Per-shell branching isn't worth
// it for a diagnostic, so this degrades silently rather than guessing.
const REMOTE_PROBE_BODY: &str = r#"p=$(command -v "$1" 2>/dev/null); case "$p" in /*) [ -x "$p" ] || p=;; *) p=;; esac; printf "ASFOUND=%s\n" "$p""#;

// `2>/dev/null` redirects only stderr (fd 2), dropping rc-file banner/error
// noise. The `printf "ASFOUND=..."` in REMOTE_PROBE_BODY writes to stdout (fd 1),
// which is left intact and carries the marker back to parse_remote_probe — so the
// hint still works in the found-binary case.
fn remote_probe_script(binary: &str) -> String {
    format!(
        "\"$SHELL\" -lic {} sh {} 2>/dev/null",
        shell_quote(REMOTE_PROBE_BODY),
        shell_quote(binary),
    )
}

enum RemoteAgentscanProbe {
    Found(String),
    Missing,
}

// Best-effort: returns None when the probe can't run or the host is unreachable
// (BatchMode/ConnectTimeout fail fast), so we enrich only when we have a result.
fn remote_not_found_probe(settings: &SshRunnerSettings) -> Option<RemoteAgentscanProbe> {
    let mut command = ssh_probe_command(settings).ok()?;
    let output = run_command_with_timeout(&mut command, REMOTE_PROBE_TIMEOUT).ok()?;
    if !output.status.success() {
        return None;
    }
    parse_remote_probe(&String::from_utf8_lossy(&output.stdout))
}

fn ssh_probe_command(settings: &SshRunnerSettings) -> Result<Command, String> {
    validate_ssh_host(&settings.host)?;

    let binary = remote_agentscan_binary_for_settings(settings);
    let mut command = Command::new("ssh");
    command
        .arg("-n")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("ConnectTimeout=5")
        .arg("--")
        .arg(settings.host.trim())
        .arg(remote_probe_script(&binary))
        .stdin(Stdio::null());
    Ok(command)
}

fn parse_remote_probe(stdout: &str) -> Option<RemoteAgentscanProbe> {
    let value = stdout
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("ASFOUND="))?
        .trim();
    Some(if value.is_empty() {
        RemoteAgentscanProbe::Missing
    } else {
        RemoteAgentscanProbe::Found(value.to_owned())
    })
}

fn remote_not_found_hint_message(probe: &RemoteAgentscanProbe) -> String {
    match probe {
        RemoteAgentscanProbe::Found(path) => format!(
            "Your shell finds agentscan at {path}, but it isn't on the non-interactive PATH SSH uses (your shell adds it only in an interactive rc file). Set this profile's agentscan binary to {path}."
        ),
        RemoteAgentscanProbe::Missing => "agentscan was not found on the remote host. Install it there, or set this profile's agentscan binary to its absolute path.".to_owned(),
    }
}

#[cfg(test)]
fn run_agentscan_binary_command<const N: usize>(
    binary: &OsStr,
    args: [&str; N],
    timeout: Duration,
) -> Result<CommandOutput, String> {
    run_agentscan_local_command_with_env(binary, args, &[], timeout)
}

fn run_agentscan_command(
    runner: &AgentscanRunner,
    args: &[&str],
    timeout: Duration,
) -> Result<CommandOutput, String> {
    let mut command = agentscan_command(runner, args)?;
    run_command_with_timeout(&mut command, timeout)
}

pub(crate) fn agentscan_command(
    runner: &AgentscanRunner,
    args: &[&str],
) -> Result<Command, String> {
    match runner {
        AgentscanRunner::Local(settings) => {
            let mut command = Command::new(agentscan_binary_for_settings(settings));
            command.args(args);
            apply_command_env(&mut command, &settings.env)?;
            Ok(command)
        }
        AgentscanRunner::Ssh(settings) => ssh_agentscan_command(settings, args),
    }
}

fn ssh_agentscan_command(settings: &SshRunnerSettings, args: &[&str]) -> Result<Command, String> {
    ssh_command_for_script(settings, remote_agentscan_script(settings, args)?)
}

// Single home for the ssh invocation shape: host validation plus the `--`
// terminator before the destination guard against option injection, so every
// ssh-backed command (data path and preflight) must route through here.
fn ssh_command_for_script(settings: &SshRunnerSettings, script: String) -> Result<Command, String> {
    validate_ssh_host(&settings.host)?;

    let mut command = Command::new("ssh");
    command.arg("--").arg(settings.host.trim()).arg(script);
    Ok(command)
}

// The preflight's command differs from the shared wrapper only over SSH, where the
// remote script additionally prints the host-probe marker (one SSH round-trip for
// version check + hostname). subscribe/focus/hotkeys keep the plain wrapper via
// agentscan_command so their stdout stays pure agentscan output.
fn agentscan_preflight_command(runner: &AgentscanRunner) -> Result<Command, String> {
    match runner {
        AgentscanRunner::Local(_) => agentscan_command(runner, &["--version"]),
        AgentscanRunner::Ssh(settings) => {
            ssh_command_for_script(settings, remote_agentscan_preflight_script(settings)?)
        }
    }
}

fn remote_agentscan_script(settings: &SshRunnerSettings, args: &[&str]) -> Result<String, String> {
    remote_agentscan_script_with_body(settings, args, &remote_path_sh_script())
}

// The preflight-only wrapper: the shared PATH script prefixed with the host-probe
// marker line. Args are pinned to `--version` because the marker may only ever
// pollute the preflight's stdout, never a data command's.
fn remote_agentscan_preflight_script(settings: &SshRunnerSettings) -> Result<String, String> {
    remote_agentscan_script_with_body(settings, &["--version"], &remote_preflight_sh_script())
}

fn remote_agentscan_script_with_body(
    settings: &SshRunnerSettings,
    args: &[&str],
    sh_body: &str,
) -> Result<String, String> {
    validate_command_env(&settings.env)?;

    // Wrap the invocation in `sh -c` so the PATH augmentation runs with
    // guaranteed POSIX semantics — quoted "$PATH" (no word-splitting when a PATH
    // entry contains spaces), colon-joined, "$HOME" expanded — regardless of the
    // remote login shell. Only *invoking* `sh` depends on that shell, which every
    // shell can do (incl. fish, where `$PATH` is a space-joined list, and
    // csh/tcsh, which reject the inline `NAME=VALUE` prefix) — preserving the
    // shell-agnostic property the bare `exec env` form had. The env assignments,
    // binary, and args are forwarded as positional parameters (`"$@"`), so they
    // keep their outer shell-quoting and need no inner re-quoting.
    let mut parts = Vec::with_capacity(settings.env.len() + args.len() + 6);
    parts.push("exec".to_owned());
    parts.push("sh".to_owned());
    parts.push("-c".to_owned());
    parts.push(shell_quote(sh_body));
    parts.push("sh".to_owned()); // $0 for the inner shell; real args follow as "$@"
    for variable in &settings.env {
        parts.push(format!(
            "{}={}",
            variable.name.trim(),
            shell_quote(&variable.value)
        ));
    }
    parts.push(shell_quote(&remote_agentscan_binary_for_settings(settings)));
    parts.extend(args.iter().map(|arg| shell_quote(arg)));
    Ok(parts.join(" "))
}

// POSIX `sh -c` body. Broadens PATH so a bare-name `agentscan` resolves on the
// remote: a non-interactive `ssh host "cmd"` shell skips rc files, so version-
// manager (mise/asdf), cargo, and `~/.local/bin` dirs are absent and `env
// agentscan` would fail with "No such file or directory". The fallback dirs are
// appended *after* `$PATH`, so the remote's own resolution wins and a stale shim
// can't shadow a binary already on PATH. "$PATH"/"$HOME" are double-quoted so a
// PATH entry with spaces isn't split; the dir list is a fixed constant, so there
// is no injection surface. `${PATH:+$PATH:}` keeps the inherited PATH (with its
// trailing separator) only when it's non-empty, so an empty remote PATH doesn't
// yield a leading `:` — which would otherwise make `env` search the cwd first.
// A PATH set in the profile env (forwarded in `"$@"`) still wins, since `env`
// applies it after this.
fn remote_path_sh_script() -> String {
    let mut path = String::from("PATH=\"${PATH:+$PATH:}");
    for (index, dir) in AGENTSCAN_BIN_DIRS
        .iter()
        .chain(AGENTSCAN_SHIM_DIRS)
        .enumerate()
    {
        if index > 0 {
            path.push(':');
        }
        match dir {
            AgentscanBinDir::Home(rel) => {
                path.push_str("$HOME/");
                path.push_str(rel);
            }
            AgentscanBinDir::Abs(abs) => path.push_str(abs),
        }
    }
    path.push('"');
    format!("{path}; export PATH; exec env \"$@\"")
}

// Prefix for the single stdout line carrying the remote hostname, emitted by the
// preflight wrapper and stripped back out by split_remote_host_marker. Unique
// enough that real `agentscan --version` output can never collide with it.
const REMOTE_HOST_MARKER: &str = "__AGENTSCAN_REMOTE_HOST__=";

// The preflight's `sh -c` body: print the remote hostname as a marked line, then
// run the shared PATH wrapper. `hostname` resolves before the PATH augmentation
// (it lives in /bin or /usr/bin everywhere), and a failure prints an empty value,
// which the parser maps to None.
fn remote_preflight_sh_script() -> String {
    format!(
        "printf '{REMOTE_HOST_MARKER}%s\\n' \"$(hostname 2>/dev/null)\"; {}",
        remote_path_sh_script()
    )
}

fn validate_ssh_host(host: &str) -> Result<(), String> {
    let host = host.trim();

    if host.is_empty() {
        return Err("SSH host cannot be empty".to_owned());
    }

    if host.starts_with('-') || host.contains('\0') {
        return Err(format!("Invalid SSH host: {host}"));
    }

    if host.chars().any(char::is_whitespace) {
        return Err(format!("Invalid SSH host: {host}"));
    }

    Ok(())
}

fn validate_client_tty(client_tty: &str) -> Result<(), String> {
    let client_tty = client_tty.trim();

    if client_tty.is_empty() {
        return Ok(());
    }

    if client_tty.contains('\0') || client_tty.chars().any(char::is_whitespace) {
        return Err(format!("Invalid remote client tty: {client_tty}"));
    }

    Ok(())
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_owned();
    }

    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
fn run_agentscan_local_command_with_env<const N: usize>(
    binary: &OsStr,
    args: [&str; N],
    env: &[LocalEnvironmentVariable],
    timeout: Duration,
) -> Result<CommandOutput, String> {
    let mut command = Command::new(binary);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_command_env(&mut command, env)?;
    run_command_with_timeout(&mut command, timeout)
}

fn run_command_with_timeout(
    command: &mut Command,
    timeout: Duration,
) -> Result<CommandOutput, String> {
    run_command_with_timeout_interruptible(command, timeout, None)
}

fn run_command_with_timeout_interruptible(
    command: &mut Command,
    timeout: Duration,
    stop: Option<&AtomicBool>,
) -> Result<CommandOutput, String> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| error.to_string())?;

    // Drain stdout/stderr on their own threads so the timeout governs the whole
    // operation. `wait_with_output` reads the pipes to EOF, which never arrives
    // if a descendant (e.g. an auto-started agentscan daemon) inherited and is
    // holding these pipes open after the direct child exits — that would hang
    // the command past its timeout. Collecting via channels lets us cap the
    // post-exit drain instead of blocking forever.
    let stdout_rx = spawn_pipe_collector(child.stdout.take());
    let stderr_rx = spawn_pipe_collector(child.stderr.take());

    let start = Instant::now();
    loop {
        // Bail promptly when a caller (e.g. the live picker worker on a profile
        // switch) signals stop, so it isn't blocked for the full timeout.
        if stop.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("agentscan command canceled".to_owned());
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                // The child exited; its own output is already written. Drain the
                // buffered bytes but don't wait out a descendant holding the pipe.
                return Ok(CommandOutput {
                    status,
                    stdout: collect_pipe(stdout_rx, LIVE_CHILD_EXIT_GRACE),
                    stderr: collect_pipe(stderr_rx, LIVE_CHILD_EXIT_GRACE),
                });
            }
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "agentscan command timed out after {}ms",
                    timeout.as_millis()
                ));
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.to_string());
            }
        }
    }
}

// A child pipe being drained on a detached background thread. Bytes read so far
// accumulate in a shared buffer, and `done` fires once the reader reaches EOF.
// Detached so a descendant holding the pipe open can't block callers; see
// run_command_with_timeout.
pub(crate) struct PipeCollector {
    buf: Arc<Mutex<Vec<u8>>>,
    done: std::sync::mpsc::Receiver<()>,
}

pub(crate) fn spawn_pipe_collector<R: std::io::Read + Send + 'static>(
    reader: Option<R>,
) -> Option<PipeCollector> {
    reader.map(|mut reader| {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let writer = Arc::clone(&buf);
        let (done_tx, done) = std::sync::mpsc::channel();
        let _ = thread::Builder::new()
            .name("agentscan-command-pipe".to_owned())
            .spawn(move || {
                let mut chunk = [0u8; 8192];
                loop {
                    match std::io::Read::read(&mut reader, &mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => {
                            if let Ok(mut guard) = writer.lock() {
                                guard.extend_from_slice(&chunk[..read]);
                            }
                        }
                    }
                }
                let _ = done_tx.send(());
            });
        PipeCollector { buf, done }
    })
}

// Wait up to `timeout` for the pipe to reach EOF, then return whatever was read.
// A descendant holding the pipe open means EOF never arrives, but the direct
// child's own output is already buffered — so we return it rather than dropping
// it on a timeout (which would make a successful command look like blank output).
pub(crate) fn collect_pipe(collector: Option<PipeCollector>, timeout: Duration) -> Vec<u8> {
    match collector {
        Some(collector) => {
            let _ = collector.done.recv_timeout(timeout);
            collector
                .buf
                .lock()
                .map(|guard| guard.clone())
                .unwrap_or_default()
        }
        None => Vec::new(),
    }
}

fn apply_command_env(
    command: &mut Command,
    env: &[LocalEnvironmentVariable],
) -> Result<(), String> {
    validate_command_env(env)?;

    for variable in env {
        command.env(variable.name.trim(), &variable.value);
    }

    Ok(())
}

fn validate_command_env(env: &[LocalEnvironmentVariable]) -> Result<(), String> {
    for variable in env {
        let name = variable.name.trim();

        if name.is_empty() {
            return Err("Environment variable names cannot be empty".to_owned());
        }

        // Names are interpolated unquoted into the remote SSH shell script
        // (`NAME=value`), so restrict them to POSIX shell identifiers to avoid
        // breaking the command or injecting shell syntax.
        let mut chars = name.chars();
        let valid = matches!(chars.next(), Some(first) if first == '_' || first.is_ascii_alphabetic())
            && chars.all(|c| c == '_' || c.is_ascii_alphanumeric());
        if !valid {
            return Err(format!("Invalid environment variable name: {name}"));
        }
    }

    Ok(())
}

fn stderr_or_status(command: &str, stderr: &[u8], status: std::process::ExitStatus) -> String {
    let stderr = String::from_utf8_lossy(stderr).trim().to_owned();

    if stderr.is_empty() {
        format!("{command} exited with status {status}")
    } else {
        stderr
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    #[test]
    fn hotkeys_args_always_latch_with_no_auto_start() {
        // The desktop's row fetch must never auto-start a daemon; only an explicit
        // "Start agentscan" subscribe may. So --no-auto-start is unconditional here.
        assert_eq!(
            hotkeys_args(),
            vec!["hotkeys", "--format", "json", "--no-auto-start"]
        );
    }

    #[test]
    fn daemon_status_reachable_only_false_for_not_running() {
        // The single confident "no daemon" signal — keep cheap-polling, don't re-arm.
        assert!(!daemon_status_reachable(
            &serde_json::json!({ "daemon_state": "not_running" })
        ));
        // Any live state is reachable — escalate to a full subscribe re-arm.
        for state in ["ready", "initializing", "startup_failed", "closing"] {
            assert!(daemon_status_reachable(
                &serde_json::json!({ "daemon_state": state })
            ));
        }
        // Missing or non-string field: safe-escalate (treat as reachable) rather than
        // wedge the latch poll on an unexpected payload.
        assert!(daemon_status_reachable(&serde_json::json!({})));
        assert!(daemon_status_reachable(
            &serde_json::json!({ "daemon_state": 7 })
        ));
    }

    #[test]
    fn missing_preflight_binary_reports_failure() {
        let result = run_agentscan_preflight(OsString::from("agentscan-missing-for-test"));

        assert_eq!(result.binary, "agentscan-missing-for-test");
        assert!(!result.ok);
        assert!(result.version.is_none());
        assert!(result.error.is_some());
    }

    #[test]
    fn focus_picker_row_rejects_empty_pane_id() {
        assert_eq!(
            focus_picker_row_with_binary(OsString::from("agentscan"), "  ").unwrap_err(),
            "Cannot focus an empty pane id"
        );
    }

    #[test]
    fn preflight_times_out_hanging_binary() {
        let script = env::temp_dir().join(format!(
            "agentscan-preflight-hang-{}-{}.sh",
            std::process::id(),
            "timeout"
        ));
        fs::write(&script, "#!/bin/sh\nsleep 5\n").expect("write test script");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))
            .expect("make test script executable");

        let result = run_agentscan_preflight_with_timeout(
            script.clone().into_os_string(),
            Duration::from_millis(50),
        );
        let _ = fs::remove_file(script);

        assert!(!result.ok);
        assert!(result.version.is_none());
        assert!(result.error.as_deref().unwrap_or("").contains("timed out"));
    }

    #[test]
    fn known_agentscan_paths_include_gui_launch_locations() {
        let home = Some(OsStr::new("/Users/example"));
        let paths: Vec<_> = agentscan_paths_in(AGENTSCAN_BIN_DIRS, home)
            .chain(agentscan_paths_in(AGENTSCAN_SHIM_DIRS, home))
            .collect();

        // Concrete GUI-launch dirs first (cargo, Homebrew, /usr/local/bin,
        // ~/.local/bin), then the version-manager shims LAST so a stale shim never
        // shadows a real binary above it.
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/Users/example/.cargo/bin/agentscan"),
                PathBuf::from("/opt/homebrew/bin/agentscan"),
                PathBuf::from("/usr/local/bin/agentscan"),
                PathBuf::from("/Users/example/.local/bin/agentscan"),
                PathBuf::from("/Users/example/.local/share/mise/shims/agentscan"),
                PathBuf::from("/Users/example/.asdf/shims/agentscan"),
            ]
        );
    }

    #[test]
    fn known_agentscan_paths_skip_empty_home() {
        let home = Some(OsStr::new(""));
        let paths: Vec<_> = agentscan_paths_in(AGENTSCAN_BIN_DIRS, home)
            .chain(agentscan_paths_in(AGENTSCAN_SHIM_DIRS, home))
            .collect();

        assert_eq!(
            paths,
            vec![
                PathBuf::from("/opt/homebrew/bin/agentscan"),
                PathBuf::from("/usr/local/bin/agentscan"),
            ]
        );
    }

    #[test]
    fn local_resolution_prefers_concrete_dir_over_path_and_shim() {
        let home = Some(OsStr::new("/Users/example"));
        let path_var = Some(OsStr::new("/custom/bin"));
        let present = [
            "/Users/example/.cargo/bin/agentscan",
            "/custom/bin/agentscan",
            "/Users/example/.asdf/shims/agentscan",
        ];

        let resolved = resolve_local_agentscan(home, path_var, |path| {
            present.iter().any(|candidate| Path::new(candidate) == path)
        });

        assert_eq!(
            resolved,
            Some(PathBuf::from("/Users/example/.cargo/bin/agentscan"))
        );
    }

    #[test]
    fn local_resolution_prefers_path_binary_over_stale_shim() {
        // A real agentscan resolvable on the inherited PATH plus a leftover mise
        // shim. PATH must win so a stale shim never shadows a working binary that
        // the prior (bare-name) spawn would have found.
        let home = Some(OsStr::new("/Users/example"));
        let path_var = Some(OsStr::new("/custom/bin:/usr/bin"));
        let present = [
            "/custom/bin/agentscan",
            "/Users/example/.local/share/mise/shims/agentscan",
        ];

        let resolved = resolve_local_agentscan(home, path_var, |path| {
            present.iter().any(|candidate| Path::new(candidate) == path)
        });

        assert_eq!(resolved, Some(PathBuf::from("/custom/bin/agentscan")));
    }

    #[test]
    fn local_resolution_falls_back_to_shim_when_path_lacks_binary() {
        // GUI launched from Finder: a minimal PATH without agentscan, installed
        // only via mise. The shim is the only way to find it.
        let home = Some(OsStr::new("/Users/example"));
        let path_var = Some(OsStr::new("/usr/bin:/bin"));
        let present = ["/Users/example/.local/share/mise/shims/agentscan"];

        let resolved = resolve_local_agentscan(home, path_var, |path| {
            present.iter().any(|candidate| Path::new(candidate) == path)
        });

        assert_eq!(
            resolved,
            Some(PathBuf::from(
                "/Users/example/.local/share/mise/shims/agentscan"
            ))
        );
    }

    #[test]
    fn local_resolution_skips_non_executable_path_entry() {
        // An earlier PATH entry holds a non-executable `agentscan` (predicate
        // false); the real executable is later on PATH. The scan must skip the
        // stub and continue, matching how the OS resolves a bare command name,
        // instead of pinning the first regular file.
        let home = Some(OsStr::new("/Users/example"));
        let path_var = Some(OsStr::new("/stub/bin:/real/bin"));
        let executable = ["/real/bin/agentscan"];

        let resolved = resolve_local_agentscan(home, path_var, |path| {
            executable
                .iter()
                .any(|candidate| Path::new(candidate) == path)
        });

        assert_eq!(resolved, Some(PathBuf::from("/real/bin/agentscan")));
    }

    #[test]
    fn is_executable_file_requires_execute_bit() {
        let dir = env::temp_dir().join(format!("agentscan-exec-test-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("create test dir");
        let exec = dir.join("agentscan-exec");
        let plain = dir.join("agentscan-plain");
        fs::write(&exec, "#!/bin/sh\n").expect("write exec");
        fs::write(&plain, "not runnable").expect("write plain");
        fs::set_permissions(&exec, fs::Permissions::from_mode(0o755)).expect("chmod exec");
        fs::set_permissions(&plain, fs::Permissions::from_mode(0o644)).expect("chmod plain");

        assert!(is_executable_file(&exec));
        assert!(!is_executable_file(&plain));
        // A directory is not an executable file even with the execute bit set.
        assert!(!is_executable_file(&dir));
        // A missing path is not executable.
        assert!(!is_executable_file(&dir.join("absent")));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn runner_settings_override_binary_path() {
        let settings = LocalRunnerSettings {
            binary_path: Some("  /tmp/agentscan-custom  ".to_owned()),
            env: Vec::new(),
        };

        assert_eq!(
            agentscan_binary_for_settings(&settings),
            OsString::from("/tmp/agentscan-custom")
        );
    }

    #[test]
    fn runner_settings_deserialize_frontend_local_payload() {
        let settings: DesktopRunnerSettings = serde_json::from_str(
            r#"{
              "kind": "local",
              "binaryPath": "/tmp/agentscan-custom",
              "env": [{ "name": "AGENTSCAN_SOCKET_PATH", "value": "/tmp/agentscan.sock" }]
            }"#,
        )
        .expect("frontend local runner payload deserializes");

        assert_eq!(
            settings,
            DesktopRunnerSettings::Local {
                binary_path: Some("/tmp/agentscan-custom".to_owned()),
                env: vec![LocalEnvironmentVariable {
                    name: "AGENTSCAN_SOCKET_PATH".to_owned(),
                    value: "/tmp/agentscan.sock".to_owned(),
                }],
            }
        );
    }

    #[test]
    fn runner_settings_deserialize_frontend_ssh_payload() {
        let settings: DesktopRunnerSettings = serde_json::from_str(
            r#"{
              "kind": "ssh",
              "host": "devbox",
              "clientTty": "/dev/ttys003",
              "binaryPath": "/opt/agentscan",
              "env": []
            }"#,
        )
        .expect("frontend ssh runner payload deserializes");

        assert_eq!(
            settings,
            DesktopRunnerSettings::Ssh {
                host: "devbox".to_owned(),
                client_tty: Some("/dev/ttys003".to_owned()),
                binary_path: Some("/opt/agentscan".to_owned()),
                env: Vec::new(),
            }
        );
    }

    #[test]
    fn ssh_focus_args_include_optional_client_tty() {
        let runner = AgentscanRunner::Ssh(SshRunnerSettings {
            host: "devbox".to_owned(),
            client_tty: Some("/dev/ttys003".to_owned()),
            binary_path: None,
            env: Vec::new(),
        });

        assert_eq!(
            focus_args_for_runner(&runner, "%42").expect("focus args build"),
            vec!["focus", "--client-tty", "/dev/ttys003", "%42"]
        );
    }

    #[test]
    fn ssh_focus_args_reject_invalid_client_tty() {
        let runner = AgentscanRunner::Ssh(SshRunnerSettings {
            host: "devbox".to_owned(),
            client_tty: Some("/dev/tty bad".to_owned()),
            binary_path: None,
            env: Vec::new(),
        });

        assert!(
            focus_args_for_runner(&runner, "%42")
                .unwrap_err()
                .contains("Remote client tty")
        );
    }

    #[test]
    fn ssh_runner_builds_remote_agentscan_script() {
        let settings = SshRunnerSettings {
            host: "devbox".to_owned(),
            client_tty: None,
            binary_path: Some("/opt/bin/agentscan custom".to_owned()),
            env: vec![
                LocalEnvironmentVariable {
                    name: "AGENTSCAN_TMUX_SOCKET".to_owned(),
                    value: "/tmp/tmux socket".to_owned(),
                },
                LocalEnvironmentVariable {
                    name: "QUOTE".to_owned(),
                    value: "can't".to_owned(),
                },
            ],
        };

        assert_eq!(
            remote_agentscan_script(&settings, &["hotkeys", "--format", "json"]).unwrap(),
            "exec sh -c 'PATH=\"${PATH:+$PATH:}$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$HOME/.local/bin:$HOME/.local/share/mise/shims:$HOME/.asdf/shims\"; export PATH; exec env \"$@\"' sh AGENTSCAN_TMUX_SOCKET='/tmp/tmux socket' QUOTE='can'\\''t' '/opt/bin/agentscan custom' 'hotkeys' '--format' 'json'"
        );
    }

    #[test]
    fn remote_script_appends_fallback_bin_dirs_after_path() {
        // Regression: a non-interactive `ssh host "cmd"` shell skips rc files, so
        // a bare-name `agentscan` lookup misses version-manager (mise/asdf),
        // cargo, and `~/.local/bin` installs. The remote script broadens PATH so
        // `env` resolves it — but *after* `$PATH`, so the remote's own resolution
        // wins and a stale shim can't shadow a binary already on PATH. The PATH
        // work runs inside `sh -c` so it's correct on any login shell (fish/csh).
        let settings = SshRunnerSettings {
            host: "devbox".to_owned(),
            client_tty: None,
            binary_path: None,
            env: Vec::new(),
        };

        let script = remote_agentscan_script(&settings, &["--version"]).unwrap();
        assert_eq!(
            script,
            "exec sh -c 'PATH=\"${PATH:+$PATH:}$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$HOME/.local/bin:$HOME/.local/share/mise/shims:$HOME/.asdf/shims\"; export PATH; exec env \"$@\"' sh 'agentscan' '--version'"
        );
        // Shell-agnostic wrapper; "$PATH" double-quoted (whitespace-safe) and kept
        // first via `${PATH:+$PATH:}` (no leading colon -> no cwd lookup when PATH
        // is empty); the mise shim dir is present and the binary is forwarded via
        // "$@".
        assert!(script.starts_with("exec sh -c "));
        assert!(script.contains("PATH=\"${PATH:+$PATH:}"));
        assert!(script.contains("exec env \"$@\""));
        assert!(script.contains("$HOME/.local/share/mise/shims"));
        // Shim dirs trail the real-binary dirs so a wrapper never wins first.
        assert!(
            script.find("$HOME/.cargo/bin").unwrap()
                < script.find("$HOME/.local/share/mise/shims").unwrap()
        );
    }

    #[test]
    fn host_marker_appears_only_in_preflight_script() {
        let settings = SshRunnerSettings {
            host: "devbox".to_owned(),
            client_tty: None,
            binary_path: None,
            env: Vec::new(),
        };

        let preflight = remote_agentscan_preflight_script(&settings).unwrap();
        assert!(preflight.contains(REMOTE_HOST_MARKER));
        assert!(preflight.contains("$(hostname 2>/dev/null)"));
        // The marker prints before the shared wrapper, and the args stay --version.
        assert!(preflight.contains("export PATH; exec env \"$@\""));
        assert!(preflight.ends_with("'agentscan' '--version'"));

        // Data commands keep the plain wrapper; their stdout must stay pure
        // agentscan output.
        for args in [
            &["--version"][..],
            &["subscribe", "--format", "json"][..],
            &["hotkeys", "--format", "json"][..],
        ] {
            assert!(
                !remote_agentscan_script(&settings, args)
                    .unwrap()
                    .contains(REMOTE_HOST_MARKER)
            );
        }
    }

    #[test]
    fn split_remote_host_marker_extracts_shortens_and_strips() {
        let (label, rest) = split_remote_host_marker(
            "__AGENTSCAN_REMOTE_HOST__=koopa.home.arpa\nagentscan 0.7.1\n",
        );
        assert_eq!(label.as_deref(), Some("koopa"));
        assert_eq!(rest, "agentscan 0.7.1");
    }

    #[test]
    fn split_remote_host_marker_missing_marker_yields_none() {
        let (label, rest) = split_remote_host_marker("agentscan 0.7.1\n");
        assert_eq!(label, None);
        assert_eq!(rest, "agentscan 0.7.1");
    }

    #[test]
    fn split_remote_host_marker_empty_hostname_yields_none() {
        let (label, rest) = split_remote_host_marker("__AGENTSCAN_REMOTE_HOST__=\nagentscan 0.7.1");
        assert_eq!(label, None);
        assert_eq!(rest, "agentscan 0.7.1");
    }

    #[test]
    fn ssh_runner_wraps_command_with_ssh_destination() {
        let settings = SshRunnerSettings {
            host: "user@devbox".to_owned(),
            client_tty: None,
            binary_path: None,
            env: Vec::new(),
        };
        let command = ssh_agentscan_command(&settings, &["subscribe", "--format", "json"])
            .expect("ssh command builds");

        assert_eq!(command.get_program(), OsStr::new("ssh"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![
                OsStr::new("--"),
                OsStr::new("user@devbox"),
                OsStr::new(
                    "exec sh -c 'PATH=\"${PATH:+$PATH:}$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$HOME/.local/bin:$HOME/.local/share/mise/shims:$HOME/.asdf/shims\"; export PATH; exec env \"$@\"' sh 'agentscan' 'subscribe' '--format' 'json'"
                )
            ]
        );
    }

    #[test]
    fn ssh_runner_rejects_empty_and_option_shaped_hosts() {
        let mut settings = SshRunnerSettings {
            host: " ".to_owned(),
            client_tty: None,
            binary_path: None,
            env: Vec::new(),
        };

        assert_eq!(
            ssh_agentscan_command(&settings, &["--version"])
                .unwrap_err()
                .as_str(),
            "SSH host cannot be empty"
        );

        settings.host = "-oProxyCommand=bad".to_owned();
        assert!(
            ssh_agentscan_command(&settings, &["--version"])
                .unwrap_err()
                .contains("Invalid SSH host")
        );

        settings.host = "dev box".to_owned();
        assert!(
            ssh_agentscan_command(&settings, &["--version"])
                .unwrap_err()
                .contains("Invalid SSH host")
        );
    }

    #[test]
    fn desktop_failure_classification_groups_remote_failures() {
        let runner = AgentscanRunner::Ssh(SshRunnerSettings {
            host: "devbox".to_owned(),
            client_tty: None,
            binary_path: None,
            env: Vec::new(),
        });

        assert!(
            classify_desktop_failure(&runner, "preflight", "Permission denied (publickey)")
                .starts_with("SSH authentication failed")
        );
        assert!(
            classify_desktop_failure(
                &runner,
                "preflight",
                "ssh: Could not resolve hostname devbox",
            )
            .starts_with("SSH host lookup failed")
        );
        assert!(
            classify_desktop_failure(&runner, "hotkeys", "agentscan: command not found")
                .starts_with("Remote agentscan binary was not found")
        );
        assert!(
            classify_desktop_failure(
                &runner,
                "subscribe",
                "agentscan subscribe exited with status 1: tmux: No such file or directory",
            )
            .starts_with("tmux is unavailable")
        );
        assert!(
            classify_desktop_failure(&runner, "hotkeys", "Invalid agentscan hotkeys JSON")
                .starts_with("Invalid JSON from agentscan hotkeys")
        );
        // A snapshot frame without `rows` is an old host binary, not corrupt
        // output: the message must name the upgrade fix (and route onto the
        // slow mismatch retry) instead of reporting generic invalid JSON.
        let old_host = classify_desktop_failure(
            &runner,
            "subscribe",
            "Invalid agentscan subscribe frame: missing field `rows` at line 1 column 512",
        );
        assert!(old_host.starts_with("Incompatible agentscan subscribe output"));
        assert!(old_host.contains("update agentscan on the host"));
        assert!(
            classify_desktop_failure(&runner, "focus", "can't find pane: %42")
                .starts_with("Focus target is stale")
        );
        assert!(
            classify_desktop_failure(
                &runner,
                "focus",
                "tmux switch-client fallback failed: server exited unexpectedly",
            )
            .contains("non-interactive SSH resolves")
        );
        // The local variant points at the desktop app's own resolution instead
        // of SSH guidance that wouldn't apply.
        let local_runner = AgentscanRunner::Local(LocalRunnerSettings {
            binary_path: None,
            env: Vec::new(),
        });
        assert!(
            classify_desktop_failure(
                &local_runner,
                "focus",
                "tmux switch-client fallback failed: server exited unexpectedly",
            )
            .contains("the desktop app resolves")
        );
    }

    #[test]
    fn binary_not_found_predicate_matches_missing_binary_only() {
        // The desktop's reproduced failure and a plain "command not found".
        assert!(looks_like_binary_not_found(
            "env: 'agentscan': no such file or directory",
            "agentscan"
        ));
        assert!(looks_like_binary_not_found(
            "agentscan: command not found",
            "agentscan"
        ));
        // A custom binary name is matched by its own name, not a hard-coded "agentscan".
        assert!(looks_like_binary_not_found(
            "env: 'scanctl': no such file or directory",
            "scanctl"
        ));
        assert!(!looks_like_binary_not_found(
            "env: 'scanctl': no such file or directory",
            "agentscan"
        ));
        // Not a missing-binary failure: auth, and a non-matching missing file.
        assert!(!looks_like_binary_not_found(
            "permission denied (publickey)",
            "agentscan"
        ));
        assert!(!looks_like_binary_not_found(
            "tmux: no such file or directory",
            "agentscan"
        ));
    }

    #[test]
    fn custom_named_ssh_binary_not_found_is_classified() {
        // A custom SSH binary name (no "agentscan" substring) must still classify
        // as not-found so the recovery probe/hint can fire — gate parity with the
        // name-aware probe.
        let runner = AgentscanRunner::Ssh(SshRunnerSettings {
            host: "devbox".to_owned(),
            client_tty: None,
            binary_path: Some("scanctl".to_owned()),
            env: Vec::new(),
        });
        let message = classify_desktop_failure(
            &runner,
            "preflight",
            "env: 'scanctl': No such file or directory",
        );
        assert!(message.starts_with("Remote agentscan binary was not found"));
    }

    #[test]
    fn ssh_probe_command_uses_fast_fail_flags_and_interactive_probe() {
        let settings = SshRunnerSettings {
            host: "devbox".to_owned(),
            client_tty: None,
            binary_path: None,
            env: Vec::new(),
        };
        let command = ssh_probe_command(&settings).expect("probe command builds");

        assert_eq!(command.get_program(), OsStr::new("ssh"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![
                OsStr::new("-n"),
                OsStr::new("-o"),
                OsStr::new("BatchMode=yes"),
                OsStr::new("-o"),
                OsStr::new("ConnectTimeout=5"),
                OsStr::new("--"),
                OsStr::new("devbox"),
                OsStr::new(&remote_probe_script("agentscan")),
            ]
        );
        let probe = remote_probe_script("agentscan");
        // Login + interactive (`-lic`): `-i` sources `.zshrc`/`.bashrc` (mise/asdf)
        // and `-l` sources `.profile`/`.zprofile` — together mirroring the SSH login
        // shell. Only an absolute, executable path is reported (`[ -x ]` + the `/*`
        // case), so an alias/function is never persisted as a binary path. The
        // configured name is probed via `$1` (forwarded as a positional), not a
        // hard-coded "agentscan".
        assert!(probe.contains("-lic"));
        assert!(probe.contains("[ -x "));
        assert!(probe.contains("command -v \"$1\""));
        assert!(probe.ends_with("sh 'agentscan' 2>/dev/null"));
    }

    #[test]
    fn ssh_probe_uses_configured_binary_name() {
        // A profile with a custom binary name must be probed by that name, so the
        // "Use this path" suggestion can't overwrite it with the default agentscan.
        let probe = remote_probe_script("agentscan-beta");
        assert!(probe.ends_with("sh 'agentscan-beta' 2>/dev/null"));
        assert!(!probe.contains("command -v agentscan-beta")); // name goes via $1, not inlined
        assert!(probe.contains("command -v \"$1\""));
    }

    #[test]
    fn parse_remote_probe_reads_marker_through_rc_noise() {
        // rc files may print their own stdout banner before the marker line.
        let found = parse_remote_probe("welcome to devbox\nASFOUND=/opt/tools/agentscan\n")
            .expect("probe parses");
        assert!(
            matches!(found, RemoteAgentscanProbe::Found(path) if path == "/opt/tools/agentscan")
        );

        assert!(matches!(
            parse_remote_probe("ASFOUND=\n").expect("probe parses"),
            RemoteAgentscanProbe::Missing
        ));
        // No marker at all (e.g. csh rejected the probe) -> nothing to report.
        assert!(parse_remote_probe("totally unrelated output").is_none());
    }

    #[test]
    fn remote_not_found_hint_distinguishes_path_gap_from_missing() {
        let found = remote_not_found_hint_message(&RemoteAgentscanProbe::Found(
            "/home/me/.local/share/mise/shims/agentscan".to_owned(),
        ));
        assert!(found.contains("/home/me/.local/share/mise/shims/agentscan"));
        assert!(found.contains("Set this profile's agentscan binary"));

        let missing = remote_not_found_hint_message(&RemoteAgentscanProbe::Missing);
        assert!(missing.contains("not found on the remote host"));
    }

    #[test]
    fn local_preflight_not_found_carries_no_remote_suggestion() {
        // A local runner never triggers the SSH probe, so the classified failure
        // stands alone with no path to one-click-apply.
        let runner = AgentscanRunner::Local(LocalRunnerSettings::default());
        let failure = classify_preflight_failure(&runner, "agentscan: command not found");

        assert!(
            failure
                .message
                .starts_with("agentscan binary was not found")
        );
        assert!(failure.suggested_binary_path.is_none());
    }

    #[test]
    fn command_env_rejects_empty_and_invalid_names() {
        let mut command = Command::new("agentscan");

        assert_eq!(
            apply_command_env(
                &mut command,
                &[LocalEnvironmentVariable {
                    name: " ".to_owned(),
                    value: "value".to_owned()
                }]
            )
            .unwrap_err(),
            "Environment variable names cannot be empty"
        );

        let mut command = Command::new("agentscan");
        assert!(
            apply_command_env(
                &mut command,
                &[LocalEnvironmentVariable {
                    name: "BAD=NAME".to_owned(),
                    value: "value".to_owned()
                }]
            )
            .unwrap_err()
            .contains("Invalid environment variable name")
        );
    }
}
