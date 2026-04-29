use std::fs;
use std::io::{Stdout, Write};
use std::path::Path;
use std::time::{Duration, SystemTime};

use crossterm::event::{self, Event};
use crossterm::terminal;

use super::*;
use input::{PopupLoopAction, handle_key_event, is_key_press};
pub(crate) use render::{
    PopupTerminalSize, render_popup_frame_for_size, render_rows, write_popup_frame,
};
#[cfg(test)]
pub(crate) use render::{render_error_frame, render_rows_for_width};
#[cfg(test)]
pub(crate) use state::merge_popup_session_panes;
pub(crate) use state::{PopupState, synchronize_key_targets};
use terminal_session::TerminalSession;

mod input;
mod render;
mod state;
mod terminal_session;

const KEY_POLL_INTERVAL: Duration = Duration::from_millis(125);
const POPUP_READY_PATH_ENV: &str = "AGENTSCAN_POPUP_READY_PATH";
const POPUP_DONE_PATH_ENV: &str = "AGENTSCAN_POPUP_DONE_PATH";

pub(crate) fn run(args: &PopupArgs) -> Result<()> {
    let result = run_popup_loop(args);
    write_popup_marker_from_env(
        POPUP_DONE_PATH_ENV,
        if result.is_ok() { "0\n" } else { "1\n" },
    )?;
    result
}

fn run_popup_loop(args: &PopupArgs) -> Result<()> {
    let mut session = TerminalSession::enter()?;
    let cache_path = cache::cache_path().ok();
    let mut state = PopupState::default();
    let mut last_cache_mtime = cache_path.as_deref().and_then(cache_mtime);

    reload_popup_state(&mut state, args.refresh.refresh, args.all)?;
    draw_popup_frame(&mut session.stdout, &mut state)?;
    write_popup_marker_from_env(POPUP_READY_PATH_ENV, "")?;

    loop {
        if event::poll(KEY_POLL_INTERVAL).context("failed to poll popup keyboard input")? {
            let next_action = match event::read().context("failed to read popup event")? {
                Event::Key(key_event) if is_key_press(key_event) => {
                    handle_key_event(&key_event, &mut state)?
                }
                Event::Resize(..) => PopupLoopAction::Redraw,
                _ => PopupLoopAction::Continue,
            };

            match next_action {
                PopupLoopAction::Continue => {}
                PopupLoopAction::Redraw => draw_popup_frame(&mut session.stdout, &mut state)?,
                PopupLoopAction::Close => break,
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
        reload_popup_state(&mut state, false, args.all)?;
        draw_popup_frame(&mut session.stdout, &mut state)?;
    }

    Ok(())
}

fn reload_popup_state(state: &mut PopupState, refresh: bool, include_all: bool) -> Result<()> {
    match load_popup_panes(refresh, include_all) {
        Ok(panes) => {
            state.replace_panes(panes);
            Ok(())
        }
        Err(error) => {
            state.set_error(format_popup_error(&error));
            Ok(())
        }
    }
}

fn load_popup_panes(refresh: bool, include_all: bool) -> Result<Vec<PaneRecord>> {
    let mut snapshot = cache::load_snapshot(refresh)?;
    cache::filter_snapshot(&mut snapshot, include_all);
    Ok(snapshot.panes)
}

fn write_popup_marker_from_env(env_name: &str, contents: &str) -> Result<()> {
    let Some(path) = env::var_os(env_name) else {
        return Ok(());
    };

    fs::write(&path, contents).with_context(|| {
        format!(
            "failed to write popup marker {}",
            Path::new(&path).display()
        )
    })
}

fn draw_popup_frame(stdout: &mut Stdout, state: &mut PopupState) -> Result<()> {
    let frame = render_popup_frame_for_size(state, terminal_size()?);
    write_popup_frame(stdout, &frame)?;
    stdout.flush().context("failed to flush popup frame")
}

fn terminal_size() -> Result<PopupTerminalSize> {
    let (width, height) = terminal::size().context("failed to read popup terminal size")?;
    Ok(PopupTerminalSize { width, height })
}

fn format_popup_error(error: &anyhow::Error) -> String {
    error.to_string()
}

fn cache_mtime(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}
