use std::collections::{BTreeMap, HashMap, HashSet};

use super::*;

const FOOTER_LINE_COUNT: usize = 2;
const MIN_SELECTABLE_TUI_HEIGHT: usize = FOOTER_LINE_COUNT + 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TuiConnectionKind {
    Connecting,
    Connected,
    Offline,
    Shutdown,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TuiConnectionState {
    pub(crate) kind: TuiConnectionKind,
    pub(crate) message: String,
    pub(crate) retrying: bool,
}

impl TuiConnectionState {
    fn connecting(message: impl Into<String>) -> Self {
        Self {
            kind: TuiConnectionKind::Connecting,
            message: message.into(),
            retrying: true,
        }
    }

    fn connected() -> Self {
        Self {
            kind: TuiConnectionKind::Connected,
            message: "live daemon subscription".to_string(),
            retrying: false,
        }
    }

    fn offline(message: impl Into<String>, retrying: bool) -> Self {
        Self {
            kind: TuiConnectionKind::Offline,
            message: message.into(),
            retrying,
        }
    }

    fn shutdown(message: impl Into<String>) -> Self {
        Self {
            kind: TuiConnectionKind::Shutdown,
            message: message.into(),
            retrying: false,
        }
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            kind: TuiConnectionKind::Unavailable,
            message: message.into(),
            retrying: false,
        }
    }

    pub(crate) fn indicator(&self) -> &'static str {
        match self.kind {
            TuiConnectionKind::Connecting => "[connecting]",
            TuiConnectionKind::Connected => "[live]",
            TuiConnectionKind::Offline if self.retrying => "[reconnecting]",
            TuiConnectionKind::Offline => "[offline]",
            TuiConnectionKind::Shutdown => "[shutdown]",
            TuiConnectionKind::Unavailable => "[unavailable]",
        }
    }
}

impl Default for TuiConnectionState {
    fn default() -> Self {
        Self::connecting("connecting to daemon")
    }
}

#[derive(Debug, Default)]
pub(crate) struct TuiState {
    pub(super) picker_keys: picker::PickerKeySet,
    pub(super) picker_group_by: picker::PickerGroupBy,
    pub(super) key_targets: BTreeMap<char, String>,
    pub(super) retired_key_targets: BTreeMap<char, String>,
    pub(super) panes: Vec<PaneRecord>,
    pub(super) pane_location_labels: HashMap<String, String>,
    pub(super) workspace_cache: picker::PickerWorkspaceCache,
    pub(super) error_message: Option<String>,
    pub(super) connection: TuiConnectionState,
    pub(super) page_start: usize,
    pub(super) reset_key_targets_on_next_render: bool,
    pub(super) selected_pane_id: Option<String>,
    last_terminal_size: Option<TuiTerminalSize>,
}

impl TuiState {
    #[cfg(test)]
    pub(crate) fn with_picker_keys(picker_keys: picker::PickerKeySet) -> Self {
        Self {
            picker_keys,
            ..Self::default()
        }
    }

    pub(crate) fn with_picker_config(
        picker_keys: picker::PickerKeySet,
        picker_group_by: picker::PickerGroupBy,
    ) -> Self {
        Self {
            picker_keys,
            picker_group_by,
            ..Self::default()
        }
    }

    pub(crate) fn set_connecting(&mut self, message: String) {
        self.connection = if self.panes.is_empty() {
            TuiConnectionState::connecting(message)
        } else {
            TuiConnectionState::offline(message, true)
        };
    }

    pub(crate) fn replace_panes(&mut self, panes: Vec<PaneRecord>) {
        let mut merged_panes = merge_tui_session_panes(&self.panes, panes);
        picker::sort_panes_for_picker_with_cache(
            &mut merged_panes,
            self.picker_group_by,
            &mut self.workspace_cache,
        );
        let order_changed = pane_id_order(&self.panes) != pane_id_order(&merged_panes);
        let same_panes_reordered = order_changed && same_pane_id_set(&self.panes, &merged_panes);
        let page_size = self.page_size();
        let page_start = if same_panes_reordered {
            reanchor_page_start_to_page_boundary(
                &self.panes,
                &merged_panes,
                self.page_start,
                page_size,
            )
        } else {
            reanchor_page_start(&self.panes, &merged_panes, self.page_start, page_size)
        };
        let visible_key_slots_changed = visible_key_slots_changed(
            &self.panes,
            &merged_panes,
            self.page_start,
            page_start,
            page_size,
        );
        if visible_key_slots_changed
            && (same_panes_reordered || self.picker_group_by != picker::PickerGroupBy::Session)
        {
            self.reset_key_targets_on_next_render = true;
        }
        let pane_location_labels = pane_location_labels(
            &merged_panes,
            self.picker_group_by,
            &mut self.workspace_cache,
        );
        self.panes = merged_panes;
        self.pane_location_labels = pane_location_labels;
        self.page_start = page_start;
        self.error_message = None;
        self.connection = TuiConnectionState::connected();
    }

