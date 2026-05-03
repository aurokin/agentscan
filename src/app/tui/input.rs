use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::*;

pub(super) enum TuiLoopAction {
    Continue,
    Redraw,
    Close,
}

pub(super) fn handle_key_event(
    key_event: &KeyEvent,
    state: &mut TuiState,
) -> Result<TuiLoopAction> {
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

    let Some(selection) = tui_selection_from_key_event(key_event) else {
        return Ok(TuiLoopAction::Continue);
    };

    let Some(target_pane_id) = state.key_targets.get(&selection) else {
        return Ok(TuiLoopAction::Continue);
    };

    let focus_target = tmux::resolve_focus_target(target_pane_id, None)?;
    if focus_target.pane_exists {
        match tmux::focus_tmux_pane(target_pane_id, focus_target.client_tty.as_deref())? {
            tmux::FocusTmuxPaneResult::Focused => {}
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
