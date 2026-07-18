use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;

use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::{Attribute, SetAttribute};
use crossterm::terminal::{Clear, ClearType};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::state::{
    TuiConnectionKind, TuiConnectionState, last_non_empty_page_start, page_count,
    page_size_for_terminal, synchronize_key_targets_with_keys,
};
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TuiTerminalSize {
    pub(crate) width: u16,
    pub(crate) height: u16,
}

// Selected-row pointer shown in the one-cell gutter every picker row carries.
pub(crate) const TUI_SELECTED_POINTER: char = '❯';

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct TuiFrame {
    pub(crate) lines: Vec<String>,
    pub(crate) visible_pane_ids: Vec<String>,
    pub(crate) page_start: usize,
    pub(crate) page_size: usize,
    pub(crate) page_count: usize,
    pub(crate) selected_row: Option<usize>,
}

pub(crate) fn write_tui_frame<W: Write>(writer: &mut W, frame: &TuiFrame) -> Result<()> {
    queue!(writer, MoveTo(0, 0), Clear(ClearType::All)).context("failed to clear tui frame")?;
    for (row, line) in frame.lines.iter().enumerate() {
        queue!(writer, MoveTo(0, row as u16)).context("failed to queue tui line")?;
        if frame.selected_row == Some(row) {
            // The selected row is marked by the gutter pointer (part of the
            // line string) plus bold weight applied here at write time — no
            // reverse video or background fill, so the highlight inherits the
            // terminal theme instead of inverting it.
            queue!(
                writer,
                SetAttribute(Attribute::Bold),
                crossterm::style::Print(line),
                SetAttribute(Attribute::NormalIntensity)
            )
            .context("failed to queue tui line")?;
        } else {
            queue!(writer, crossterm::style::Print(line)).context("failed to queue tui line")?;
        }
        queue!(writer, Clear(ClearType::UntilNewLine)).context("failed to queue tui line")?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn render_tui_frame_for_size(
    state: &mut TuiState,
    terminal_size: TuiTerminalSize,
) -> TuiFrame {
    render_tui_frame_for_size_with_icons(state, terminal_size, IconMode::Emoji)
}

pub(crate) fn render_tui_frame_for_size_with_icons(
    state: &mut TuiState,
    terminal_size: TuiTerminalSize,
    icon_mode: IconMode,
) -> TuiFrame {
    state.set_terminal_size(terminal_size);

    if let Some(error_message) = state.error_message.as_deref() {
        state.key_targets.clear();
        state.retired_key_targets.clear();
        state.selected_pane_id = None;
        // Search mode exists only while the pane list is on screen; dropping it
        // on non-list frames keeps Esc's close semantics unambiguous there.
        state.search_query = None;
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
            selected_row: None,
        };
    }

    if state.panes.is_empty() {
        state.key_targets.clear();
        state.retired_key_targets.clear();
        state.selected_pane_id = None;
        state.search_query = None;
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
            selected_row: None,
        };
    }

    let page_size = page_size_for_terminal(terminal_size, &state.picker_keys);
    if page_size == 0 {
        state.key_targets.clear();
        state.retired_key_targets.clear();
        state.selected_pane_id = None;
        state.search_query = None;
        return TuiFrame {
            lines: fit_lines_to_terminal(&render_undersized_frame(), terminal_size),
            visible_pane_ids: Vec::new(),
            page_start: state.page_start,
            page_size: 0,
            page_count: 0,
            selected_row: None,
        };
    }

    let row_width = usize::from(terminal_size.width);
    let view = state.view_pane_indices();

    // Panes exist but the search query filters them all out.
    if view.is_empty() {
        state.page_start = 0;
        reconcile_selection(&mut state.selected_pane_id, &[]);
        let query = state.search_query.as_deref().unwrap_or_default();
        let mut lines = vec![
            truncate_to_width(&format!("No panes match \"{query}\"."), row_width),
            truncate_to_width("Backspace edits the query. Esc cancels search.", row_width),
        ];
        lines.extend(render_search_footer_lines(
            query,
            0,
            page_size,
            0,
            state.panes.len(),
            &state.connection,
            row_width,
        ));
        lines.truncate(usize::from(terminal_size.height));
        return TuiFrame {
            lines,
            visible_pane_ids: Vec::new(),
            page_start: 0,
            page_size,
            page_count: 0,
            selected_row: None,
        };
    }

    if state.page_start >= view.len() {
        state.page_start = last_non_empty_page_start(view.len(), page_size);
    }

    // One-shot initial-selection seed (caller-pane hint, then focus
    // recency). Placed after the clamp and before the visible window is
    // computed so a seed beyond page one repositions `page_start` first.
    state.seed_initial_selection(&view, page_size);

    let visible_end = state.page_start.saturating_add(page_size).min(view.len());
    if state.search_query.is_some() {
        // Letter hotkeys are suspended while searching: typed characters edit
        // the query, so rows carry no key labels and no targets are assigned.
        state.key_targets.clear();
        state.retired_key_targets.clear();
        state.reset_key_targets_on_next_render = false;
    } else {
        // Outside search mode the view is the identity over `panes`, so the
        // visible view positions are the same contiguous pane range as before.
        let visible_panes = &state.panes[state.page_start..visible_end];
        let previous_key_targets = state.key_targets.clone();
        if state.reset_key_targets_on_next_render {
            state.key_targets.clear();
            state.reset_key_targets_on_next_render = false;
        }
        synchronize_key_targets_with_keys(
            &mut state.key_targets,
            visible_panes,
            &state.picker_keys,
        );
        let current_pane_ids = state
            .panes
            .iter()
            .map(|pane| pane.pane_id.as_str())
            .collect::<HashSet<_>>();
        for (key, pane_id) in previous_key_targets {
            if !state.key_targets.contains_key(&key) && !current_pane_ids.contains(pane_id.as_str())
            {
                state.retired_key_targets.insert(key, pane_id);
            }
        }
        for key in state.key_targets.keys() {
            state.retired_key_targets.remove(key);
        }
    }

    let visible_panes = view[state.page_start..visible_end]
        .iter()
        .map(|&pane_index| &state.panes[pane_index])
        .collect::<Vec<_>>();
    let visible_pane_ids = visible_panes
        .iter()
        .map(|pane| pane.pane_id.clone())
        .collect::<Vec<_>>();
    let selected_row = reconcile_selection(&mut state.selected_pane_id, &visible_pane_ids);
    // Rows leave one cell for the selection gutter: the selected row carries
    // the pointer there, other rows a space, so text stays aligned as the
    // selection moves.
    let mut lines = render_rows_for_width_with_location_labels_and_icons(
        &visible_panes,
        &state.key_targets,
        row_width.saturating_sub(1),
        &state.pane_location_labels,
        icon_mode,
    );
    if row_width > 0 {
        for (row, line) in lines.iter_mut().enumerate() {
            let gutter = if selected_row == Some(row) {
                TUI_SELECTED_POINTER
            } else {
                ' '
            };
            line.insert(0, gutter);
        }
    }
    lines.extend(if let Some(query) = state.search_query.as_deref() {
        render_search_footer_lines(
            query,
            state.page_start,
            page_size,
            view.len(),
            state.panes.len(),
            &state.connection,
            row_width,
        )
    } else {
        render_footer_lines(
            state.page_start,
            page_size,
            state.panes.len(),
            &state.connection,
            row_width,
        )
    });

    TuiFrame {
        lines,
        visible_pane_ids,
        page_start: state.page_start,
        page_size,
        page_count: page_count(view.len(), page_size),
        selected_row,
    }
}

