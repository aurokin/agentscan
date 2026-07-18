#[cfg(target_os = "macos")]
use std::time::Duration;

use tauri::Manager;

const PICKER_WINDOW_MARGIN: f64 = 16.0;
const PICKER_WINDOW_TARGET_WIDTH: f64 = 360.0;
// The picker opens wide enough for source names and menu controls, while a user
// who wants a tighter strip can pull it in further by hand. Below ~250 the CSS
// flips agent rows to a two-line layout so the title + status keep breathing.
const PICKER_WINDOW_MIN_WIDTH: f64 = 220.0;
const PICKER_WINDOW_MAX_WIDTH: f64 = 520.0;
const PICKER_WINDOW_MIN_HEIGHT: f64 = 560.0;
const PICKER_WINDOW_MAX_HEIGHT: f64 = 960.0;
// Snap height for the horizontal "bar" dock: a short ribbon along the bottom edge,
// sized to the stacked session-label + chip strip (the chrome centers within it) rather
// than a tall slab. This is the inner/content height; a native titlebar (when not in
// frameless mode) sits above it. The frontend locks a PINNED bar to this exact height
// (min == max == BAR_WINDOW_HEIGHT in src/windowOperations.ts) so it only resizes
// horizontally — keep the two values in sync.
const BAR_WINDOW_HEIGHT: f64 = 56.0;
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
pub(crate) fn place_picker_window(window: tauri::Window) -> Result<(), String> {
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
pub(crate) fn place_bar_window(window: tauri::Window) -> Result<(), String> {
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
pub(crate) fn place_settings_window(window: tauri::Window) -> Result<(), String> {
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
pub(crate) fn set_window_glass(
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
        // Bounded like the other cross-thread waits: if the main thread is wedged
        // (or the closure is dropped unrun during shutdown), fail the command
        // instead of parking this worker thread forever.
        rx.recv_timeout(Duration::from_secs(2))
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
pub(crate) fn set_window_decorations(
    window: tauri::WebviewWindow,
    decorations: bool,
) -> Result<(), String> {
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

/// Bring the main dock window back to the foreground. Shared by the second-launch
/// (single-instance) handler and the macOS Dock-reopen handler. The window's close
/// button only hides the window in both framed and frameless mode (see
/// `DockApp.tsx`/`WindowControls.tsx`), so clicking the Dock icon is the reliable way
/// back when the summon hotkey is unavailable; without this, a hidden window could
/// only be recovered by force-quit.
pub(crate) fn reveal_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn tauri_window_configs_stay_in_sync_across_conf_overlays() {
        // tauri.macos.conf.json overlays tauri.conf.json via RFC 7396 merge, and
        // JSON merge-patch REPLACES arrays wholesale — so its `app.windows` must be
        // a hand-synced copy of the base list. Assert they match except for the
        // macOS-only `transparent` flag, so an edit to one file cannot silently
        // drift from the other.
        let read_windows = |name: &str| -> Vec<serde_json::Value> {
            let path = format!("{}/{name}", env!("CARGO_MANIFEST_DIR"));
            let config: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).expect("read tauri config"))
                    .expect("parse tauri config");
            config["app"]["windows"]
                .as_array()
                .expect("app.windows array")
                .clone()
        };

        let base = read_windows("tauri.conf.json");
        let overlay = read_windows("tauri.macos.conf.json");
        assert_eq!(
            base.len(),
            overlay.len(),
            "tauri.conf.json and tauri.macos.conf.json define different window counts"
        );
        for (base_window, overlay_window) in base.iter().zip(overlay.iter()) {
            let mut overlay_window = overlay_window.as_object().expect("window object").clone();
            overlay_window.remove("transparent");
            assert_eq!(
                base_window.as_object().expect("window object"),
                &overlay_window,
                "window {:?} drifted between tauri.conf.json and tauri.macos.conf.json                  (only `transparent` may differ)",
                base_window["label"]
            );
        }
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
                width: 360.0,
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
                width: 360.0,
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
}
