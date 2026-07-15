use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::*;

pub(crate) enum TuiLoopAction {
    Continue,
    Redraw,
    Close,
}

pub(crate) fn handle_key_event(
    key_event: &KeyEvent,
    state: &mut TuiState,
) -> Result<TuiLoopAction> {
    if state.is_searching() {
        return handle_search_key_event(key_event, state);
    }

    if is_tui_close_key(key_event) {
        return Ok(TuiLoopAction::Close);
    }

    if key_event.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key_event.code, KeyCode::Char('b') | KeyCode::Char('B'))
    {
        tmux::switch_tmux_client_to_prefix(None)?;
        return Ok(TuiLoopAction::Continue);
    }

    if is_tui_previous_page_key(key_event) {
        return Ok(if state.previous_page() {
            TuiLoopAction::Redraw
        } else {
            TuiLoopAction::Continue
        });
    }

    if is_tui_next_page_key(key_event) {
        return Ok(if state.next_page() {
            TuiLoopAction::Redraw
        } else {
            TuiLoopAction::Continue
        });
    }

    if matches!(key_event.code, KeyCode::Up) {
        return Ok(if state.select_previous() {
            TuiLoopAction::Redraw
        } else {
            TuiLoopAction::Continue
        });
    }

    if matches!(key_event.code, KeyCode::Down) {
        return Ok(if state.select_next() {
            TuiLoopAction::Redraw
        } else {
            TuiLoopAction::Continue
        });
    }

    if matches!(key_event.code, KeyCode::Enter) {
        return activate_selection(state);
    }

    if matches!(key_event.code, KeyCode::Char('/'))
        && !key_event
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return Ok(if state.begin_search() {
            TuiLoopAction::Redraw
        } else {
            TuiLoopAction::Continue
        });
    }

    let Some(selection) = tui_selection_from_key_event(key_event) else {
        return Ok(TuiLoopAction::Continue);
    };

    let target_pane_id = state
        .key_targets
        .get(&selection)
        .or_else(|| state.retired_key_targets.get(&selection));
    let Some(target_pane_id) = target_pane_id else {
        return Ok(TuiLoopAction::Continue);
    };

    focus_pane_and_close(target_pane_id)
}

// Search mode owns the key loop: characters edit the query instead of firing
// letter hotkeys, Esc cancels back to the full list instead of closing, and
// Ctrl-C stays the unconditional close key.
fn handle_search_key_event(key_event: &KeyEvent, state: &mut TuiState) -> Result<TuiLoopAction> {
    if key_event.modifiers.contains(KeyModifiers::CONTROL) {
        if matches!(key_event.code, KeyCode::Char('c') | KeyCode::Char('C')) {
            return Ok(TuiLoopAction::Close);
        }
        if matches!(key_event.code, KeyCode::Char('b') | KeyCode::Char('B')) {
            tmux::switch_tmux_client_to_prefix(None)?;
            return Ok(TuiLoopAction::Continue);
        }
    }

    let handled = match key_event.code {
        KeyCode::Esc => state.cancel_search(),
        KeyCode::Up => state.select_previous(),
        KeyCode::Down => state.select_next(),
        KeyCode::Left | KeyCode::PageUp => state.previous_page(),
        KeyCode::Right | KeyCode::PageDown => state.next_page(),
        KeyCode::Enter => return activate_selection(state),
        KeyCode::Backspace => state.pop_search_char(),
        KeyCode::Char(character)
            if !key_event
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            state.push_search_char(character)
        }
        _ => false,
    };

    Ok(if handled {
        TuiLoopAction::Redraw
    } else {
        TuiLoopAction::Continue
    })
}

fn activate_selection(state: &mut TuiState) -> Result<TuiLoopAction> {
    let Some(target_pane_id) = state.selected_pane_id.clone() else {
        return Ok(TuiLoopAction::Continue);
    };
    focus_pane_and_close(&target_pane_id)
}

fn focus_pane_and_close(target_pane_id: &str) -> Result<TuiLoopAction> {
    let focus_target = tmux::resolve_focus_target(target_pane_id, None)?;
    if focus_target.pane_exists {
        match tmux::focus_tmux_pane(target_pane_id, focus_target.client_tty.as_deref())? {
            tmux::FocusTmuxPaneResult::Focused => {
                daemon::emit_pane_focus_event_best_effort(target_pane_id);
            }
            tmux::FocusTmuxPaneResult::Missing => {
                tmux::display_tmux_message(
                    focus_target.client_tty.as_deref(),
                    &format!("pane {} is no longer available", target_pane_id),
                )?;
            }
        }
    } else {
        tmux::display_tmux_message(
            focus_target.client_tty.as_deref(),
            &format!("pane {} is no longer available", target_pane_id),
        )?;
    }

    Ok(TuiLoopAction::Close)
}

fn is_tui_close_key(key_event: &KeyEvent) -> bool {
    matches!(key_event.code, KeyCode::Esc)
        || (key_event.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key_event.code, KeyCode::Char('c') | KeyCode::Char('C')))
}

fn is_tui_previous_page_key(key_event: &KeyEvent) -> bool {
    matches!(key_event.code, KeyCode::Left | KeyCode::PageUp)
        || matches!(key_event.code, KeyCode::Char('p' | 'P'))
}

fn is_tui_next_page_key(key_event: &KeyEvent) -> bool {
    matches!(key_event.code, KeyCode::Right | KeyCode::PageDown)
        || matches!(key_event.code, KeyCode::Char('n' | 'N'))
}

fn tui_selection_from_key_event(key_event: &KeyEvent) -> Option<char> {
    match key_event.code {
        KeyCode::Char(character) => Some(character.to_ascii_uppercase()),
        _ => None,
    }
}

pub(super) fn is_key_press(key_event: KeyEvent) -> bool {
    match key_event.kind {
        KeyEventKind::Press | KeyEventKind::Repeat => true,
        KeyEventKind::Release => false,
    }
}