// Selection is pane-id anchored so live updates keep it on the same pane. When
// the pane is gone — or still exists but a live update pushed it off the visible
// page — the highlight deliberately snaps to the first visible row rather than
// re-paging to chase it: the page anchor follows the previously visible rows
// (`reanchor_page_start`), and moving the page to follow the selection would
// fight that contract and shift the list under the user. The frame is redrawn on
// the same update, so Enter always acts on the visibly highlighted row.
//
// One sanctioned exception: the one-shot initial-selection seed
// (`seed_initial_selection`) writes `page_start` to bring the seeded row on
// screen. That write happens before the user has seen a populated frame, so
// the no-repaging contract — which protects an established view — is not in
// play; do not "fix" the seed's page write to conform to it.
fn reconcile_selection(
    selected_pane_id: &mut Option<String>,
    visible_pane_ids: &[String],
) -> Option<usize> {
    let selected_row = selected_pane_id.as_deref().and_then(|selected| {
        visible_pane_ids
            .iter()
            .position(|pane_id| pane_id == selected)
    });
    if selected_row.is_some() {
        return selected_row;
    }

    *selected_pane_id = visible_pane_ids.first().cloned();
    selected_pane_id.as_ref().map(|_| 0)
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
    render_rows_for_width_with_icons(panes, key_targets, width, IconMode::Emoji)
}

