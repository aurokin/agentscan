use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{IsTerminal, Stdout, Write, stdout};
use std::path::Path;
use std::time::{Duration, SystemTime};

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{self, Clear, ClearType};
use crossterm::{execute, queue};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::*;

const KEY_POLL_INTERVAL: Duration = Duration::from_millis(125);
const FOOTER_LINE_COUNT: usize = 2;
const MIN_SELECTABLE_POPUP_HEIGHT: usize = FOOTER_LINE_COUNT + 1;
const POPUP_READY_PATH_ENV: &str = "AGENTSCAN_POPUP_READY_PATH";
const POPUP_DONE_PATH_ENV: &str = "AGENTSCAN_POPUP_DONE_PATH";
const POPUP_SELECTION_KEYS: [char; 16] = [
    '1', '2', '3', '4', '5', 'Q', 'E', 'R', 'F', 'G', 'T', 'Z', 'X', 'C', 'V', 'B',
];

enum PopupLoopAction {
    Continue,
    Redraw,
    Close,
}

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

#[derive(Debug, Default)]
pub(crate) struct PopupState {
    key_targets: BTreeMap<char, String>,
    panes: Vec<PaneRecord>,
    error_message: Option<String>,
    page_start: usize,
    last_terminal_size: Option<PopupTerminalSize>,
}

impl PopupState {
    pub(crate) fn replace_panes(&mut self, panes: Vec<PaneRecord>) {
        let merged_panes = merge_popup_session_panes(&self.panes, panes);
        let page_start = reanchor_page_start(
            &self.panes,
            &merged_panes,
            self.page_start,
            self.page_size(),
        );
        self.panes = merged_panes;
        self.page_start = page_start;
        self.error_message = None;
    }

    pub(crate) fn set_error(&mut self, message: String) {
        self.key_targets.clear();
        self.panes.clear();
        self.page_start = 0;
        self.error_message = Some(message);
    }

    fn page_size(&self) -> usize {
        self.last_terminal_size
            .map(page_size_for_terminal)
            .unwrap_or_default()
    }

    fn max_page_start(&self) -> usize {
        last_non_empty_page_start(self.panes.len(), self.page_size())
    }

    pub(crate) fn next_page(&mut self) -> bool {
        let page_size = self.page_size();
        if page_size == 0 || self.panes.is_empty() {
            return false;
        }

        let next_page_start = self.page_start.saturating_add(page_size);
        if next_page_start >= self.panes.len() {
            return false;
        }

        self.page_start = next_page_start.min(self.max_page_start());
        self.key_targets.clear();
        true
    }

    pub(crate) fn previous_page(&mut self) -> bool {
        let page_size = self.page_size();
        if page_size == 0 || self.page_start == 0 {
            return false;
        }

        self.page_start = self.page_start.saturating_sub(page_size);
        self.key_targets.clear();
        true
    }
}

struct TerminalSession {
    stdout: Stdout,
}

impl TerminalSession {
    fn enter() -> Result<Self> {
        if !std::io::stdin().is_terminal() || !stdout().is_terminal() {
            bail!("`agentscan popup` requires an interactive tty");
        }

        terminal::enable_raw_mode().context("failed to enable terminal raw mode")?;
        let mut stdout = stdout();
        execute!(stdout, Hide).context("failed to hide cursor for popup session")?;
        Ok(Self { stdout })
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = execute!(self.stdout, Show);
        let _ = terminal::disable_raw_mode();
    }
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

fn handle_key_event(key_event: &KeyEvent, state: &mut PopupState) -> Result<PopupLoopAction> {
    if is_popup_close_key(key_event) {
        return Ok(PopupLoopAction::Close);
    }

    if key_event.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key_event.code, KeyCode::Char('b') | KeyCode::Char('B'))
    {
        tmux::switch_tmux_client_to_prefix(None)?;
        return Ok(PopupLoopAction::Continue);
    }

    if is_popup_previous_page_key(key_event) {
        return Ok(if state.previous_page() {
            PopupLoopAction::Redraw
        } else {
            PopupLoopAction::Continue
        });
    }

    if is_popup_next_page_key(key_event) {
        return Ok(if state.next_page() {
            PopupLoopAction::Redraw
        } else {
            PopupLoopAction::Continue
        });
    }

    let Some(selection) = popup_selection_from_key_event(key_event) else {
        return Ok(PopupLoopAction::Continue);
    };

    let Some(target_pane_id) = state.key_targets.get(&selection) else {
        return Ok(PopupLoopAction::Continue);
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

    Ok(PopupLoopAction::Close)
}

fn is_popup_close_key(key_event: &KeyEvent) -> bool {
    matches!(key_event.code, KeyCode::Esc)
        || (key_event.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key_event.code, KeyCode::Char('c') | KeyCode::Char('C')))
}

fn is_popup_previous_page_key(key_event: &KeyEvent) -> bool {
    matches!(key_event.code, KeyCode::Left | KeyCode::PageUp)
        || matches!(key_event.code, KeyCode::Char('p' | 'P'))
}

fn is_popup_next_page_key(key_event: &KeyEvent) -> bool {
    matches!(key_event.code, KeyCode::Right | KeyCode::PageDown)
        || matches!(key_event.code, KeyCode::Char('n' | 'N'))
}