    pub(crate) fn set_unavailable(&mut self, message: String) {
        self.key_targets.clear();
        self.retired_key_targets.clear();
        self.panes.clear();
        self.pane_location_labels.clear();
        self.workspace_cache.clear();
        self.page_start = 0;
        self.selected_pane_id = None;
        self.error_message = Some(message.clone());
        self.connection = TuiConnectionState::unavailable(message);
    }

    pub(crate) fn set_offline(&mut self, message: String, retrying: bool) {
        self.connection = TuiConnectionState::offline(message, retrying);
    }

    pub(crate) fn set_shutdown(&mut self, message: String) {
        self.connection = TuiConnectionState::shutdown(message);
    }

    pub(super) fn set_terminal_size(&mut self, terminal_size: TuiTerminalSize) {
        self.last_terminal_size = Some(terminal_size);
    }

    #[cfg(test)]
    pub(crate) fn test_key_target(&self, key: char) -> Option<&str> {
        self.key_targets.get(&key).map(String::as_str)
    }

    #[cfg(test)]
    pub(crate) fn test_retired_key_target(&self, key: char) -> Option<&str> {
        self.retired_key_targets.get(&key).map(String::as_str)
    }

    #[cfg(test)]
    pub(crate) fn test_selected_pane_id(&self) -> Option<&str> {
        self.selected_pane_id.as_deref()
    }

    fn page_size(&self) -> usize {
        self.last_terminal_size
            .map(|terminal_size| page_size_for_terminal(terminal_size, &self.picker_keys))
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
        self.retired_key_targets.clear();
        true
    }

    pub(crate) fn previous_page(&mut self) -> bool {
        let page_size = self.page_size();
        if page_size == 0 || self.page_start == 0 {
            return false;
        }

        self.page_start = self.page_start.saturating_sub(page_size);
        self.key_targets.clear();
        self.retired_key_targets.clear();
        true
    }

    pub(crate) fn select_next(&mut self) -> bool {
        let Some(visible) = self.visible_pane_range() else {
            return false;
        };

        match self.selected_visible_index(&visible) {
            None => self.select_pane_at(visible.start),
            Some(index) if visible.start + index + 1 < visible.end => {
                self.select_pane_at(visible.start + index + 1)
            }
            Some(_) => self.next_page() && self.select_pane_at(self.page_start),
        }
    }

    pub(crate) fn select_previous(&mut self) -> bool {
        let Some(visible) = self.visible_pane_range() else {
            return false;
        };

        match self.selected_visible_index(&visible) {
            None => self.select_pane_at(visible.start),
            Some(0) => {
                if visible.start == 0 {
                    return false;
                }
                // Reveal exactly the row above the current window and keep the
                // highlight on it. A live reanchor can leave `page_start`
                // non-aligned, so jumping back a whole page and selecting that
                // window's last row could land on a row that was already on
                // screen, visually moving the highlight forward.
                let target = visible.start - 1;
                self.page_start = target.saturating_add(1).saturating_sub(self.page_size());
                self.key_targets.clear();
                self.retired_key_targets.clear();
                self.select_pane_at(target)
            }
            Some(index) => self.select_pane_at(visible.start + index - 1),
        }
    }

    fn visible_pane_range(&self) -> Option<std::ops::Range<usize>> {
        let page_size = self.page_size();
        if page_size == 0 || self.panes.is_empty() {
            return None;
        }

        // Mirror the render path's clamp exactly: `page_start` is reclamped only
        // when it points past the end. A live reanchor can leave it at a
        // non-page-aligned index inside the final partial page, and clamping it
        // differently here would make arrow movement act on rows the frame does
        // not show.
        let start = if self.page_start >= self.panes.len() {
            last_non_empty_page_start(self.panes.len(), page_size)
        } else {
            self.page_start
        };
        let end = start.saturating_add(page_size).min(self.panes.len());
        Some(start..end)
    }

    fn selected_visible_index(&self, visible: &std::ops::Range<usize>) -> Option<usize> {
        let selected_pane_id = self.selected_pane_id.as_deref()?;
        self.panes[visible.clone()]
            .iter()
            .position(|pane| pane.pane_id == selected_pane_id)
    }

