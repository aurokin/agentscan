use std::{
    env,
    ffi::{OsStr, OsString},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(2);
const HOTKEYS_TIMEOUT: Duration = Duration::from_secs(5);
const FOCUS_TIMEOUT: Duration = Duration::from_secs(5);
const DAEMON_STATUS_TIMEOUT: Duration = Duration::from_secs(2);
const LIVE_RECONNECT_DELAY: Duration = Duration::from_secs(1);
const LIVE_PICKER_EVENT: &str = "agentscan-live-picker";

static LIVE_PICKER: OnceLock<Mutex<Option<LivePickerSupervisor>>> = OnceLock::new();

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopProfile {
    id: &'static str,
    name: &'static str,
    kind: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentscanPreflight {
    binary: String,
    ok: bool,
    version: Option<String>,
    error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalRunnerSettings {
    binary_path: Option<String>,
    #[serde(default)]
    env: Vec<LocalEnvironmentVariable>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalEnvironmentVariable {
    name: String,
    value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
enum DesktopRunnerSettings {
    Local {
        binary_path: Option<String>,
        #[serde(default)]
        env: Vec<LocalEnvironmentVariable>,
    },
    Ssh {
        host: String,
        binary_path: Option<String>,
        #[serde(default)]
        env: Vec<LocalEnvironmentVariable>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum AgentscanRunner {
    Local(LocalRunnerSettings),
    Ssh(SshRunnerSettings),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SshRunnerSettings {
    host: String,
    binary_path: Option<String>,
    env: Vec<LocalEnvironmentVariable>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CommandOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
struct PickerRow {
    key: String,
    pane_id: String,
    provider: Option<String>,
    status: PickerStatus,
    display_label: String,
    location_tag: String,
    location: PickerLocation,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
struct PickerStatus {
    kind: String,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
struct PickerLocation {
    session_name: String,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug)]
struct LivePickerSupervisor {
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    worker: Option<JoinHandle<()>>,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SubscribeFrame {
    Connecting { message: String },
    Snapshot { snapshot: serde_json::Value },
    Offline { message: String, retrying: bool },
    Shutdown { message: String },
    Fatal { message: String },
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum LivePickerEvent {
    Connecting {
        message: String,
    },
    Reconnecting {
        message: String,
        diagnostics: Option<serde_json::Value>,
    },
    Rows {
        rows: Vec<PickerRow>,
        snapshot: LiveSnapshotSummary,
    },
    Offline {
        message: String,
        retrying: bool,
        diagnostics: Option<serde_json::Value>,
    },
    Shutdown {
        message: String,
    },
    Fatal {
        message: String,
        diagnostics: Option<serde_json::Value>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LiveSnapshotSummary {
    pane_count: usize,
    generated_at: Option<String>,
    source_kind: Option<String>,
}

#[tauri::command]
fn local_profiles() -> Vec<DesktopProfile> {
    vec![DesktopProfile {
        id: "local",
        name: "Local",
        kind: "local",
    }]
}

#[tauri::command]
fn preflight_agentscan(settings: Option<DesktopRunnerSettings>) -> AgentscanPreflight {
    let runner = AgentscanRunner::from_settings(settings);
    run_agentscan_preflight_with_runner(&runner)
}

#[tauri::command]
fn load_picker_rows(settings: Option<DesktopRunnerSettings>) -> Result<Vec<PickerRow>, String> {
    let runner = AgentscanRunner::from_settings(settings);
    load_picker_rows_with_runner(&runner)
}

#[tauri::command]
fn focus_picker_row(pane_id: String, settings: Option<DesktopRunnerSettings>) -> Result<(), String> {
    let runner = AgentscanRunner::from_settings(settings);
    focus_picker_row_with_runner(&runner, &pane_id)
}

#[tauri::command]
fn start_live_picker(
    app: tauri::AppHandle,
    settings: Option<DesktopRunnerSettings>,
) -> Result<(), String> {
    let runner = AgentscanRunner::from_settings(settings);
    start_live_picker_with_runner(app, runner)
}

#[tauri::command]
fn stop_live_picker() -> Result<(), String> {
    stop_live_picker_supervisor()
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

fn find_known_agentscan_binary() -> Option<PathBuf> {
    known_agentscan_paths(env::var_os("HOME").as_deref()).find(|path| path.is_file())
}

fn known_agentscan_paths(home: Option<&OsStr>) -> impl Iterator<Item = PathBuf> {
    let home_candidate = home
        .filter(|home| !home.is_empty())
        .map(|home| Path::new(home).join(".cargo/bin/agentscan"));

    [
        home_candidate,
        Some(PathBuf::from("/opt/homebrew/bin/agentscan")),
        Some(PathBuf::from("/usr/local/bin/agentscan")),
    ]
    .into_iter()
    .flatten()
}

impl AgentscanRunner {
    fn from_settings(settings: Option<DesktopRunnerSettings>) -> Self {
        match settings {
            Some(DesktopRunnerSettings::Local { binary_path, env }) => {
                Self::Local(LocalRunnerSettings { binary_path, env })
            }
            Some(DesktopRunnerSettings::Ssh {
                host,
                binary_path,
                env,
            }) => Self::Ssh(SshRunnerSettings {
                host: host.trim().to_owned(),
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

fn run_agentscan_preflight_with_runner(runner: &AgentscanRunner) -> AgentscanPreflight {
    let binary_display = runner.display_binary();

    match run_agentscan_command(runner, ["--version"], PREFLIGHT_TIMEOUT) {
        Ok(output) if output.status.success() => AgentscanPreflight {
            binary: binary_display,
            ok: true,
            version: Some(String::from_utf8_lossy(&output.stdout).trim().to_owned()),
            error: None,
        },
        Ok(output) => AgentscanPreflight {
            binary: binary_display,
            ok: false,
            version: None,
            error: Some(stderr_or_status("agentscan", &output.stderr, output.status)),
        },
        Err(error) => AgentscanPreflight {
            binary: binary_display,
            ok: false,
            version: None,
            error: Some(error.to_string()),
        },
    }
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
        },
        Ok(output) => AgentscanPreflight {
            binary: binary_display,
            ok: false,
            version: None,
            error: Some(stderr_or_status("agentscan", &output.stderr, output.status)),
        },
        Err(error) => AgentscanPreflight {
            binary: binary_display,
            ok: false,
            version: None,
            error: Some(error.to_string()),
        },
    }
}

fn load_picker_rows_with_runner(runner: &AgentscanRunner) -> Result<Vec<PickerRow>, String> {
    load_picker_rows_from_runner(runner)
}

fn load_picker_rows_from_runner(runner: &AgentscanRunner) -> Result<Vec<PickerRow>, String> {
    let output = run_agentscan_command(runner, ["hotkeys", "--format", "json"], HOTKEYS_TIMEOUT)
        .map_err(|error| format!("Unable to run agentscan hotkeys: {error}"))?;

    if !output.status.success() {
        return Err(stderr_or_status(
            "agentscan hotkeys",
            &output.stderr,
            output.status,
        ));
    }

    let rows: Vec<PickerRow> = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("Invalid agentscan hotkeys JSON: {error}"))?;
    validate_picker_rows(&rows)?;
    Ok(rows)
}

fn focus_picker_row_with_runner(runner: &AgentscanRunner, pane_id: &str) -> Result<(), String> {
    focus_picker_row_with_runner_and_timeout(runner, pane_id, FOCUS_TIMEOUT)
}

#[cfg(test)]
fn focus_picker_row_with_binary(binary: OsString, pane_id: &str) -> Result<(), String> {
    focus_picker_row_with_runner(&AgentscanRunner::Local(LocalRunnerSettings {
        binary_path: Some(binary.to_string_lossy().into_owned()),
        env: Vec::new(),
    }), pane_id)
}

fn focus_picker_row_with_runner_and_timeout(
    runner: &AgentscanRunner,
    pane_id: &str,
    timeout: Duration,
) -> Result<(), String> {
    if pane_id.trim().is_empty() {
        return Err("Cannot focus an empty pane id".to_owned());
    }

    let output = run_agentscan_command(runner, ["focus", pane_id], timeout)
        .map_err(|error| format!("Unable to run agentscan focus: {error}"))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(stderr_or_status(
            "agentscan focus",
            &output.stderr,
            output.status,
        ))
    }
}

fn start_live_picker_with_runner(
    app: tauri::AppHandle,
    runner: AgentscanRunner,
) -> Result<(), String> {
    let mut supervisor = live_picker_supervisor()
        .lock()
        .map_err(|_| "live picker supervisor lock poisoned".to_owned())?;

    if supervisor.is_some() {
        return Ok(());
    }

    let stop = Arc::new(AtomicBool::new(false));
    let child = Arc::new(Mutex::new(None));
    let worker_stop = Arc::clone(&stop);
    let worker_child = Arc::clone(&child);
    let worker = thread::Builder::new()
        .name("agentscan-live-picker".to_owned())
        .spawn(move || run_live_picker_worker(app, runner, worker_stop, worker_child))
        .map_err(|error| format!("Unable to start live picker worker: {error}"))?;

    *supervisor = Some(LivePickerSupervisor {
        stop,
        child,
        worker: Some(worker),
    });

    Ok(())
}

fn stop_live_picker_supervisor() -> Result<(), String> {
    let supervisor = live_picker_supervisor()
        .lock()
        .map_err(|_| "live picker supervisor lock poisoned".to_owned())?
        .take();

    if let Some(mut supervisor) = supervisor {
        supervisor.stop.store(true, Ordering::SeqCst);
        kill_live_picker_child(&supervisor.child);

        if let Some(worker) = supervisor.worker.take() {
            let _ = worker.join();
        }
    }

    Ok(())
}

fn live_picker_supervisor() -> &'static Mutex<Option<LivePickerSupervisor>> {
    LIVE_PICKER.get_or_init(|| Mutex::new(None))
}

fn run_live_picker_worker(
    app: tauri::AppHandle,
    runner: AgentscanRunner,
    stop: Arc<AtomicBool>,
    child_slot: Arc<Mutex<Option<Child>>>,
) {
    let mut has_connected = false;

    while !stop.load(Ordering::SeqCst) {
        if has_connected {
            emit_live_picker_event(
                &app,
                LivePickerEvent::Reconnecting {
                    message: "Reconnecting to agentscan subscribe".to_owned(),
                    diagnostics: load_daemon_status(&runner).ok(),
                },
            );
        } else {
            emit_live_picker_event(
                &app,
                LivePickerEvent::Connecting {
                    message: "Connecting to agentscan subscribe".to_owned(),
                },
            );
        }

        match run_live_picker_subscription(&app, &runner, &stop, &child_slot) {
            LivePickerWorkerExit::Stopped | LivePickerWorkerExit::Shutdown => break,
            LivePickerWorkerExit::Fatal => break,
            LivePickerWorkerExit::Retry => {
                has_connected = true;
                sleep_until_retry_or_stop(&stop);
            }
        }
    }

    kill_live_picker_child(&child_slot);
    let _ = live_picker_supervisor().lock().map(|mut supervisor| {
        if supervisor
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(&current.stop, &stop))
        {
            *supervisor = None;
        }
    });
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LivePickerWorkerExit {
    Retry,
    Shutdown,
    Fatal,
    Stopped,
}

fn run_live_picker_subscription(
    app: &tauri::AppHandle,
    runner: &AgentscanRunner,
    stop: &AtomicBool,
    child_slot: &Arc<Mutex<Option<Child>>>,
) -> LivePickerWorkerExit {
    let mut command = match agentscan_command(runner, ["subscribe", "--format", "json"]) {
        Ok(command) => command,
        Err(error) => {
            emit_live_picker_event(
                app,
                LivePickerEvent::Fatal {
                    message: error,
                    diagnostics: None,
                },
            );
            return LivePickerWorkerExit::Fatal;
        }
    };
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            emit_live_picker_event(
                app,
                LivePickerEvent::Offline {
                    message: format!("Unable to start agentscan subscribe: {error}"),
                    retrying: true,
                    diagnostics: load_daemon_status(runner).ok(),
                },
            );
            return LivePickerWorkerExit::Retry;
        }
    };

    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            emit_live_picker_event(
                app,
                LivePickerEvent::Offline {
                    message: "agentscan subscribe did not expose stdout".to_owned(),
                    retrying: true,
                    diagnostics: load_daemon_status(runner).ok(),
                },
            );
            return LivePickerWorkerExit::Retry;
        }
    };

    let stderr = child.stderr.take();
    if let Ok(mut slot) = child_slot.lock() {
        *slot = Some(child);
    } else {
        let _ = child.kill();
        let _ = child.wait();
        return LivePickerWorkerExit::Retry;
    }

    let stderr_reader = stderr.map(|stderr| {
        thread::Builder::new()
            .name("agentscan-live-picker-stderr".to_owned())
            .spawn(move || read_process_stderr(stderr))
            .ok()
    });
    let mut exit = LivePickerWorkerExit::Retry;

    for line in BufReader::new(stdout).lines() {
        if stop.load(Ordering::SeqCst) {
            exit = LivePickerWorkerExit::Stopped;
            break;
        }

        match line {
            Ok(line) if line.trim().is_empty() => {}
            Ok(line) => match serde_json::from_str::<SubscribeFrame>(&line) {
                Ok(frame) => match handle_subscribe_frame(app, runner, frame) {
                    LivePickerWorkerExit::Retry => {}
                    terminal_exit => {
                        exit = terminal_exit;
                        break;
                    }
                },
                Err(error) => {
                    emit_live_picker_event(
                        app,
                        LivePickerEvent::Offline {
                            message: format!("Invalid agentscan subscribe frame: {error}"),
                            retrying: true,
                            diagnostics: load_daemon_status(runner).ok(),
                        },
                    );
                    break;
                }
            },
            Err(error) => {
                if !stop.load(Ordering::SeqCst) {
                    emit_live_picker_event(
                        app,
                        LivePickerEvent::Offline {
                            message: format!("Unable to read agentscan subscribe output: {error}"),
                            retrying: true,
                            diagnostics: load_daemon_status(runner).ok(),
                        },
                    );
                }
                break;
            }
        }
    }

    let status_message = wait_for_live_picker_child(child_slot);
    let stderr = stderr_reader
        .flatten()
        .and_then(|worker| worker.join().ok())
        .unwrap_or_default();

    if stop.load(Ordering::SeqCst) {
        return LivePickerWorkerExit::Stopped;
    }

    if matches!(exit, LivePickerWorkerExit::Retry) {
        emit_live_picker_event(
            app,
                LivePickerEvent::Offline {
                    message: process_exit_message(status_message.as_deref(), &stderr),
                    retrying: true,
                    diagnostics: load_daemon_status(runner).ok(),
                },
            );
    }

    exit
}

fn handle_subscribe_frame(
    app: &tauri::AppHandle,
    runner: &AgentscanRunner,
    frame: SubscribeFrame,
) -> LivePickerWorkerExit {
    match live_event_from_subscribe_frame(runner, frame) {
        Ok((event, exit)) => {
            emit_live_picker_event(app, event);
            exit
        }
        Err(message) => {
            emit_live_picker_event(
                app,
                LivePickerEvent::Fatal {
                    message,
                    diagnostics: load_daemon_status(runner).ok(),
                },
            );
            LivePickerWorkerExit::Fatal
        }
    }
}

fn live_event_from_subscribe_frame(
    runner: &AgentscanRunner,
    frame: SubscribeFrame,
) -> Result<(LivePickerEvent, LivePickerWorkerExit), String> {
    match frame {
        SubscribeFrame::Connecting { message } => Ok((
            LivePickerEvent::Connecting { message },
            LivePickerWorkerExit::Retry,
        )),
        SubscribeFrame::Snapshot { snapshot } => {
            let rows = match load_picker_rows_from_runner(runner) {
                Ok(rows) => rows,
                Err(message) => {
                    return Ok((
                        LivePickerEvent::Offline {
                            message,
                            retrying: true,
                            diagnostics: load_daemon_status(runner).ok(),
                        },
                        LivePickerWorkerExit::Retry,
                    ));
                }
            };
            let snapshot = summarize_snapshot(&snapshot);
            Ok((
                LivePickerEvent::Rows { rows, snapshot },
                LivePickerWorkerExit::Retry,
            ))
        }
        SubscribeFrame::Offline { message, retrying } => Ok((
            LivePickerEvent::Offline {
                message,
                retrying,
                diagnostics: load_daemon_status(runner).ok(),
            },
            LivePickerWorkerExit::Retry,
        )),
        SubscribeFrame::Shutdown { message } => Ok((
            LivePickerEvent::Shutdown { message },
            LivePickerWorkerExit::Shutdown,
        )),
        SubscribeFrame::Fatal { message } => Ok((
            LivePickerEvent::Fatal {
                message,
                diagnostics: load_daemon_status(runner).ok(),
            },
            LivePickerWorkerExit::Fatal,
        )),
    }
}

fn summarize_snapshot(snapshot: &serde_json::Value) -> LiveSnapshotSummary {
    LiveSnapshotSummary {
        pane_count: snapshot
            .get("panes")
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len),
        generated_at: snapshot
            .get("generated_at")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        source_kind: snapshot
            .get("source")
            .and_then(|source| source.get("kind"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
    }
}

fn load_daemon_status(runner: &AgentscanRunner) -> Result<serde_json::Value, String> {
    let output = run_agentscan_command(
        runner,
        ["daemon", "status", "--format", "json"],
        DAEMON_STATUS_TIMEOUT,
    )
    .map_err(|error| format!("Unable to run agentscan daemon status: {error}"))?;

    if !output.status.success() {
        return Err(stderr_or_status(
            "agentscan daemon status",
            &output.stderr,
            output.status,
        ));
    }

    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("Invalid agentscan daemon status JSON: {error}"))
}

fn read_process_stderr(stderr: impl std::io::Read) -> String {
    let mut lines = Vec::new();
    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
        if !line.trim().is_empty() {
            lines.push(line);
        }
    }
    lines.join("\n")
}

fn wait_for_live_picker_child(child_slot: &Arc<Mutex<Option<Child>>>) -> Option<String> {
    let mut child = child_slot.lock().ok()?.take()?;
    Some(match child.wait() {
        Ok(status) => format!("agentscan subscribe exited with status {status}"),
        Err(error) => format!("Unable to wait for agentscan subscribe: {error}"),
    })
}

fn kill_live_picker_child(child_slot: &Arc<Mutex<Option<Child>>>) {
    if let Ok(mut slot) = child_slot.lock()
        && let Some(mut child) = slot.take()
    {
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn process_exit_message(status_message: Option<&str>, stderr: &str) -> String {
    let stderr = stderr.trim();

    match (status_message, stderr.is_empty()) {
        (Some(status), true) => status.to_owned(),
        (Some(status), false) => format!("{status}: {stderr}"),
        (None, true) => "agentscan subscribe exited".to_owned(),
        (None, false) => stderr.to_owned(),
    }
}

fn sleep_until_retry_or_stop(stop: &AtomicBool) {
    let start = Instant::now();
    while !stop.load(Ordering::SeqCst) && start.elapsed() < LIVE_RECONNECT_DELAY {
        thread::sleep(Duration::from_millis(25));
    }
}

fn emit_live_picker_event(app: &tauri::AppHandle, event: LivePickerEvent) {
    let _ = tauri::Emitter::emit(app, LIVE_PICKER_EVENT, event);
}

fn validate_picker_rows(rows: &[PickerRow]) -> Result<(), String> {
    for row in rows {
        if row.key.trim().is_empty() {
            return Err("Incompatible agentscan hotkeys output: row key is empty".to_owned());
        }

        if row.pane_id.trim().is_empty() {
            return Err(format!(
                "Incompatible agentscan hotkeys output: row {} has an empty pane_id",
                row.key
            ));
        }

        if row.display_label.trim().is_empty() {
            return Err(format!(
                "Incompatible agentscan hotkeys output: row {} has an empty display_label",
                row.key
            ));
        }

        if row.location_tag.trim().is_empty() {
            return Err(format!(
                "Incompatible agentscan hotkeys output: row {} has an empty location_tag",
                row.key
            ));
        }

        if row
            .provider
            .as_deref()
            .is_some_and(|provider| provider.trim().is_empty())
        {
            return Err(format!(
                "Incompatible agentscan hotkeys output: row {} has an empty provider",
                row.key
            ));
        }

        if row.status.kind.trim().is_empty() {
            return Err(format!(
                "Incompatible agentscan hotkeys output: row {} has an empty status kind",
                row.key
            ));
        }

        if row.location.session_name.trim().is_empty() {
            return Err(format!(
                "Incompatible agentscan hotkeys output: row {} has an empty session_name",
                row.key
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
fn run_agentscan_binary_command<const N: usize>(
    binary: &OsStr,
    args: [&str; N],
    timeout: Duration,
) -> Result<CommandOutput, String> {
    run_agentscan_local_command_with_env(binary, args, &[], timeout)
}

fn run_agentscan_command<const N: usize>(
    runner: &AgentscanRunner,
    args: [&str; N],
    timeout: Duration,
) -> Result<CommandOutput, String> {
    let mut command = agentscan_command(runner, args)?;
    run_command_with_timeout(&mut command, timeout)
}

fn agentscan_command<const N: usize>(
    runner: &AgentscanRunner,
    args: [&str; N],
) -> Result<Command, String> {
    match runner {
        AgentscanRunner::Local(settings) => {
            let mut command = Command::new(agentscan_binary_for_settings(settings));
            command.args(args);
            apply_command_env(&mut command, &settings.env)?;
            Ok(command)
        }
        AgentscanRunner::Ssh(settings) => ssh_agentscan_command(settings, &args),
    }
}

fn ssh_agentscan_command(settings: &SshRunnerSettings, args: &[&str]) -> Result<Command, String> {
    validate_ssh_host(&settings.host)?;

    let mut command = Command::new("ssh");
    command
        .arg("--")
        .arg(settings.host.trim())
        .arg(remote_agentscan_script(settings, args)?);
    Ok(command)
}

fn remote_agentscan_script(settings: &SshRunnerSettings, args: &[&str]) -> Result<String, String> {
    validate_command_env(&settings.env)?;

    let mut parts = Vec::with_capacity(settings.env.len() + args.len() + 2);
    for variable in &settings.env {
        parts.push(format!(
            "{}={}",
            variable.name.trim(),
            shell_quote(&variable.value)
        ));
    }
    parts.push("exec".to_owned());
    parts.push(shell_quote(&remote_agentscan_binary_for_settings(settings)));
    parts.extend(args.iter().map(|arg| shell_quote(arg)));
    Ok(parts.join(" "))
}

fn validate_ssh_host(host: &str) -> Result<(), String> {
    let host = host.trim();

    if host.is_empty() {
        return Err("SSH host cannot be empty".to_owned());
    }

    if host.starts_with('-') || host.contains('\0') {
        return Err(format!("Invalid SSH host: {host}"));
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
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| error.to_string())?;

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
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

    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    Ok(CommandOutput {
        status: output.status,
        stdout: output.stdout,
        stderr: output.stderr,
    })
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

        if name.contains('=') || name.contains('\0') {
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

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            focus_picker_row,
            local_profiles,
            load_picker_rows,
            preflight_agentscan,
            start_live_picker,
            stop_live_picker
        ])
        .run(tauri::generate_context!())
        .expect("error while running agentscan desktop");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    #[test]
    fn local_profile_is_built_in() {
        assert_eq!(
            local_profiles(),
            vec![DesktopProfile {
                id: "local",
                name: "Local",
                kind: "local"
            }]
        );
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
    fn picker_rows_accept_empty_output() {
        let rows: Vec<PickerRow> = serde_json::from_str("[]").expect("empty rows parse");

        assert!(validate_picker_rows(&rows).is_ok());
    }

    #[test]
    fn picker_rows_parse_contract_fields_and_preserve_extra_fields() {
        let rows: Vec<PickerRow> = serde_json::from_str(
            r#"[
              {
                "key": "1",
                "pane_id": "%1",
                "provider": "codex",
                "status": { "kind": "idle" },
                "display_label": "Root Task",
                "location_tag": "work:0.0",
                "location": { "session_name": "work" },
                "display": { "provider_marker": "🤖" }
              }
            ]"#,
        )
        .expect("picker row parses");

        assert!(validate_picker_rows(&rows).is_ok());
        assert_eq!(rows[0].key, "1");
        assert_eq!(rows[0].pane_id, "%1");
        assert_eq!(rows[0].provider.as_deref(), Some("codex"));
        assert_eq!(rows[0].status.kind, "idle");
        assert_eq!(rows[0].location.session_name, "work");
        assert!(rows[0].extra.contains_key("display"));
    }

    #[test]
    fn picker_rows_reject_incompatible_output() {
        let rows: Vec<PickerRow> = serde_json::from_str(
            r#"[
              {
                "key": "1",
                "pane_id": "",
                "provider": "codex",
                "status": { "kind": "idle" },
                "display_label": "Root Task",
                "location_tag": "work:0.0",
                "location": { "session_name": "work" }
              }
            ]"#,
        )
        .expect("picker row parses");

        assert!(validate_picker_rows(&rows).unwrap_err().contains("pane_id"));
    }

    #[test]
    fn picker_rows_reject_wrong_field_shapes() {
        let error = serde_json::from_str::<Vec<PickerRow>>(
            r#"[
              {
                "key": "1",
                "pane_id": "%1",
                "provider": {},
                "status": { "kind": "idle" },
                "display_label": "Root Task",
                "location_tag": "work:0.0",
                "location": { "session_name": "work" }
              }
            ]"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("invalid type"));
    }

    #[test]
    fn picker_rows_reject_empty_nested_fields() {
        let rows: Vec<PickerRow> = serde_json::from_str(
            r#"[
              {
                "key": "1",
                "pane_id": "%1",
                "provider": "codex",
                "status": { "kind": "" },
                "display_label": "Root Task",
                "location_tag": "work:0.0",
                "location": { "session_name": "work" }
              }
            ]"#,
        )
        .expect("picker row parses");

        assert!(
            validate_picker_rows(&rows)
                .unwrap_err()
                .contains("status kind")
        );
    }

    #[test]
    fn picker_rows_reject_missing_status() {
        let error = serde_json::from_str::<Vec<PickerRow>>(
            r#"[
              {
                "key": "1",
                "pane_id": "%1",
                "provider": "codex",
                "display_label": "Root Task",
                "location_tag": "work:0.0",
                "location": { "session_name": "work" }
              }
            ]"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("missing field `status`"));
    }

    #[test]
    fn focus_picker_row_rejects_empty_pane_id() {
        assert_eq!(
            focus_picker_row_with_binary(OsString::from("agentscan"), "  ").unwrap_err(),
            "Cannot focus an empty pane id"
        );
    }

    #[test]
    fn subscribe_lifecycle_frames_parse() {
        let frame: SubscribeFrame =
            serde_json::from_str(r#"{"type":"connecting","message":"connecting"}"#)
                .expect("connecting frame parses");

        assert_eq!(
            frame,
            SubscribeFrame::Connecting {
                message: "connecting".to_owned()
            }
        );

        let frame: SubscribeFrame =
            serde_json::from_str(r#"{"type":"offline","message":"lost","retrying":true}"#)
                .expect("offline frame parses");

        assert_eq!(
            frame,
            SubscribeFrame::Offline {
                message: "lost".to_owned(),
                retrying: true
            }
        );
    }

    #[test]
    fn snapshot_summary_reads_canonical_fields() {
        let snapshot: serde_json::Value = serde_json::from_str(
            r#"{
              "generated_at": "2026-05-23T20:00:00Z",
              "source": { "kind": "daemon" },
              "panes": [{ "pane_id": "%1" }, { "pane_id": "%2" }]
            }"#,
        )
        .expect("snapshot parses");

        assert_eq!(
            summarize_snapshot(&snapshot),
            LiveSnapshotSummary {
                pane_count: 2,
                generated_at: Some("2026-05-23T20:00:00Z".to_owned()),
                source_kind: Some("daemon".to_owned())
            }
        );
    }

    #[test]
    fn snapshot_summary_defaults_missing_optional_fields() {
        let snapshot: serde_json::Value =
            serde_json::from_str(r#"{ "panes": [] }"#).expect("snapshot parses");

        assert_eq!(
            summarize_snapshot(&snapshot),
            LiveSnapshotSummary {
                pane_count: 0,
                generated_at: None,
                source_kind: None
            }
        );
    }

    #[test]
    fn process_exit_message_preserves_stderr_context() {
        assert_eq!(
            process_exit_message(
                Some("agentscan subscribe exited with status 1"),
                "tmux missing"
            ),
            "agentscan subscribe exited with status 1: tmux missing"
        );

        assert_eq!(process_exit_message(None, ""), "agentscan subscribe exited");
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
        let paths: Vec<_> = known_agentscan_paths(Some(OsStr::new("/Users/example"))).collect();

        assert_eq!(
            paths,
            vec![
                PathBuf::from("/Users/example/.cargo/bin/agentscan"),
                PathBuf::from("/opt/homebrew/bin/agentscan"),
                PathBuf::from("/usr/local/bin/agentscan"),
            ]
        );
    }

    #[test]
    fn known_agentscan_paths_skip_empty_home() {
        let paths: Vec<_> = known_agentscan_paths(Some(OsStr::new(""))).collect();

        assert_eq!(
            paths,
            vec![
                PathBuf::from("/opt/homebrew/bin/agentscan"),
                PathBuf::from("/usr/local/bin/agentscan"),
            ]
        );
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
              "binaryPath": "/opt/agentscan",
              "env": []
            }"#,
        )
        .expect("frontend ssh runner payload deserializes");

        assert_eq!(
            settings,
            DesktopRunnerSettings::Ssh {
                host: "devbox".to_owned(),
                binary_path: Some("/opt/agentscan".to_owned()),
                env: Vec::new(),
            }
        );
    }

    #[test]
    fn ssh_runner_builds_remote_agentscan_script() {
        let settings = SshRunnerSettings {
            host: "devbox".to_owned(),
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
            "AGENTSCAN_TMUX_SOCKET='/tmp/tmux socket' QUOTE='can'\\''t' exec '/opt/bin/agentscan custom' 'hotkeys' '--format' 'json'"
        );
    }

    #[test]
    fn ssh_runner_wraps_command_with_ssh_destination() {
        let settings = SshRunnerSettings {
            host: "user@devbox".to_owned(),
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
                OsStr::new("exec 'agentscan' 'subscribe' '--format' 'json'")
            ]
        );
    }

    #[test]
    fn ssh_runner_rejects_empty_and_option_shaped_hosts() {
        let mut settings = SshRunnerSettings {
            host: " ".to_owned(),
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
