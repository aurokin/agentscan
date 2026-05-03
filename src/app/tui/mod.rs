use std::fs;
use std::io::{Stdout, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

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

enum TuiEvent {
    Terminal(Event),
    Subscription(daemon::DaemonSubscriptionEvent),
    InputFatal(String),
}

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
    let mut state = TuiState::default();
    state.set_connecting("connecting to daemon".to_string());
    draw_tui_frame(&mut session.stdout, &mut state)?;
    write_tui_marker_from_env(TUI_READY_PATH_ENV, "")?;

    let cancel = Arc::new(AtomicBool::new(false));
    let (events_tx, events_rx) = mpsc::channel();
    spawn_input_worker(events_tx.clone(), cancel.clone());
    spawn_subscription_bridge(events_tx, cancel.clone(), args.auto_start);

    while let Ok(event) = events_rx.recv() {
        match handle_tui_event(event, &mut state, args.all)? {
            TuiLoopAction::Continue => {}
            TuiLoopAction::Redraw => draw_tui_frame(&mut session.stdout, &mut state)?,
            TuiLoopAction::Close => break,
        };
    }

    cancel.store(true, Ordering::Relaxed);
    Ok(())
}

fn spawn_input_worker(events: mpsc::Sender<TuiEvent>, cancel: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        while !cancel.load(Ordering::Relaxed) {
            match event::poll(KEY_POLL_INTERVAL) {
                Ok(true) => match event::read() {
                    Ok(event) => {
                        if events.send(TuiEvent::Terminal(event)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = events.send(TuiEvent::InputFatal(format!(
                            "failed to read tui event: {error}"
                        )));
                        break;
                    }
                },
                Ok(false) => {}
                Err(error) => {
                    let _ = events.send(TuiEvent::InputFatal(format!(
                        "failed to poll tui keyboard input: {error}"
                    )));
                    break;
                }
            }
        }
    });
}

fn spawn_subscription_bridge(
    events: mpsc::Sender<TuiEvent>,
    cancel: Arc<AtomicBool>,
    auto_start: AutoStartArgs,
) {
    let (subscription_tx, subscription_rx) = mpsc::channel();
    daemon::spawn_subscription_worker(
        daemon::AutoStartPolicy::from_args(auto_start),
        subscription_tx,
        cancel.clone(),
    );
    std::thread::spawn(move || {
        while !cancel.load(Ordering::Relaxed) {
            match subscription_rx.recv_timeout(KEY_POLL_INTERVAL) {
                Ok(event) => {
                    if events.send(TuiEvent::Subscription(event)).is_err() {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
}

fn handle_tui_event(
    event: TuiEvent,
    state: &mut TuiState,
    include_all: bool,
) -> Result<TuiLoopAction> {
    match event {
        TuiEvent::Terminal(Event::Key(key_event)) if is_key_press(key_event) => {
            handle_key_event(&key_event, state)
        }
        TuiEvent::Terminal(Event::Resize(..)) => Ok(TuiLoopAction::Redraw),
        TuiEvent::Terminal(_) => Ok(TuiLoopAction::Continue),
        TuiEvent::Subscription(event) => handle_subscription_event(event, state, include_all),
        TuiEvent::InputFatal(message) => Err(anyhow::anyhow!(message)),
    }
}

fn handle_subscription_event(
    event: daemon::DaemonSubscriptionEvent,
    state: &mut TuiState,
    include_all: bool,
) -> Result<TuiLoopAction> {
    match event {
        daemon::DaemonSubscriptionEvent::Connecting { message } => {
            state.set_connecting(message);
            Ok(TuiLoopAction::Redraw)
        }
        daemon::DaemonSubscriptionEvent::Snapshot { mut snapshot } => {
            snapshot::filter_snapshot(&mut snapshot, include_all);
            state.replace_panes(snapshot.panes);
            Ok(TuiLoopAction::Redraw)
        }
        daemon::DaemonSubscriptionEvent::Offline { message, retrying } => {
            state.set_offline(message, retrying);
            Ok(TuiLoopAction::Redraw)
        }
        daemon::DaemonSubscriptionEvent::Shutdown { message } => {
            state.set_shutdown(message);
            Ok(TuiLoopAction::Redraw)
        }
        daemon::DaemonSubscriptionEvent::Fatal { message } => {
            state.set_unavailable(message);
            Ok(TuiLoopAction::Redraw)
        }
    }
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