fn popup_selection_from_key_event(key_event: &KeyEvent) -> Option<char> {
    match key_event.code {
        KeyCode::Char(character) => Some(character.to_ascii_uppercase()),
        _ => None,
    }
}

fn is_key_press(key_event: KeyEvent) -> bool {
    match key_event.kind {
        KeyEventKind::Press | KeyEventKind::Repeat => true,
        KeyEventKind::Release => false,
    }
}

fn draw_popup_frame(stdout: &mut Stdout, state: &mut PopupState) -> Result<()> {
    let frame = render_popup_frame_for_size(state, terminal_size()?);
    write_popup_frame(stdout, &frame)?;
    stdout.flush().context("failed to flush popup frame")
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

fn reanchor_page_start(
    previous_panes: &[PaneRecord],
    updated_panes: &[PaneRecord],
    previous_page_start: usize,
    page_size: usize,
) -> usize {
    if updated_panes.is_empty() || page_size == 0 {
        return 0;
    }

    let visible_start = previous_page_start.min(previous_panes.len());
    let visible_end = visible_start
        .saturating_add(page_size)
        .min(previous_panes.len());
    let updated_index_by_id: HashMap<&str, usize> = updated_panes
        .iter()
        .enumerate()
        .map(|(index, pane)| (pane.pane_id.as_str(), index))
        .collect();

    for pane in &previous_panes[visible_start..visible_end] {
        if let Some(index) = updated_index_by_id.get(pane.pane_id.as_str()).copied() {
            return index;
        }
    }

    previous_page_start.min(last_non_empty_page_start(updated_panes.len(), page_size))
}

fn terminal_size() -> Result<PopupTerminalSize> {
    let (width, height) = terminal::size().context("failed to read popup terminal size")?;
    Ok(PopupTerminalSize { width, height })
}

pub(crate) fn render_popup_frame_for_size(
    state: &mut PopupState,
    terminal_size: PopupTerminalSize,
) -> PopupFrame {
    state.last_terminal_size = Some(terminal_size);

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

pub(crate) fn merge_popup_session_panes(
    current_order: &[PaneRecord],
    updated_panes: Vec<PaneRecord>,
) -> Vec<PaneRecord> {
    let mut updated_by_id: HashMap<String, PaneRecord> = updated_panes
        .iter()
        .cloned()
        .map(|pane| (pane.pane_id.clone(), pane))
        .collect();
    let mut ordered_panes = Vec::with_capacity(updated_by_id.len());

    for pane in current_order {
        if let Some(updated_pane) = updated_by_id.remove(pane.pane_id.as_str()) {
            ordered_panes.push(updated_pane);
        }
    }

    for pane in updated_panes {
        if let Some(updated_pane) = updated_by_id.remove(pane.pane_id.as_str()) {
            ordered_panes.push(updated_pane);
        }
    }

    ordered_panes
}

pub(crate) fn synchronize_key_targets(
    key_targets: &mut BTreeMap<char, String>,
    panes: &[PaneRecord],
) {
    let present_pane_ids: HashSet<&str> = panes.iter().map(|pane| pane.pane_id.as_str()).collect();
    key_targets.retain(|_, pane_id| present_pane_ids.contains(pane_id.as_str()));

    let mut assigned_pane_ids: HashSet<String> = key_targets.values().cloned().collect();
    let free_keys: Vec<char> = POPUP_SELECTION_KEYS
        .iter()
        .copied()
        .filter(|key| !key_targets.contains_key(key))
        .collect();

    let mut next_free_key = free_keys.into_iter();
    for pane in panes {
        if assigned_pane_ids.contains(pane.pane_id.as_str()) {
            continue;
        }
        let Some(free_key) = next_free_key.next() else {
            break;
        };
        key_targets.insert(free_key, pane.pane_id.clone());
        assigned_pane_ids.insert(pane.pane_id.clone());
    }
}

fn fit_lines_to_terminal(lines: &[String], terminal_size: PopupTerminalSize) -> Vec<String> {
    let width = usize::from(terminal_size.width);
    lines
        .iter()
        .take(usize::from(terminal_size.height))
        .map(|line| truncate_to_width(line, width))
        .collect()
}

fn page_size_for_terminal(terminal_size: PopupTerminalSize) -> usize {
    let available_height = usize::from(terminal_size.height);
    if available_height < MIN_SELECTABLE_POPUP_HEIGHT {
        return 0;
    }

    available_height
        .saturating_sub(FOOTER_LINE_COUNT)
        .min(POPUP_SELECTION_KEYS.len())
}

fn page_count(total_panes: usize, page_size: usize) -> usize {
    if total_panes == 0 || page_size == 0 {
        return 0;
    }

    total_panes.div_ceil(page_size)
}

fn last_non_empty_page_start(total_panes: usize, page_size: usize) -> usize {
    if total_panes == 0 || page_size == 0 {
        return 0;
    }

    ((total_panes - 1) / page_size) * page_size
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

fn format_popup_error(error: &anyhow::Error) -> String {
    error.to_string()
}

fn cache_mtime(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}
