use std::collections::{BTreeMap, HashMap, HashSet};

use super::*;

pub(super) const POPUP_SELECTION_KEYS: [char; 16] = [
    '1', '2', '3', '4', '5', 'Q', 'E', 'R', 'F', 'G', 'T', 'Z', 'X', 'C', 'V', 'B',
];

const FOOTER_LINE_COUNT: usize = 2;
const MIN_SELECTABLE_POPUP_HEIGHT: usize = FOOTER_LINE_COUNT + 1;

#[derive(Debug, Default)]
pub(crate) struct PopupState {
    pub(super) key_targets: BTreeMap<char, String>,
    pub(super) panes: Vec<PaneRecord>,
    pub(super) error_message: Option<String>,
    pub(super) page_start: usize,
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

    pub(super) fn set_terminal_size(&mut self, terminal_size: PopupTerminalSize) {
        self.last_terminal_size = Some(terminal_size);
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

pub(super) fn page_size_for_terminal(terminal_size: PopupTerminalSize) -> usize {
    let available_height = usize::from(terminal_size.height);
    if available_height < MIN_SELECTABLE_POPUP_HEIGHT {
        return 0;
    }

    available_height
        .saturating_sub(FOOTER_LINE_COUNT)
        .min(POPUP_SELECTION_KEYS.len())
}

pub(super) fn page_count(total_panes: usize, page_size: usize) -> usize {
    if total_panes == 0 || page_size == 0 {
        return 0;
    }

    total_panes.div_ceil(page_size)
}

pub(super) fn last_non_empty_page_start(total_panes: usize, page_size: usize) -> usize {
    if total_panes == 0 || page_size == 0 {
        return 0;
    }

    ((total_panes - 1) / page_size) * page_size
}