    fn select_pane_at(&mut self, pane_index: usize) -> bool {
        let Some(pane) = self.panes.get(pane_index) else {
            return false;
        };

        self.selected_pane_id = Some(pane.pane_id.clone());
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

fn reanchor_page_start_to_page_boundary(
    previous_panes: &[PaneRecord],
    updated_panes: &[PaneRecord],
    previous_page_start: usize,
    page_size: usize,
) -> usize {
    let anchored_start = reanchor_page_start(
        previous_panes,
        updated_panes,
        previous_page_start,
        page_size,
    );
    if page_size == 0 {
        return 0;
    }

    ((anchored_start / page_size) * page_size)
        .min(last_non_empty_page_start(updated_panes.len(), page_size))
}

pub(crate) fn merge_tui_session_panes(
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

fn pane_id_order(panes: &[PaneRecord]) -> Vec<&str> {
    panes.iter().map(|pane| pane.pane_id.as_str()).collect()
}

fn same_pane_id_set(left: &[PaneRecord], right: &[PaneRecord]) -> bool {
    if left.len() != right.len() {
        return false;
    }

    let left_ids = left
        .iter()
        .map(|pane| pane.pane_id.as_str())
        .collect::<HashSet<_>>();
    right
        .iter()
        .all(|pane| left_ids.contains(pane.pane_id.as_str()))
}

fn visible_key_slots_changed(
    previous_panes: &[PaneRecord],
    updated_panes: &[PaneRecord],
    previous_page_start: usize,
    updated_page_start: usize,
    page_size: usize,
) -> bool {
    if previous_panes.is_empty() || updated_panes.is_empty() || page_size == 0 {
        return false;
    }

    let previous_visible_end = previous_page_start
        .saturating_add(page_size)
        .min(previous_panes.len());
    let previous_slots = previous_panes
        [previous_page_start.min(previous_panes.len())..previous_visible_end]
        .iter()
        .enumerate()
        .map(|(slot, pane)| (pane.pane_id.as_str(), slot))
        .collect::<HashMap<_, _>>();
    let updated_visible_end = updated_page_start
        .saturating_add(page_size)
        .min(updated_panes.len());

    updated_panes[updated_page_start.min(updated_panes.len())..updated_visible_end]
        .iter()
        .enumerate()
        .any(|(slot, pane)| {
            previous_slots
                .get(pane.pane_id.as_str())
                .is_some_and(|previous_slot| *previous_slot != slot)
        })
}

fn pane_location_labels(
    panes: &[PaneRecord],
    picker_group_by: picker::PickerGroupBy,
    workspace_cache: &mut picker::PickerWorkspaceCache,
) -> HashMap<String, String> {
    panes
        .iter()
        .map(|pane| {
            (
                pane.pane_id.clone(),
                pane_picker_location_label(pane, picker_group_by, workspace_cache),
            )
        })
        .collect()
}

fn pane_picker_location_label(
    pane: &PaneRecord,
    picker_group_by: picker::PickerGroupBy,
    workspace_cache: &mut picker::PickerWorkspaceCache,
) -> String {
    let location_tag = pane.location.tag();
    if picker_group_by == picker::PickerGroupBy::Session {
        return location_tag;
    }

    let workspace = workspace_cache.workspace_for_pane(pane, picker_group_by);
    if workspace.source == picker::PickerWorkspaceSource::Session
        || workspace.label == pane.location.session_name
    {
        location_tag
    } else {
        format!("{} {}", workspace.label, location_tag)
    }
}

pub(crate) fn synchronize_key_targets(
    key_targets: &mut BTreeMap<char, String>,
    panes: &[PaneRecord],
) {
    synchronize_key_targets_with_keys(key_targets, panes, &picker::PickerKeySet::default());
}

pub(crate) fn synchronize_key_targets_with_keys(
    key_targets: &mut BTreeMap<char, String>,
    panes: &[PaneRecord],
    picker_keys: &picker::PickerKeySet,
) {
    let present_pane_ids: HashSet<&str> = panes.iter().map(|pane| pane.pane_id.as_str()).collect();
    key_targets.retain(|_, pane_id| present_pane_ids.contains(pane_id.as_str()));

    let mut assigned_pane_ids: HashSet<String> = key_targets.values().cloned().collect();
    let free_keys: Vec<char> = picker_keys
        .keys()
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

pub(super) fn page_size_for_terminal(
    terminal_size: TuiTerminalSize,
    picker_keys: &picker::PickerKeySet,
) -> usize {
    let available_height = usize::from(terminal_size.height);
    if available_height < MIN_SELECTABLE_TUI_HEIGHT {
        return 0;
    }

    available_height
        .saturating_sub(FOOTER_LINE_COUNT)
        .min(picker_keys.len())
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
