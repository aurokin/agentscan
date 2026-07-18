use crate::{
    contract::PickerRow,
    runner::{
        AgentscanPreflight, AgentscanRunner, DaemonPollResult, DesktopRunnerSettings,
        focus_picker_row_with_runner, load_picker_rows_with_runner, poll_daemon_status_with_runner,
        run_agentscan_preflight_with_runner,
    },
    subscribe::{start_live_picker_with_runner, stop_live_picker_supervisor_for_epoch},
};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DesktopProfile {
    id: &'static str,
    name: &'static str,
    kind: &'static str,
}

#[tauri::command]
pub(crate) fn local_profiles() -> Vec<DesktopProfile> {
    vec![DesktopProfile {
        id: "local",
        name: "Local",
        kind: "local",
    }]
}

/// The text before the first `.` in a hostname — the short, human form users name
/// their machine by ("koopa" from "koopa.home.arpa"). An empty or dotless host
/// passes through unchanged.
pub(crate) fn short_host_label(full: &str) -> &str {
    full.split('.').next().unwrap_or(full)
}

/// The local machine's short hostname, used as the label for the local source the
/// way a remote source is keyed by its SSH host. Returns an empty string if the
/// hostname can't be read, so the frontend can fall back to a generic label.
#[tauri::command]
pub(crate) fn local_host_label() -> String {
    short_host_label(&gethostname::gethostname().to_string_lossy()).to_string()
}

#[tauri::command]
pub(crate) fn preflight_agentscan(settings: Option<DesktopRunnerSettings>) -> AgentscanPreflight {
    let runner = AgentscanRunner::from_settings(settings);
    run_agentscan_preflight_with_runner(&runner)
}

#[tauri::command]
pub(crate) fn load_picker_rows(
    settings: Option<DesktopRunnerSettings>,
) -> Result<Vec<PickerRow>, String> {
    let runner = AgentscanRunner::from_settings(settings);
    load_picker_rows_with_runner(&runner)
}

#[tauri::command]
pub(crate) fn poll_daemon_status(
    settings: Option<DesktopRunnerSettings>,
) -> Result<DaemonPollResult, String> {
    let runner = AgentscanRunner::from_settings(settings);
    poll_daemon_status_with_runner(&runner)
}

#[tauri::command]
pub(crate) fn focus_picker_row(
    pane_id: String,
    settings: Option<DesktopRunnerSettings>,
) -> Result<(), String> {
    let runner = AgentscanRunner::from_settings(settings);
    focus_picker_row_with_runner(&runner, &pane_id)
}

#[tauri::command]
pub(crate) fn start_live_picker(
    app: tauri::AppHandle,
    settings: Option<DesktopRunnerSettings>,
    // The frontend's runnerKey for this subscription's source. Starting replaces
    // only this key's supervisor; other sources' workers keep streaming.
    source_key: String,
    epoch: u64,
    // Latch policy: the desktop owns daemon lifecycle, so reconnect/latch attempts
    // pass `false` (subscribe with `--no-auto-start`, connecting only if a daemon is
    // already running). Only an explicit user "Start agentscan" passes `true`.
    auto_start: bool,
) -> Result<(), String> {
    let runner = AgentscanRunner::from_settings(settings);
    start_live_picker_with_runner(app, runner, source_key, epoch, auto_start)
}

#[tauri::command]
pub(crate) fn stop_live_picker(source_key: String, epoch: u64) -> Result<(), String> {
    // Epoch-guarded so a stale stop (e.g. from a reloaded/HMR'd frontend whose
    // async cleanup arrives after a newer subscription has started) cannot tear
    // down the current worker. Only stop this key's supervisor, and only if it
    // is running this epoch.
    stop_live_picker_supervisor_for_epoch(&source_key, Some(epoch))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn short_host_label_uses_text_before_first_dot() {
        assert_eq!(short_host_label("koopa.home.arpa"), "koopa");
        assert_eq!(short_host_label("koopa"), "koopa");
        assert_eq!(short_host_label(""), "");
    }
}
