use std::collections::{BTreeMap, HashMap};
use std::io::Write;

use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::terminal::{Clear, ClearType};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::state::{
    TuiConnectionKind, TuiConnectionState, last_non_empty_page_start, page_count,
    page_size_for_terminal, synchronize_key_targets,
};
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TuiTerminalSize {
    pub(crate) width: u16,
    pub(crate) height: u16,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct TuiFrame {
    pub(crate) lines: Vec<String>,
    pub(crate) visible_pane_ids: Vec<String>,
    pub(crate) page_start: usize,
    pub(crate) page_size: usize,
    pub(crate) page_count: usize,
}

pub(crate) fn write_tui_frame<W: Write>(writer: &mut W, frame: &TuiFrame) -> Result<()> {
    queue!(writer, MoveTo(0, 0), Clear(ClearType::All)).context("failed to clear tui frame")?;
    for (row, line) in frame.lines.iter().enumerate() {
        queue!(
            writer,
            MoveTo(0, row as u16),
            crossterm::style::Print(line),
            Clear(ClearType::UntilNewLine)
        )
        .context("failed to queue tui line")?;
    }
    Ok(())
}

pub(crate) fn render_tui_frame_for_size(
    state: &mut TuiState,
    terminal_size: TuiTerminalSize,
) -> TuiFrame {
    state.set_terminal_size(terminal_size);

    if let Some(error_message) = state.error_message.as_deref() {
        state.key_targets.clear();
        let lines = render_body_with_footer(
            &render_error_frame(error_message),
            &state.connection,
            terminal_size,
        );
        return TuiFrame {
            lines,
            visible_pane_ids: Vec::new(),
            page_start: 0,
            page_size: 0,
            page_count: 0,
        };
    }

    if state.panes.is_empty() {
        state.key_targets.clear();
        state.page_start = 0;
        let body = if state.connection.kind == TuiConnectionKind::Connecting {
            vec![
                "Connecting to agentscan daemon...".to_string(),
                state.connection.message.clone(),
            ]
        } else {
            vec![
                "No panes available in current snapshot.".to_string(),
                "Press Esc or Ctrl-C to close.".to_string(),
            ]
        };
        return TuiFrame {
            lines: render_body_with_footer(&body, &state.connection, terminal_size),
            visible_pane_ids: Vec::new(),
            page_start: 0,
            page_size: 0,
            page_count: 0,
        };
    }

    let page_size = page_size_for_terminal(terminal_size);
    if page_size == 0 {
        state.key_targets.clear();
        return TuiFrame {
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
        &state.connection,
        row_width,
    ));

    TuiFrame {
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
    let sanitized_label = sanitize_tui_label(pane.display.label.as_str());
    format_row_with_trailing_label(&prefix, sanitized_label.as_str(), width)
}

fn render_footer_lines(
    page_start: usize,
    page_size: usize,
    total_panes: usize,
    connection: &TuiConnectionState,
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

    let first_line = footer_line_with_indicator(
        "Select with highlighted key. Ctrl-B tmux prefix. Esc/Ctrl-C close.",
        connection.indicator(),
        width,
    );

    let second_line = if page_count > 1 {
        footer_line_with_indicator(
            format!(
                "Page {page_number}/{page_count} | {shown_count}/{total_panes} shown | N/P, Left/Right, or PgUp/PgDn."
            )
            .as_str(),
            connection.message.as_str(),
            width,
        )
    } else {
        footer_line_with_indicator(
            format!("Page 1/1 | {shown_count}/{total_panes} shown").as_str(),
            connection.message.as_str(),
            width,
        )
    };

    vec![first_line, second_line]
}

fn render_undersized_frame() -> Vec<String> {
    vec![
        "TUI too small for pane selection.".to_string(),
        "Resize the TUI, then choose a pane.".to_string(),
        "Press Esc or Ctrl-C to close.".to_string(),
    ]
}

pub(crate) fn render_error_frame(error_message: &str) -> Vec<String> {
    vec![
        "agentscan tui unavailable".to_string(),
        String::new(),
        error_message.to_string(),
        String::new(),
        "Run `agentscan daemon status` to inspect daemon health.".to_string(),
        "Press Esc or Ctrl-C to close.".to_string(),
    ]
}

fn render_body_with_footer(
    body: &[String],
    connection: &TuiConnectionState,
    terminal_size: TuiTerminalSize,
) -> Vec<String> {
    let width = usize::from(terminal_size.width);
    let height = usize::from(terminal_size.height);
    if height == 0 {
        return Vec::new();
    }

    let footer = vec![
        footer_line_with_indicator("Esc/Ctrl-C close.", connection.indicator(), width),
        footer_line_with_indicator(connection.message.as_str(), "", width),
    ];
    let body_height = height.saturating_sub(footer.len());
    let mut lines = body
        .iter()
        .take(body_height)
        .map(|line| truncate_to_width(line, width))
        .collect::<Vec<_>>();
    lines.extend(footer);
    lines.truncate(height);
    lines
}

fn fit_lines_to_terminal(lines: &[String], terminal_size: TuiTerminalSize) -> Vec<String> {
    let width = usize::from(terminal_size.width);
    lines
        .iter()
        .take(usize::from(terminal_size.height))
        .map(|line| truncate_to_width(line, width))
        .collect()
}

fn footer_line_with_indicator(left: &str, indicator: &str, width: usize) -> String {
    if width == usize::MAX {
        return if indicator.is_empty() {
            left.to_string()
        } else if left.is_empty() {
            indicator.to_string()
        } else {
            format!("{left} {indicator}")
        };
    }
    if width == 0 {
        return String::new();
    }

    let indicator_width = display_width(indicator);
    if indicator_width == 0 {
        return truncate_to_width(left, width);
    }
    if indicator_width >= width {
        return truncate_to_width(indicator, width);
    }

    let left_width = width.saturating_sub(indicator_width + 1);
    let left = truncate_to_width(left, left_width);
    let used_width = display_width(left.as_str());
    let spaces = width.saturating_sub(used_width + indicator_width).max(1);
    format!("{left}{}{indicator}", " ".repeat(spaces))
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

fn sanitize_tui_label(label: &str) -> String {
    let mut sanitized = String::with_capacity(label.len());
    let mut characters = label.chars().peekable();
    let mut last_was_space = false;

    while let Some(character) = characters.next() {
        match character {
            '\u{1b}' => {
                strip_terminal_escape_sequence(&mut characters);
            }
            '\n' | '\r' | '\t' => push_sanitized_space(&mut sanitized, &mut last_was_space),
            character if is_tui_disallowed_control(character) => {}
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

fn is_tui_disallowed_control(character: char) -> bool {
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
