use std::collections::{BTreeMap, HashMap};
use std::io::Write;

use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::terminal::{Clear, ClearType};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::state::{
    last_non_empty_page_start, page_count, page_size_for_terminal, synchronize_key_targets,
};
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PopupTerminalSize {
    pub(crate) width: u16,
    pub(crate) height: u16,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct PopupFrame {
    pub(crate) lines: Vec<String>,
    pub(crate) visible_pane_ids: Vec<String>,
    pub(crate) page_start: usize,
    pub(crate) page_size: usize,
    pub(crate) page_count: usize,
}

pub(crate) fn write_popup_frame<W: Write>(writer: &mut W, frame: &PopupFrame) -> Result<()> {
    queue!(writer, MoveTo(0, 0), Clear(ClearType::All)).context("failed to clear popup frame")?;
    for (row, line) in frame.lines.iter().enumerate() {
        queue!(
            writer,
            MoveTo(0, row as u16),
            crossterm::style::Print(line),
            Clear(ClearType::UntilNewLine)
        )
        .context("failed to queue popup line")?;
    }
    Ok(())
}

pub(crate) fn render_popup_frame_for_size(
    state: &mut PopupState,
    terminal_size: PopupTerminalSize,
) -> PopupFrame {
    state.set_terminal_size(terminal_size);

    if let Some(error_message) = state.error_message.as_deref() {
        state.key_targets.clear();
        return PopupFrame {
            lines: fit_lines_to_terminal(&render_error_frame(error_message), terminal_size),
            visible_pane_ids: Vec::new(),
            page_start: 0,
            page_size: 0,
            page_count: 0,
        };
    }

    if state.panes.is_empty() {
        state.key_targets.clear();
        state.page_start = 0;
        return PopupFrame {
            lines: fit_lines_to_terminal(
                &[
                    "No panes available in cache.".to_string(),
                    "Press Esc or Ctrl-C to close.".to_string(),
                ],
                terminal_size,
            ),
            visible_pane_ids: Vec::new(),
            page_start: 0,
            page_size: 0,
            page_count: 0,
        };
    }

    let page_size = page_size_for_terminal(terminal_size);
    if page_size == 0 {
        state.key_targets.clear();
        return PopupFrame {
            lines: fit_lines_to_terminal(&render_undersized_frame(), terminal_size),
            visible_pane_ids: Vec::new(),
            page_start: state.page_start,
            page_size: 0,
            page_count: 0,
        };
    }

    if state.page_start >= state.panes.len() {
        state.page_start = last_non_empty_page_start(state.panes.len(), page_size);
    }

    let visible_end = state
        .page_start
        .saturating_add(page_size)
        .min(state.panes.len());
    let visible_panes = &state.panes[state.page_start..visible_end];
    synchronize_key_targets(&mut state.key_targets, visible_panes);

    let visible_pane_ids = visible_panes
        .iter()
        .map(|pane| pane.pane_id.clone())
        .collect::<Vec<_>>();
    let row_width = usize::from(terminal_size.width);
    let mut lines = render_rows_for_width(visible_panes, &state.key_targets, row_width);
    lines.extend(render_footer_lines(
        state.page_start,
        page_size,
        state.panes.len(),
        row_width,
    ));

    PopupFrame {
        lines,
        visible_pane_ids,
        page_start: state.page_start,
        page_size,
        page_count: page_count(state.panes.len(), page_size),
    }
}

pub(crate) fn render_rows(
    panes: &[PaneRecord],
    key_targets: &BTreeMap<char, String>,
) -> Vec<String> {
    render_rows_for_width(panes, key_targets, usize::MAX)
}

pub(crate) fn render_rows_for_width(
    panes: &[PaneRecord],
    key_targets: &BTreeMap<char, String>,
    width: usize,
) -> Vec<String> {
    let key_labels: HashMap<&str, char> = key_targets
        .iter()
        .map(|(key, pane_id)| (pane_id.as_str(), *key))
        .collect();

    panes
        .iter()
        .map(|pane| render_pane_row(pane, key_labels.get(pane.pane_id.as_str()).copied(), width))
        .collect()
}

fn render_pane_row(pane: &PaneRecord, selection: Option<char>, width: usize) -> String {
    let selection_label = selection
        .map(|assigned_key| format!("[{assigned_key}]"))
        .unwrap_or_else(|| "   ".to_string());
    let provider = provider_display_marker(pane.provider);
    let prefix = format!(
        "{} {} {} {} - ",
        selection_label,
        status_emoji(pane.status.kind),
        provider,
        pane.location.tag()
    );
    let sanitized_label = sanitize_popup_label(pane.display.label.as_str());
    format_row_with_trailing_label(&prefix, sanitized_label.as_str(), width)
}

fn render_footer_lines(
    page_start: usize,
    page_size: usize,
    total_panes: usize,
    width: usize,
) -> Vec<String> {
    let page_count = page_count(total_panes, page_size);
    let page_number = if page_count == 0 {
        0
    } else {
        let last_visible_index = page_start
            .saturating_add(page_size)
            .min(total_panes)
            .saturating_sub(1);
        (last_visible_index / page_size)
            .saturating_add(1)
            .min(page_count)
    };
    let shown_count = total_panes.saturating_sub(page_start).min(page_size.max(1));

    let first_line = truncate_to_width(
        "Select with highlighted key. Ctrl-B tmux prefix. Esc/Ctrl-C close.",
        width,
    );

    let second_line = if page_count > 1 {
        truncate_to_width(
            format!(
                "Page {page_number}/{page_count} | {shown_count}/{total_panes} shown | N/P, Left/Right, or PgUp/PgDn."
            )
            .as_str(),
            width,
        )
    } else {
        truncate_to_width(
            format!("Page 1/1 | {shown_count}/{total_panes} shown").as_str(),
            width,
        )
    };

    vec![first_line, second_line]
}

fn render_undersized_frame() -> Vec<String> {
    vec![
        "Popup too small for pane selection.".to_string(),
        "Resize the popup, then choose a pane.".to_string(),
        "Press Esc or Ctrl-C to close.".to_string(),
    ]
}

pub(crate) fn render_error_frame(error_message: &str) -> Vec<String> {
    vec![
        "agentscan popup unavailable".to_string(),
        String::new(),
        error_message.to_string(),
        String::new(),
        "Run `agentscan popup --refresh` for a one-shot tmux snapshot.".to_string(),
        "Run `agentscan daemon run` for normal cached use.".to_string(),
        "Press Esc or Ctrl-C to close.".to_string(),
    ]
}

fn fit_lines_to_terminal(lines: &[String], terminal_size: PopupTerminalSize) -> Vec<String> {
    let width = usize::from(terminal_size.width);
    lines
        .iter()
        .take(usize::from(terminal_size.height))
        .map(|line| truncate_to_width(line, width))
        .collect()
}

fn format_row_with_trailing_label(prefix: &str, label: &str, width: usize) -> String {
    if width == usize::MAX {
        return format!("{prefix}{label}");
    }

    let prefix_width = display_width(prefix);
    if prefix_width >= width {
        return truncate_to_width(prefix, width);
    }

    let remaining_width = width - prefix_width;
    format!("{prefix}{}", truncate_to_width(label, remaining_width))
}

fn sanitize_popup_label(label: &str) -> String {
    let mut sanitized = String::with_capacity(label.len());
    let mut characters = label.chars().peekable();
    let mut last_was_space = false;

    while let Some(character) = characters.next() {
        match character {
            '\u{1b}' => {
                strip_terminal_escape_sequence(&mut characters);
            }
            '\n' | '\r' | '\t' => push_sanitized_space(&mut sanitized, &mut last_was_space),
            character if is_popup_disallowed_control(character) => {}
            character => {
                sanitized.push(character);
                last_was_space = false;
            }
        }
    }

    sanitized.trim().to_string()
}

fn strip_terminal_escape_sequence<I>(characters: &mut std::iter::Peekable<I>)
where
    I: Iterator<Item = char>,
{
    let Some(next_character) = characters.next() else {
        return;
    };

    match next_character {
        '[' => {
            for character in characters.by_ref() {
                if ('@'..='~').contains(&character) {
                    break;
                }
            }
        }
        ']' => {
            let mut previous_was_escape = false;
            for character in characters.by_ref() {
                if character == '\u{7}' || (previous_was_escape && character == '\\') {
                    break;
                }
                previous_was_escape = character == '\u{1b}';
            }
        }
        'P' | '_' | '^' => {
            let mut previous_was_escape = false;
            for character in characters.by_ref() {
                if previous_was_escape && character == '\\' {
                    break;
                }
                previous_was_escape = character == '\u{1b}';
            }
        }
        _ => {}
    }
}

fn push_sanitized_space(output: &mut String, last_was_space: &mut bool) {
    if *last_was_space {
        return;
    }

    output.push(' ');
    *last_was_space = true;
}

fn is_popup_disallowed_control(character: char) -> bool {
    character.is_control()
}

fn truncate_to_width(input: &str, width: usize) -> String {
    if width == usize::MAX {
        return input.to_string();
    }
    if width == 0 {
        return String::new();
    }

    let rendered_width = display_width(input);
    if rendered_width <= width {
        return input.to_string();
    }
    if width == 1 {
        return "…".to_string();
    }

    let mut truncated = String::new();
    let mut used_width = 0;
    for character in input.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used_width + character_width + 1 > width {
            break;
        }
        truncated.push(character);
        used_width += character_width;
    }

    if truncated.is_empty() {
        "…".to_string()
    } else {
        format!("{truncated}…")
    }
}

fn display_width(input: &str) -> usize {
    UnicodeWidthStr::width(input)
}

fn status_emoji(status: StatusKind) -> &'static str {
    match status {
        StatusKind::Idle => "🟢",
        StatusKind::Busy => "🟡",
        StatusKind::Unknown => "⚫",
    }
}