pub(crate) fn render_rows_for_width_with_icons(
    panes: &[PaneRecord],
    key_targets: &BTreeMap<char, String>,
    width: usize,
    icon_mode: IconMode,
) -> Vec<String> {
    render_rows_for_width_with_location_labels_and_icons(
        &panes.iter().collect::<Vec<_>>(),
        key_targets,
        width,
        &HashMap::new(),
        icon_mode,
    )
}

fn render_rows_for_width_with_location_labels_and_icons(
    panes: &[&PaneRecord],
    key_targets: &BTreeMap<char, String>,
    width: usize,
    location_labels: &HashMap<String, String>,
    icon_mode: IconMode,
) -> Vec<String> {
    let key_labels: HashMap<&str, char> = key_targets
        .iter()
        .map(|(key, pane_id)| (pane_id.as_str(), *key))
        .collect();

    panes
        .iter()
        .map(|pane| {
            render_pane_row(
                pane,
                key_labels.get(pane.pane_id.as_str()).copied(),
                width,
                location_labels,
                icon_mode,
            )
        })
        .collect()
}

fn render_pane_row(
    pane: &PaneRecord,
    selection: Option<char>,
    width: usize,
    location_labels: &HashMap<String, String>,
    icon_mode: IconMode,
) -> String {
    let selection_label = selection
        .map(|assigned_key| format!("[{assigned_key}]"))
        .unwrap_or_else(|| "   ".to_string());
    let provider = provider_display_marker(pane.provider, icon_mode);
    let fallback_location;
    let location = if let Some(location) = location_labels.get(pane.pane_id.as_str()) {
        location.as_str()
    } else {
        fallback_location = pane.location.tag();
        fallback_location.as_str()
    };
    let prefix = format!(
        "{} {} {} {} - ",
        selection_label,
        status_emoji(pane.status.kind),
        provider,
        location
    );
    let sanitized_label = sanitize_tui_label(pane.display.label.as_str());
    format_row_with_trailing_label(&prefix, sanitized_label.as_str(), width)
}

fn footer_page_number(page_start: usize, page_size: usize, total_rows: usize) -> usize {
    let page_count = page_count(total_rows, page_size);
    if page_count == 0 {
        return 0;
    }

    let last_visible_index = page_start
        .saturating_add(page_size)
        .min(total_rows)
        .saturating_sub(1);
    (last_visible_index / page_size)
        .saturating_add(1)
        .min(page_count)
}

fn render_footer_lines(
    page_start: usize,
    page_size: usize,
    total_panes: usize,
    connection: &TuiConnectionState,
    width: usize,
) -> Vec<String> {
    let page_count = page_count(total_panes, page_size);
    let page_number = footer_page_number(page_start, page_size, total_panes);
    let shown_count = total_panes.saturating_sub(page_start).min(page_size.max(1));

    let first_line = footer_line_with_indicator(
        "Select with key or Up/Down + Enter. / search. Ctrl-B tmux prefix. Esc/Ctrl-C close.",
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

// The search footer keeps the two-line footprint of the normal footer: the
// first line is the query input line, the second documents the mode and shows
// how many panes match.
fn render_search_footer_lines(
    query: &str,
    page_start: usize,
    page_size: usize,
    matched_count: usize,
    total_panes: usize,
    connection: &TuiConnectionState,
    width: usize,
) -> Vec<String> {
    let page_count = page_count(matched_count, page_size);
    let first_line =
        footer_line_with_indicator(&format!("/{query}▌"), connection.indicator(), width);

    let second_line = if page_count > 1 {
        let page_number = footer_page_number(page_start, page_size, matched_count);
        footer_line_with_indicator(
            format!(
                "Page {page_number}/{page_count} | {matched_count}/{total_panes} matched | Enter focus. Esc cancel."
            )
            .as_str(),
            connection.message.as_str(),
            width,
        )
    } else {
        footer_line_with_indicator(
            format!("{matched_count}/{total_panes} matched | Enter focus. Esc cancel.").as_str(),
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

pub(super) fn sanitize_tui_label(label: &str) -> String {
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
        StatusKind::Waiting => "🟠",
        StatusKind::Unknown => "⚫",
    }
}
