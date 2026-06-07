use std::{
    env,
    ffi::{OsStr, OsString},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use tauri::Manager;

const PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(2);
const HOTKEYS_TIMEOUT: Duration = Duration::from_secs(5);
const FOCUS_TIMEOUT: Duration = Duration::from_secs(5);
const DAEMON_STATUS_TIMEOUT: Duration = Duration::from_secs(2);
// Grace period to let a subscribe child exit on its own after its stdout signals
// termination (EOF or a terminal frame) before we kill it, so a child that
// lingers can't park the worker thread on an unbounded wait().
const LIVE_CHILD_EXIT_GRACE: Duration = Duration::from_millis(500);
const LIVE_PICKER_EVENT: &str = "agentscan-live-picker";
const PICKER_WINDOW_MARGIN: f64 = 16.0;
const PICKER_WINDOW_TARGET_WIDTH: f64 = 280.0;
// The drag floor sits ~21% below the default opening width: the picker opens at
// a compact sidebar size (matching the codex/claude code chat sidebars), and a
// user who wants a tighter strip can pull it in further by hand. Below ~250 the
// CSS flips agent rows to a two-line layout so the title + status keep breathing.
const PICKER_WINDOW_MIN_WIDTH: f64 = 220.0;
const PICKER_WINDOW_MAX_WIDTH: f64 = 520.0;
const PICKER_WINDOW_MIN_HEIGHT: f64 = 560.0;
const PICKER_WINDOW_MAX_HEIGHT: f64 = 960.0;
// Snap height for the horizontal "bar" dock: a short ribbon along the bottom edge,
// sized to the stacked session-label + chip strip (the chrome centers within it) rather
// than a tall slab. This is the inner/content height; a native titlebar (when not in
// frameless mode) sits above it. The frontend locks a PINNED bar to this exact height
// (min == max == BAR_WINDOW_HEIGHT in App.tsx) so it only resizes horizontally — keep
// the two values in sync.
const BAR_WINDOW_HEIGHT: f64 = 56.0;

static LIVE_PICKER: OnceLock<Mutex<Option<LivePickerSupervisor>>> = OnceLock::new();
// Serializes whole start operations (stop + spawn + install) so overlapping
// starts cannot interleave and leave a newer start silently no-op'ing while an
// older one wins the install race.
static LIVE_PICKER_START: OnceLock<Mutex<()>> = OnceLock::new();
// Highest subscription epoch we have honored with a start. The frontend issues
// strictly-increasing epochs (persisted across reload/HMR), so a late start()
// from a torn-down page carries a lower epoch; rejecting it here keeps that
// stale start from replacing the live page's worker. Read/written only under
// the start lock.
static LIVE_PICKER_LAST_STARTED: AtomicU64 = AtomicU64::new(0);

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
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum DesktopRunnerSettings {
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
enum AgentscanRunner {
    Local(LocalRunnerSettings),
    Ssh(SshRunnerSettings),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SshRunnerSettings {
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
    epoch: u64,
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
    // Idle heartbeat the daemon emits ~1/s so it can detect a closed consumer
    // while the stream is otherwise silent. It carries no state, so the live
    // worker ignores it (without it, every heartbeat would fail to parse and
    // tear down the subscription with a spurious "Offline, retrying").
    Keepalive,
}

// Wraps every emitted event with the epoch of the subscription that produced
// it. The live event channel is global, so on a profile switch a late frame
// from the previous worker can still arrive; the frontend compares the epoch to
// the subscription it requested and drops events from a superseded worker.
#[derive(Clone, Debug, serde::Serialize)]
struct LivePickerEnvelope {
    epoch: u64,
    #[serde(flatten)]
    event: LivePickerEvent,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum LivePickerEvent {
    Connecting {
        message: String,
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

#[derive(Clone, Copy, Debug, PartialEq)]
struct LogicalWorkArea {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PickerWindowPlacement {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
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
fn poll_daemon_status(settings: Option<DesktopRunnerSettings>) -> Result<DaemonPollResult, String> {
    let runner = AgentscanRunner::from_settings(settings);
    poll_daemon_status_with_runner(&runner)
}

#[tauri::command]
fn focus_picker_row(
    pane_id: String,
    settings: Option<DesktopRunnerSettings>,
) -> Result<(), String> {
    let runner = AgentscanRunner::from_settings(settings);
    focus_picker_row_with_runner(&runner, &pane_id)
}

#[tauri::command]
fn start_live_picker(
    app: tauri::AppHandle,
    settings: Option<DesktopRunnerSettings>,
    epoch: u64,
    // Latch policy: the desktop owns daemon lifecycle, so reconnect/latch attempts
    // pass `false` (subscribe with `--no-auto-start`, connecting only if a daemon is
    // already running). Only an explicit user "Start agentscan" passes `true`.
    auto_start: bool,
) -> Result<(), String> {
    let runner = AgentscanRunner::from_settings(settings);
    start_live_picker_with_runner(app, runner, epoch, auto_start)
}

#[tauri::command]
fn stop_live_picker(epoch: u64) -> Result<(), String> {
    // Epoch-guarded so a stale stop (e.g. from a reloaded/HMR'd frontend whose
    // async cleanup arrives after a newer subscription has started) cannot tear
    // down the current worker. Only stop if the running supervisor is this epoch.
    stop_live_picker_supervisor_for_epoch(Some(epoch))
}

#[tauri::command]
fn place_picker_window(window: tauri::Window) -> Result<(), String> {
    let Some(monitor) = summon_monitor(&window)? else {
        return Ok(());
    };
    let placement = sidebar_placement_for_work_area(logical_work_area_for_monitor(&monitor));

    window
        .set_size(tauri::LogicalSize::new(placement.width, placement.height))
        .map_err(|error| format!("Unable to size picker window: {error}"))?;
    window
        .set_position(tauri::LogicalPosition::new(placement.x, placement.y))
        .map_err(|error| format!("Unable to position picker window: {error}"))?;

    Ok(())
}

/// Snap the window into the horizontal "bar" dock: full work-area width, a short
/// bar height, pinned to the bottom edge. Mirrors place_picker_window for the
/// vertical strip; the frontend calls whichever matches the pinned orientation.
#[tauri::command]
fn place_bar_window(window: tauri::Window) -> Result<(), String> {
    let Some(monitor) = summon_monitor(&window)? else {
        return Ok(());
    };
    let placement = bar_placement_for_work_area(logical_work_area_for_monitor(&monitor));

    window
        .set_size(tauri::LogicalSize::new(placement.width, placement.height))
        .map_err(|error| format!("Unable to size bar window: {error}"))?;
    window
        .set_position(tauri::LogicalPosition::new(placement.x, placement.y))
        .map_err(|error| format!("Unable to position bar window: {error}"))?;

    Ok(())
}

/// Center the kept-warm settings window on the dock's current monitor. The window is
/// created hidden, so without this its first open (or a reopen after the dock moved to a
/// different display) reuses a stale position that can land off-screen or on the wrong
/// monitor. Invoked from the dock (the caller) before it shows the settings window, so the
/// monitor is resolved the same cursor-first way as the dock's own placement.
#[tauri::command]
fn place_settings_window(window: tauri::Window) -> Result<(), String> {
    let Some(monitor) = summon_monitor(&window)? else {
        return Ok(());
    };
    let Some(settings) = window.get_webview_window("settings") else {
        return Ok(());
    };
    let work_area = logical_work_area_for_monitor(&monitor);
    // Convert the settings window's physical size with ITS OWN monitor's scale, not the
    // dock/cursor monitor's: on a mixed-DPI setup (e.g. a 2x laptop plus a 1x external)
    // the two windows can sit on displays with different scale factors, and using the
    // wrong one yields a wrong logical size and a mis-centered (or partly off-screen)
    // window. Logical points are a shared space, so the result still centers correctly
    // against the dock monitor's logical work area.
    let settings_scale = settings
        .scale_factor()
        .map_err(|error| format!("Unable to read settings window scale: {error}"))?
        .max(1.0);
    let size = settings
        .outer_size()
        .map_err(|error| format!("Unable to read settings window size: {error}"))?
        .to_logical::<f64>(settings_scale);
    let (x, y) = centered_placement_for_work_area(work_area, size.width, size.height);
    settings
        .set_position(tauri::LogicalPosition::new(x, y))
        .map_err(|error| format!("Unable to position settings window: {error}"))?;

    Ok(())
}

/// Toggle the macOS "glass" backdrop (NSVisualEffectView) behind the webview.
/// The frontend owns the on/off preference and the surface tint; this just turns
/// the OS blur layer on or off so a translucent webview reveals it. No-op off
/// macOS, where the toggle isn't offered.
#[tauri::command]
fn set_window_glass(
    window: tauri::WebviewWindow,
    enabled: bool,
    // Corner radius for the vibrancy view, matching the CSS frameless rounding so the
    // frosted backdrop doesn't show square corners behind a rounded webview. None lets the
    // framed window's native rounding apply.
    radius: Option<f64>,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use std::sync::mpsc;

        // apply_vibrancy/clear_vibrancy touch AppKit (NSView) and fail with
        // Error::NotMainThread off the main thread — and Tauri command handlers run
        // on a worker thread. Marshal the work onto the main thread and block this
        // worker on the result so the command still reports success/failure.
        let (tx, rx) = mpsc::channel();
        let main_window = window.clone();
        window
            .run_on_main_thread(move || {
                use window_vibrancy::{
                    NSVisualEffectMaterial, NSVisualEffectState, apply_vibrancy, clear_vibrancy,
                };

                let outcome = (|| {
                    // apply_vibrancy appends a fresh NSVisualEffectView each call while
                    // clear_vibrancy only removes one, so always clear first. This keeps
                    // the command idempotent: repeated enables (HMR, remounts, double
                    // toggles) never stack blur layers, and disable fully removes it.
                    clear_vibrancy(&main_window)
                        .map_err(|error| format!("Unable to reset glass: {error}"))?;
                    if enabled {
                        apply_vibrancy(
                            &main_window,
                            // Popover is a frostier, appearance-adaptive material than
                            // Sidebar: the native blur itself carries enough contrast to
                            // keep text legible even when the CSS surface tint is fully
                            // clear (transparency at 100%), so no per-glyph scrim is
                            // needed. (HudWindow is frostier still but biased dark, which
                            // would wreck light mode — Popover follows the appearance.)
                            NSVisualEffectMaterial::Popover,
                            Some(NSVisualEffectState::Active),
                            radius,
                        )
                        .map_err(|error| format!("Unable to enable glass: {error}"))?;
                    }
                    Ok::<(), String>(())
                })();
                let _ = tx.send(outcome);
            })
            .map_err(|error| format!("Unable to schedule glass update: {error}"))?;
        rx.recv()
            .map_err(|error| format!("Glass update did not complete: {error}"))??;
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Keep the signature stable across platforms; nothing to do without vibrancy.
        let _ = (&window, enabled, radius);
    }

    Ok(())
}

/// Toggle the window's native decorations (titlebar + the macOS traffic lights). The
/// frontend owns the frameless preference; this just adds/removes the OS chrome so the
/// dock can supply its own drag/minimize/close controls when borderless. Cross-platform.
#[tauri::command]
fn set_window_decorations(window: tauri::WebviewWindow, decorations: bool) -> Result<(), String> {
    window
        .set_decorations(decorations)
        .map_err(|error| format!("Unable to set window decorations: {error}"))
}

fn summon_monitor(window: &tauri::Window) -> Result<Option<tauri::Monitor>, String> {
    if let Ok(cursor) = window.cursor_position()
        && let Ok(Some(monitor)) = window.monitor_from_point(cursor.x, cursor.y)
    {
        return Ok(Some(monitor));
    }

    window
        .primary_monitor()
        .map_err(|error| format!("Unable to resolve primary display: {error}"))
}

fn logical_work_area_for_monitor(monitor: &tauri::Monitor) -> LogicalWorkArea {
    let scale_factor = monitor.scale_factor().max(1.0);
    let work_area = monitor.work_area();

    LogicalWorkArea {
        x: f64::from(work_area.position.x) / scale_factor,
        y: f64::from(work_area.position.y) / scale_factor,
        width: f64::from(work_area.size.width) / scale_factor,
        height: f64::from(work_area.size.height) / scale_factor,
    }
}

fn sidebar_placement_for_work_area(work_area: LogicalWorkArea) -> PickerWindowPlacement {
    let available_width =
        (work_area.width - PICKER_WINDOW_MARGIN * 2.0).max(PICKER_WINDOW_MIN_WIDTH);
    let available_height =
        (work_area.height - PICKER_WINDOW_MARGIN * 2.0).max(PICKER_WINDOW_MIN_HEIGHT);
    let width = clamp_f64(
        PICKER_WINDOW_TARGET_WIDTH.min(available_width),
        PICKER_WINDOW_MIN_WIDTH,
        PICKER_WINDOW_MAX_WIDTH,
    );
    let height = clamp_f64(
        available_height,
        PICKER_WINDOW_MIN_HEIGHT,
        PICKER_WINDOW_MAX_HEIGHT,
    );

    PickerWindowPlacement {
        x: work_area.x + PICKER_WINDOW_MARGIN,
        y: work_area.y + PICKER_WINDOW_MARGIN,
        width,
        height,
    }
}

fn bar_placement_for_work_area(work_area: LogicalWorkArea) -> PickerWindowPlacement {
    let width = (work_area.width - PICKER_WINDOW_MARGIN * 2.0).max(PICKER_WINDOW_MIN_WIDTH);
    let height = BAR_WINDOW_HEIGHT;
    // Pin to the bottom of the work area, but never let the bar sit above its top
    // edge on a work area too short to hold the bar plus its margin. `height` is the
    // inner/content height (what set_size sets), so this pin is exact for a frameless bar
    // (outer == inner) — the intended horizontal mode. A framed window's native titlebar
    // adds outer height the pin doesn't model, so the framed bar overshoots the bottom by
    // that titlebar; this offset predates the bar-height work (it applied identically at the
    // old taller height) and self-corrects once decorations are dropped, so it's left as-is.
    let y = (work_area.y + work_area.height - height - PICKER_WINDOW_MARGIN).max(work_area.y);

    PickerWindowPlacement {
        x: work_area.x + PICKER_WINDOW_MARGIN,
        y,
        width,
        height,
    }
}

/// Center a window of the given logical size within a work area, clamping to the top-left
/// so an oversized window (or a very small display) still lands on-screen instead of off
/// the top/left edge.
fn centered_placement_for_work_area(
    work_area: LogicalWorkArea,
    width: f64,
    height: f64,
) -> (f64, f64) {
    let x = work_area.x + ((work_area.width - width) / 2.0).max(0.0);
    let y = work_area.y + ((work_area.height - height) / 2.0).max(0.0);
    (x, y)
}

fn clamp_f64(value: f64, min: f64, max: f64) -> f64 {
    value.max(min).min(max)
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

fn run_agentscan_preflight_with_runner(runner: &AgentscanRunner) -> AgentscanPreflight {
    let binary_display = runner.display_binary();

    match run_agentscan_command(runner, &["--version"], PREFLIGHT_TIMEOUT) {
        Ok(output) if output.status.success() => AgentscanPreflight {
            binary: binary_display,
            ok: true,
            version: Some(String::from_utf8_lossy(&output.stdout).trim().to_owned()),
            error: None,
        },
        Ok(output) => {
            let error = stderr_or_status("agentscan", &output.stderr, output.status);
            AgentscanPreflight {
                binary: binary_display,
                ok: false,
                version: None,
                error: Some(classify_desktop_failure(runner, "preflight", &error)),
            }
        }
        Err(error) => AgentscanPreflight {
            binary: binary_display,
            ok: false,
            version: None,
            error: Some(classify_desktop_failure(runner, "preflight", &error)),
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

fn load_picker_rows_with_runner(runner: &AgentscanRunner) -> Result<Vec<PickerRow>, String> {
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

    let rows: Vec<PickerRow> = serde_json::from_slice(&output.stdout).map_err(|error| {
        classify_desktop_failure(
            runner,
            "hotkeys",
            &format!("Invalid agentscan hotkeys JSON: {error}"),
        )
    })?;
    validate_picker_rows(&rows)
        .map_err(|error| classify_desktop_failure(runner, "hotkeys", &error))?;
    Ok(rows)
}

fn focus_picker_row_with_runner(runner: &AgentscanRunner, pane_id: &str) -> Result<(), String> {
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

fn focus_picker_row_with_runner_and_timeout(
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

fn start_live_picker_with_runner(
    app: tauri::AppHandle,
    runner: AgentscanRunner,
    epoch: u64,
    auto_start: bool,
) -> Result<(), String> {
    // Hold the start lock across the whole stop+spawn+install so overlapping
    // starts can't interleave; the loser would otherwise see a supervisor
    // installed between our stop and re-lock and silently no-op.
    let _start_guard = live_picker_start_lock()
        .lock()
        .map_err(|_| "live picker start lock poisoned".to_owned())?;

    // Ignore a stale start whose epoch does not advance past the last one we
    // honored. Epochs increase strictly across reloads/HMR, so a lower-or-equal
    // epoch here means this start came from a torn-down page; installing it
    // would stop the live page's worker and the live page (filtering on its own
    // higher epoch) would then drop every frame. We only *commit* the epoch
    // after the worker is installed (below), so a failed start does not advance
    // the guard and silently reject the frontend's retry of the same epoch.
    if epoch <= LIVE_PICKER_LAST_STARTED.load(Ordering::SeqCst) {
        return Ok(());
    }

    // Replace any running supervisor so the requested subscription (and its
    // epoch) always starts. stop joins the old worker without holding the
    // supervisor lock, so re-locking below is safe and (under the start lock)
    // no other start can install in between.
    stop_live_picker_supervisor()?;

    let mut supervisor = live_picker_supervisor()
        .lock()
        .map_err(|_| "live picker supervisor lock poisoned".to_owned())?;

    let stop = Arc::new(AtomicBool::new(false));
    let child = Arc::new(Mutex::new(None));
    let worker_stop = Arc::clone(&stop);
    let worker_child = Arc::clone(&child);
    let worker = thread::Builder::new()
        .name("agentscan-live-picker".to_owned())
        .spawn(move || {
            run_live_picker_worker(app, runner, worker_stop, worker_child, epoch, auto_start)
        })
        .map_err(|error| format!("Unable to start live picker worker: {error}"))?;

    *supervisor = Some(LivePickerSupervisor {
        epoch,
        stop,
        child,
        worker: Some(worker),
    });

    // Commit the epoch only now that the worker is installed.
    LIVE_PICKER_LAST_STARTED.store(epoch, Ordering::SeqCst);

    Ok(())
}

fn stop_live_picker_supervisor() -> Result<(), String> {
    stop_live_picker_supervisor_for_epoch(None)
}

// Take and tear down the current supervisor. When `target` is Some, only stop
// if the running supervisor's epoch matches (used by the epoch-guarded command);
// when None, stop unconditionally (used by start to replace any prior worker).
// The worker is joined after the lock guard is dropped to avoid deadlocking with
// the worker's own supervisor cleanup.
fn stop_live_picker_supervisor_for_epoch(target: Option<u64>) -> Result<(), String> {
    let supervisor = {
        let mut guard = live_picker_supervisor()
            .lock()
            .map_err(|_| "live picker supervisor lock poisoned".to_owned())?;
        let matches = guard
            .as_ref()
            .is_some_and(|current| target.is_none_or(|epoch| current.epoch == epoch));
        if matches { guard.take() } else { None }
    };

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

fn live_picker_start_lock() -> &'static Mutex<()> {
    LIVE_PICKER_START.get_or_init(|| Mutex::new(()))
}

fn run_live_picker_worker(
    app: tauri::AppHandle,
    runner: AgentscanRunner,
    stop: Arc<AtomicBool>,
    child_slot: Arc<Mutex<Option<Child>>>,
    epoch: u64,
    auto_start: bool,
) {
    // Single-shot: one subscribe attempt, no in-worker retry loop. Reconnect is owned by
    // the layers that can see it. The `agentscan subscribe` CLI self-recovers mid-stream
    // transient drops in its own loop (frames keep streaming on the live child). On a clean
    // daemon loss the CLI emits a terminal frame (Shutdown / Offline retrying:false / Fatal);
    // on an abnormal subscribe-child death (spawn/IO/protocol failure) this worker emits a
    // terminal Offline{retrying:false}. Either way the TS LiveConnection service re-arms with
    // a FRESH epoch and autoStart=false (`first && target.autoStart`, LiveConnection.ts), so
    // the desktop's latch-only recovery holds without this worker advancing the epoch or
    // auto-starting on its own. The recoverable re-arm backoff (~1s) lives in TS, matching
    // the old in-worker LIVE_RECONNECT_DELAY. See AUR-517 and the latch-only ADR.
    //
    // No connecting/reconnecting frame is emitted here: LiveConnection sets that status
    // itself before invoking start_live_picker (connecting on the first attach, reconnecting
    // on a re-arm), and the `agentscan subscribe` CLI emits its own per-connect `connecting`
    // frame (forwarded in handle_subscribe_frame). An emit here would only duplicate the
    // former and be overwritten by the latter.
    run_live_picker_subscription(&app, &runner, &stop, &child_slot, epoch, auto_start);

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
}

// Subscribe argv for the live worker. `--no-auto-start` is appended when the
// desktop wants to *latch* onto an already-running daemon without spawning one;
// only an explicit user "Start agentscan" requests auto-start (no flag).
fn subscribe_args(auto_start: bool) -> Vec<&'static str> {
    let mut args = vec!["subscribe", "--format", "json"];
    if !auto_start {
        args.push("--no-auto-start");
    }
    args
}

fn run_live_picker_subscription(
    app: &tauri::AppHandle,
    runner: &AgentscanRunner,
    stop: &AtomicBool,
    child_slot: &Arc<Mutex<Option<Child>>>,
    epoch: u64,
    auto_start: bool,
) {
    // Single-shot per AUR-517: this runs ONE subscribe. Any abnormal end of the child
    // (spawn/IO/protocol failure, or a bare exit with no terminal frame) is reported as a
    // terminal Offline{retrying:false} so the TS LiveConnection service re-arms with a fresh
    // epoch (latch-only). Only frames that keep the live child streaming — the daemon's own
    // Offline{retrying:true} self-heal and a transient row-fetch miss, both in
    // handle_subscribe_frame — stay retrying:true.
    let mut command = match agentscan_command(runner, &subscribe_args(auto_start)) {
        Ok(command) => command,
        Err(error) => {
            let message = classify_desktop_failure(runner, "subscribe", &error);
            emit_live_picker_event(
                app,
                epoch,
                LivePickerEvent::Fatal {
                    message,
                    diagnostics: None,
                },
            );
            return;
        }
    };
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let message = classify_desktop_failure(
                runner,
                "subscribe",
                &format!("Unable to start agentscan subscribe: {error}"),
            );
            emit_live_picker_event(
                app,
                epoch,
                LivePickerEvent::Offline {
                    message,
                    retrying: false,
                    diagnostics: load_daemon_status(runner).ok(),
                },
            );
            return;
        }
    };

    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            emit_live_picker_event(
                app,
                epoch,
                LivePickerEvent::Offline {
                    message: "agentscan subscribe did not expose stdout".to_owned(),
                    retrying: false,
                    diagnostics: load_daemon_status(runner).ok(),
                },
            );
            return;
        }
    };

    let stderr = child.stderr.take();
    match child_slot.lock() {
        Ok(mut slot) => {
            // A stop that raced ahead of us already ran its kill against an empty
            // slot (the child wasn't stored yet), so re-check the flag under the
            // lock. Otherwise we'd store the child and block in the read loop on a
            // process nobody will kill, wedging the stop that joins this worker.
            if stop.load(Ordering::SeqCst) {
                drop(slot);
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
            *slot = Some(child);
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            // Poisoned mutex (a holder panicked): the child is unreachable for a later
            // stop, so kill it and report a recoverable terminal — TS re-arms (latch-only).
            emit_live_picker_event(
                app,
                epoch,
                LivePickerEvent::Offline {
                    message: classify_desktop_failure(
                        runner,
                        "subscribe",
                        "agentscan subscribe state was poisoned",
                    ),
                    retrying: false,
                    diagnostics: load_daemon_status(runner).ok(),
                },
            );
            return;
        }
    }

    // Drain stderr on its own thread, accumulating into a shared buffer rather
    // than joining the thread directly: a descendant that inherited this pipe
    // (e.g. an auto-started daemon) can hold it open after the subscribe child
    // exits, so an unbounded join would wedge the worker — and any stop joining
    // it — forever. The shared buffer also means the bounded collection below
    // still returns the stderr the thread already read instead of discarding it.
    let stderr_collector = spawn_pipe_collector(stderr);
    // Set once a terminal frame (shutdown / offline-retrying-false / fatal) ended the read,
    // so the generic process-exit fallback below doesn't also emit for a clean terminal.
    let mut saw_terminal = false;
    // Set when the loop already emitted an Offline describing why it ended, so
    // the generic exit-reason emit below doesn't overwrite it with a vaguer
    // (or, after we kill the child, misleading) message.
    let mut reported_offline = false;

    for line in BufReader::new(stdout).lines() {
        if stop.load(Ordering::SeqCst) {
            break;
        }

        match line {
            Ok(line) if line.trim().is_empty() => {}
            Ok(line) => match serde_json::from_str::<SubscribeFrame>(&line) {
                Ok(frame) => match handle_subscribe_frame(app, runner, frame, epoch, stop) {
                    LivePickerWorkerExit::Retry => {}
                    _ => {
                        saw_terminal = true;
                        break;
                    }
                },
                Err(error) => {
                    let message = classify_desktop_failure(
                        runner,
                        "subscribe",
                        &format!("Invalid agentscan subscribe frame: {error}"),
                    );
                    emit_live_picker_event(
                        app,
                        epoch,
                        LivePickerEvent::Offline {
                            message,
                            retrying: false,
                            diagnostics: load_daemon_status(runner).ok(),
                        },
                    );
                    // A malformed frame is a protocol error, not a process exit:
                    // the child keeps stdout open and would block the wait below
                    // forever. Kill it so the worker can fall through to teardown.
                    kill_live_picker_child(child_slot);
                    reported_offline = true;
                    break;
                }
            },
            Err(error) => {
                if !stop.load(Ordering::SeqCst) {
                    let message = classify_desktop_failure(
                        runner,
                        "subscribe",
                        &format!("Unable to read agentscan subscribe output: {error}"),
                    );
                    emit_live_picker_event(
                        app,
                        epoch,
                        LivePickerEvent::Offline {
                            message,
                            retrying: false,
                            diagnostics: load_daemon_status(runner).ok(),
                        },
                    );
                    reported_offline = true;
                }
                break;
            }
        }
    }

    let status_message = wait_for_live_picker_child(child_slot);
    let stderr = filter_stderr_text(&collect_pipe(stderr_collector, LIVE_CHILD_EXIT_GRACE));

    if stop.load(Ordering::SeqCst) {
        return;
    }

    if !saw_terminal && !reported_offline {
        let message = classify_desktop_failure(
            runner,
            "subscribe",
            &process_exit_message(status_message.as_deref(), &stderr),
        );
        emit_live_picker_event(
            app,
            epoch,
            LivePickerEvent::Offline {
                message,
                retrying: false,
                diagnostics: load_daemon_status(runner).ok(),
            },
        );
    }
}

fn handle_subscribe_frame(
    app: &tauri::AppHandle,
    runner: &AgentscanRunner,
    frame: SubscribeFrame,
    epoch: u64,
    stop: &AtomicBool,
) -> LivePickerWorkerExit {
    match live_event_from_subscribe_frame(runner, frame, stop) {
        // A heartbeat (or any frame the worker doesn't act on) maps to no event:
        // keep reading the stream without disturbing the picker.
        Ok(None) => LivePickerWorkerExit::Retry,
        Ok(Some((event, exit))) => {
            emit_live_picker_event(app, epoch, event);
            exit
        }
        Err(message) => {
            emit_live_picker_event(
                app,
                epoch,
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
    stop: &AtomicBool,
) -> Result<Option<(LivePickerEvent, LivePickerWorkerExit)>, String> {
    match frame {
        SubscribeFrame::Connecting { message } => Ok(Some((
            LivePickerEvent::Connecting { message },
            LivePickerWorkerExit::Retry,
        ))),
        SubscribeFrame::Snapshot { snapshot } => {
            // Pass the worker's stop flag so a profile/runner switch isn't blocked
            // for the full hotkeys timeout while this snapshot fetch is in flight.
            let rows = match load_picker_rows_from_runner_interruptible(runner, Some(stop)) {
                Ok(rows) => rows,
                Err(message) => {
                    return Ok(Some((
                        LivePickerEvent::Offline {
                            message: classify_desktop_failure(runner, "hotkeys", &message),
                            retrying: true,
                            diagnostics: load_daemon_status(runner).ok(),
                        },
                        LivePickerWorkerExit::Retry,
                    )));
                }
            };
            let snapshot = summarize_snapshot(&snapshot);
            Ok(Some((
                LivePickerEvent::Rows { rows, snapshot },
                LivePickerWorkerExit::Retry,
            )))
        }
        SubscribeFrame::Offline { message, retrying } => Ok(Some((
            LivePickerEvent::Offline {
                message,
                retrying,
                diagnostics: load_daemon_status(runner).ok(),
            },
            // Honor the daemon's own retry decision: a terminal offline frame
            // (retrying:false, e.g. auto-start disabled) must settle, not loop
            // the subscription forever.
            if retrying {
                LivePickerWorkerExit::Retry
            } else {
                LivePickerWorkerExit::Shutdown
            },
        ))),
        SubscribeFrame::Shutdown { message } => Ok(Some((
            LivePickerEvent::Shutdown { message },
            LivePickerWorkerExit::Shutdown,
        ))),
        SubscribeFrame::Fatal { message } => Ok(Some((
            LivePickerEvent::Fatal {
                message,
                diagnostics: load_daemon_status(runner).ok(),
            },
            LivePickerWorkerExit::Fatal,
        ))),
        // Heartbeat: no picker-visible state, so emit nothing and keep reading.
        SubscribeFrame::Keepalive => Ok(None),
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
struct DaemonPollResult {
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
fn poll_daemon_status_with_runner(runner: &AgentscanRunner) -> Result<DaemonPollResult, String> {
    let status = load_daemon_status(runner)?;
    Ok(DaemonPollResult {
        reachable: daemon_status_reachable(&status),
    })
}

// Render collected stderr bytes into a compact message, dropping blank lines.
// Takes already-buffered bytes (from a pipe collector) so partial diagnostics
// survive even when the pipe never reaches EOF because a descendant holds it.
fn filter_stderr_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn wait_for_live_picker_child(child_slot: &Arc<Mutex<Option<Child>>>) -> Option<String> {
    // Take the child out and drop the slot guard before blocking, so a
    // concurrent kill (profile switch / shutdown) can always reach the slot
    // while we reap. (If that kill ran first it already took the child, and
    // this returns None.)
    let mut child = child_slot.lock().ok().and_then(|mut slot| slot.take())?;

    // The child has almost always exited already (that is why stdout reached
    // EOF). If it lingers after signaling termination, give it a short grace
    // period and then kill it rather than waiting unbounded.
    let deadline = Instant::now() + LIVE_CHILD_EXIT_GRACE;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Some(format!("agentscan subscribe exited with status {status}"));
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                return Some(match child.wait() {
                    Ok(status) => {
                        format!("agentscan subscribe did not exit; terminated ({status})")
                    }
                    Err(error) => format!("Unable to wait for agentscan subscribe: {error}"),
                });
            }
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(error) => return Some(format!("Unable to wait for agentscan subscribe: {error}")),
        }
    }
}

fn kill_live_picker_child(child_slot: &Arc<Mutex<Option<Child>>>) {
    // Take the child out and release the slot lock before blocking in wait(),
    // so other lifecycle paths can still acquire the slot while we reap.
    let child = child_slot.lock().ok().and_then(|mut slot| slot.take());
    if let Some(mut child) = child {
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

fn emit_live_picker_event(app: &tauri::AppHandle, epoch: u64, event: LivePickerEvent) {
    let _ = tauri::Emitter::emit(app, LIVE_PICKER_EVENT, LivePickerEnvelope { epoch, event });
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

fn classify_desktop_failure(runner: &AgentscanRunner, operation: &str, message: &str) -> String {
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

    if (lower.contains("command not found")
        || lower.contains("not found")
        || lower.contains("no such file or directory"))
        && lower.contains("agentscan")
    {
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

fn agentscan_command(runner: &AgentscanRunner, args: &[&str]) -> Result<Command, String> {
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

    let mut parts = Vec::with_capacity(settings.env.len() + args.len() + 3);
    // Run via `exec env NAME=VALUE ... binary args` rather than the POSIX
    // `NAME=VALUE exec binary` prefix form. SSH hands this string to the remote
    // user's login shell, and the inline assignment prefix is not portable
    // (csh/tcsh reject it). `env` is an external command parsed identically by
    // every shell, so env-bearing SSH profiles work regardless of remote shell.
    parts.push("exec".to_owned());
    parts.push("env".to_owned());
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
struct PipeCollector {
    buf: Arc<Mutex<Vec<u8>>>,
    done: std::sync::mpsc::Receiver<()>,
}

fn spawn_pipe_collector<R: std::io::Read + Send + 'static>(
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
fn collect_pipe(collector: Option<PipeCollector>, timeout: Duration) -> Vec<u8> {
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

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            focus_picker_row,
            local_profiles,
            load_picker_rows,
            place_bar_window,
            place_picker_window,
            place_settings_window,
            poll_daemon_status,
            preflight_agentscan,
            set_window_decorations,
            set_window_glass,
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
    fn subscribe_args_appends_no_auto_start_when_latching() {
        // Auto-start enabled (explicit "Start agentscan"): no flag, daemon may spawn.
        assert_eq!(subscribe_args(true), vec!["subscribe", "--format", "json"]);
        // Latch-only (reconnect/launch): never spawn a daemon, only attach to one.
        assert_eq!(
            subscribe_args(false),
            vec!["subscribe", "--format", "json", "--no-auto-start"]
        );
    }

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
    fn sidebar_placement_uses_standard_width_and_work_area_height() {
        assert_eq!(
            sidebar_placement_for_work_area(LogicalWorkArea {
                x: 100.0,
                y: 24.0,
                width: 1440.0,
                height: 900.0,
            }),
            PickerWindowPlacement {
                x: 116.0,
                y: 40.0,
                width: 280.0,
                height: 868.0,
            }
        );
    }

    #[test]
    fn sidebar_placement_clamps_small_and_large_work_areas() {
        // Small work area: width minus margins falls below MIN_WIDTH (220), so the
        // window is clamped up to the floor instead of shrinking with the screen.
        assert_eq!(
            sidebar_placement_for_work_area(LogicalWorkArea {
                x: 0.0,
                y: 0.0,
                width: 230.0,
                height: 420.0,
            }),
            PickerWindowPlacement {
                x: 16.0,
                y: 16.0,
                width: 220.0,
                height: 560.0,
            }
        );
        assert_eq!(
            sidebar_placement_for_work_area(LogicalWorkArea {
                x: -1920.0,
                y: 0.0,
                width: 2560.0,
                height: 1600.0,
            }),
            PickerWindowPlacement {
                x: -1904.0,
                y: 16.0,
                width: 280.0,
                height: 960.0,
            }
        );
    }

    #[test]
    fn bar_placement_spans_width_and_pins_to_bottom() {
        assert_eq!(
            bar_placement_for_work_area(LogicalWorkArea {
                x: 100.0,
                y: 24.0,
                width: 1440.0,
                height: 900.0,
            }),
            PickerWindowPlacement {
                x: 116.0,
                // work_area bottom (24 + 900) minus the bar height (56) and margin (16).
                y: 852.0,
                // full work-area width minus both side margins.
                width: 1408.0,
                height: 56.0,
            }
        );
    }

    #[test]
    fn bar_placement_clamps_narrow_work_area_width() {
        // Narrow work area: width minus margins falls below MIN_WIDTH (220), so the
        // bar is clamped up to the floor instead of shrinking with the screen.
        assert_eq!(
            bar_placement_for_work_area(LogicalWorkArea {
                x: 0.0,
                y: 0.0,
                width: 230.0,
                height: 420.0,
            }),
            PickerWindowPlacement {
                x: 16.0,
                y: 348.0,
                width: 220.0,
                height: 56.0,
            }
        );
        // Large work area: the bar stays a fixed-height ribbon pinned to the bottom.
        assert_eq!(
            bar_placement_for_work_area(LogicalWorkArea {
                x: -1920.0,
                y: 0.0,
                width: 2560.0,
                height: 1600.0,
            }),
            PickerWindowPlacement {
                x: -1904.0,
                y: 1528.0,
                width: 2528.0,
                height: 56.0,
            }
        );
    }

    #[test]
    fn bar_placement_clamps_short_work_area_to_top() {
        // Work area too short to hold the bar + margin: the bottom-anchored y would
        // fall above the work-area top, so it clamps to the top edge instead.
        assert_eq!(
            bar_placement_for_work_area(LogicalWorkArea {
                x: 0.0,
                y: 50.0,
                width: 1440.0,
                height: 60.0,
            }),
            PickerWindowPlacement {
                x: 16.0,
                // 50 + 60 - 56 - 16 = 38, above the work-area top (50), so it clamps
                // up to y = 50.
                y: 50.0,
                width: 1408.0,
                height: 56.0,
            }
        );
    }

    #[test]
    fn settings_placement_centers_within_work_area() {
        // Centered: x = 100 + (1000 - 560)/2 = 320, y = 50 + (800 - 640)/2 = 130.
        let (x, y) = centered_placement_for_work_area(
            LogicalWorkArea {
                x: 100.0,
                y: 50.0,
                width: 1000.0,
                height: 800.0,
            },
            560.0,
            640.0,
        );

        assert_eq!(x, 320.0);
        assert_eq!(y, 130.0);
    }

    #[test]
    fn settings_placement_clamps_oversized_window_to_top_left() {
        // Window larger than the work area: centering would push it off the top/left, so
        // it clamps to the work-area origin instead.
        let (x, y) = centered_placement_for_work_area(
            LogicalWorkArea {
                x: 10.0,
                y: 20.0,
                width: 400.0,
                height: 300.0,
            },
            560.0,
            640.0,
        );

        assert_eq!(x, 10.0);
        assert_eq!(y, 20.0);
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
                "display": { "provider_marker": "💭" }
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
    fn subscribe_keepalive_frame_parses_to_keepalive_variant() {
        // The daemon emits this idle heartbeat ~1/s; the consumer must accept it
        // rather than tear the subscription down with a spurious "Offline, retrying".
        let frame: SubscribeFrame =
            serde_json::from_str(r#"{"type":"keepalive"}"#).expect("keepalive frame parses");

        assert_eq!(frame, SubscribeFrame::Keepalive);
    }

    #[test]
    fn keepalive_frame_maps_to_no_event() {
        // Keepalive is a no-op for the picker: it produces no event and keeps the
        // worker reading the stream.
        let stop = AtomicBool::new(false);
        let event = live_event_from_subscribe_frame(
            &AgentscanRunner::Local(LocalRunnerSettings {
                binary_path: None,
                env: Vec::new(),
            }),
            SubscribeFrame::Keepalive,
            &stop,
        )
        .expect("keepalive maps cleanly");

        assert!(event.is_none(), "keepalive must not emit a picker event");
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
            "exec env AGENTSCAN_TMUX_SOCKET='/tmp/tmux socket' QUOTE='can'\\''t' '/opt/bin/agentscan custom' 'hotkeys' '--format' 'json'"
        );
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
                OsStr::new("exec env 'agentscan' 'subscribe' '--format' 'json'")
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
        assert!(
            classify_desktop_failure(&runner, "focus", "can't find pane: %42")
                .starts_with("Focus target is stale")
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
