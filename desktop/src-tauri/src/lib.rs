mod commands;
mod contract;
mod runner;
mod subscribe;
mod windowing;

use windowing::reveal_main_window;

pub fn run() {
    let builder = tauri::Builder::default();

    // Registered first, per the plugin's contract, so a second launch is
    // caught before anything else initializes. macOS grants a global hotkey
    // to one process, so a competing copy would silently lose the summon key;
    // surface the instance that already owns it instead. Release-only: dev
    // builds share the bundle identifier, so an unconditional lock would make
    // `tauri dev` defer to a running installed copy instead of starting. The
    // intentional dev-beside-release pair is handled by the summon hotkey's
    // in-use banner and retry loop, not by refusing to run.
    #[cfg(not(debug_assertions))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
        reveal_main_window(app);
    }));

    builder
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            commands::focus_picker_row,
            commands::local_host_label,
            commands::local_profiles,
            commands::load_picker_rows,
            windowing::place_bar_window,
            windowing::place_picker_window,
            windowing::place_settings_window,
            commands::poll_daemon_status,
            commands::preflight_agentscan,
            windowing::set_window_decorations,
            windowing::set_window_glass,
            commands::start_live_picker,
            commands::stop_live_picker
        ])
        .build(tauri::generate_context!())
        .expect("error while running agentscan desktop")
        .run(|app, event| {
            // macOS fires Reopen when the user clicks the Dock icon while the app is
            // already running. The close button only hides the window, so this is the
            // recovery path that reshows it when the summon hotkey is unavailable —
            // the alternative is a force-quit.
            if let tauri::RunEvent::Reopen { .. } = event {
                reveal_main_window(app);
            }
        });
}
