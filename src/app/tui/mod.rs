use std::fs;
use std::io::{Stdout, Write};
use std::path::Path;
use std::time::{Duration, SystemTime};

use crossterm::event::{self, Event};
use crossterm::terminal;

use super::*;
use input::{TuiLoopAction, handle_key_event, is_key_press};
pub(crate) use render::{TuiTerminalSize, render_rows, render_tui_frame_for_size, write_tui_frame};
#[cfg(test)]
pub(crate) use render::{render_error_frame, render_rows_for_width};
#[cfg(test)]
pub(crate) use state::merge_tui_session_panes;
pub(crate) use state::{TuiState, synchronize_key_targets};
use terminal_session::TerminalSession;

mod input;
mod render;
mod state;
mod terminal_session;

const KEY_POLL_INTERVAL: Duration = Duration::from_millis(125);
const TUI_READY_PATH_ENV: &str = "AGENTSCAN_TUI_READY_PATH";
const TUI_DONE_PATH_ENV: &str = "AGENTSCAN_TUI_DONE_PATH";

pub(crate) fn run(args: &TuiArgs) -> Result<()> {
    let result = run_tui_loop(args);
    write_tui_marker_from_env(
        TUI_DONE_PATH_ENV,
        if result.is_ok() { "0\n" } else { "1\n" },
    )?;
    result
}

fn run_tui_loop(args: &TuiArgs) -> Result<()> {
    let mut session = TerminalSession::enter()?;
    let cache_path = cache::cache_path().ok();
    let mut state = TuiState::default();
    let mut last_cache_mtime = cache_path.as_deref().and_then(cache_mtime);

    reload_tui_state(&mut state, args.refresh.refresh, args.all)?;
    draw_tui_frame(&mut session.stdout, &mut state)?;
    write_tui_marker_from_env(TUI_READY_PATH_ENV, "")?;

    loop {
        if event::poll(KEY_POLL_INTERVAL).context("failed to poll tui keyboard input")? {
            let next_action = match event::read().context("failed to read tui event")? {
                Event::Key(key_event) if is_key_press(key_event) => {
                    handle_key_event(&key_event, &mut state)?
                }
                Event::Resize(..) => TuiLoopAction::Redraw,
                _ => TuiLoopAction::Continue,
            };

            match next_action {
                TuiLoopAction::Continue => {}
                TuiLoopAction::Redraw => draw_tui_frame(&mut session.stdout, &mut state)?,
                TuiLoopAction::Close => break,
            }
        }

        let Some(cache_path) = cache_path.as_deref() else {
            continue;
        };
        let current_cache_mtime = cache_mtime(cache_path);
        if current_cache_mtime == last_cache_mtime {
            continue;
        }

        last_cache_mtime = current_cache_mtime;
        reload_tui_state(&mut state, false, args.all)?;
        draw_tui_frame(&mut session.stdout, &mut state)?;
    }

    Ok(())
}

fn reload_tui_state(state: &mut TuiState, refresh: bool, include_all: bool) -> Result<()> {
    match load_tui_panes(refresh, include_all) {
        Ok(panes) => {
            state.replace_panes(panes);
            Ok(())
        }
        Err(error) => {
            state.set_error(format_tui_error(&error));
            Ok(())
        }
    }
}

fn load_tui_panes(refresh: bool, include_all: bool) -> Result<Vec<PaneRecord>> {
    let mut snapshot = cache::load_snapshot(refresh)?;
    cache::filter_snapshot(&mut snapshot, include_all);
    Ok(snapshot.panes)
}

fn write_tui_marker_from_env(env_name: &str, contents: &str) -> Result<()> {
    let Some(path) = env::var_os(env_name) else {
        return Ok(());
    };

    fs::write(&path, contents)
        .with_context(|| format!("failed to write tui marker {}", Path::new(&path).display()))
}

fn draw_tui_frame(stdout: &mut Stdout, state: &mut TuiState) -> Result<()> {
    let frame = render_tui_frame_for_size(state, terminal_size()?);
    write_tui_frame(stdout, &frame)?;
    stdout.flush().context("failed to flush tui frame")
}

fn terminal_size() -> Result<TuiTerminalSize> {
    let (width, height) = terminal::size().context("failed to read tui terminal size")?;
    Ok(TuiTerminalSize { width, height })
}

fn format_tui_error(error: &anyhow::Error) -> String {
    error.to_string()
}

fn cache_mtime(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}
