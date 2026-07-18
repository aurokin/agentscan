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
    // `Some` while the picker is in search mode; the string is the typed query
    // (possibly empty). While searching, `page_start` addresses positions in
    // the filtered view rather than `panes` indexes.
    pub(super) search_query: Option<String>,
    // Caller-pane hint captured at startup (the pane the popup was invoked
    // over). One-shot: consulted only by the initial-selection seed on the
    // first populated frame, then dropped. No transport — it never leaves
    // this process.
    pub(super) initial_selection_hint: Option<String>,
    // One-shot latch for the initial-selection seed (caller hint, then focus
    // recency). Set on the first populated frame that reaches the seed site
    // regardless of outcome, and by any selection-moving input before it, so
    // daemon blips that clear `selected_pane_id` can never re-seed and yank
    // an established view.
    pub(super) initial_selection_seeded: bool,
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
        // While searching, `page_start` addresses the filtered view and the
        // reanchor helpers below operate on full-list positions. Capture the
        // currently visible filtered rows so the anchor can follow them across
        // the update (mirroring `reanchor_page_start`) once the merged panes
        // are in place; key targets are suspended in search mode, so the
        // key-slot bookkeeping below does not apply either.
        let search_visible_ids = self
            .search_query
            .is_some()
            .then(|| self.visible_search_pane_ids());
        let page_start = if search_visible_ids.is_some() {
            self.page_start
        } else if same_panes_reordered {
            reanchor_page_start_to_page_boundary(
                &self.panes,
                &merged_panes,
                self.page_start,
                page_size,
            )
        } else {
            reanchor_page_start(&self.panes, &merged_panes, self.page_start, page_size)
        };
        if self.search_query.is_none() {
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
        }
        let pane_location_labels = pane_location_labels(
            &merged_panes,
            self.picker_group_by,
            &mut self.workspace_cache,
        );
        self.panes = merged_panes;
        self.pane_location_labels = pane_location_labels;
        self.page_start = match &search_visible_ids {
            Some(previous_visible_ids) => self.reanchor_search_page_start(previous_visible_ids),
            None => page_start,
        };
        self.error_message = None;
        self.connection = TuiConnectionState::connected();
    }

    // The filtered-view pane ids currently on screen, in row order.
    fn visible_search_pane_ids(&self) -> Vec<String> {
        let view = self.view_pane_indices();
        let Some(visible) = self.visible_view_range(view.len()) else {
            return Vec::new();
        };
        view[visible]
            .iter()
            .map(|&pane_index| self.panes[pane_index].pane_id.clone())
            .collect()
    }

    // Filtered-view counterpart of `reanchor_page_start`: keep the first
    // surviving previously visible filtered row at the top of the window, so
    // live updates that insert or remove matches ahead of the window do not
    // shift the rows on screen (and with them the pane-anchored selection).
    //
    // Deliberately anchors on visible rows, not the selection: if an update
    // inserts matches between the window's first row and a selection further
    // down, the selection can leave the window and snap to the first visible
    // row. That is the same contract normal mode follows (see
    // `reconcile_selection`) — the window tracks what the user was looking
    // at, and re-paging to chase the selection would shift the list under
    // them.
    fn reanchor_search_page_start(&self, previous_visible_ids: &[String]) -> usize {
        let view = self.view_pane_indices();
        let page_size = self.page_size();
        if view.is_empty() || page_size == 0 {
            return 0;
        }

        for previous_id in previous_visible_ids {
            if let Some(position) = view
                .iter()
                .position(|&pane_index| self.panes[pane_index].pane_id == *previous_id)
            {
                return position;
            }
        }

        self.page_start
            .min(last_non_empty_page_start(view.len(), page_size))
    }

    pub(crate) fn set_unavailable(&mut self, message: String) {
        self.key_targets.clear();
        self.retired_key_targets.clear();
        self.panes.clear();
        self.pane_location_labels.clear();
        self.workspace_cache.clear();
        self.page_start = 0;
        self.selected_pane_id = None;
        self.search_query = None;
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

    pub(crate) fn is_searching(&self) -> bool {
        self.search_query.is_some()
    }

    pub(crate) fn begin_search(&mut self) -> bool {
        self.cancel_initial_selection_seed();
        if self.search_query.is_some() || self.panes.is_empty() || self.error_message.is_some() {
            return false;
        }

        self.search_query = Some(String::new());
        // The empty query lists every pane, so keep the current page: resetting
        // it here would push an off-first-page selection out of view and the
        // redraw would snap the selection away — making `/` then Esc lossy.
        // Query edits reset the anchor because they change the view.
        //
        // Letter hotkeys are suspended while searching; keys now type into the
        // query instead of selecting rows.
        self.key_targets.clear();
        self.retired_key_targets.clear();
        true
    }

    pub(crate) fn cancel_search(&mut self) -> bool {
        if self.search_query.take().is_none() {
            return false;
        }

        self.key_targets.clear();
        self.retired_key_targets.clear();
        // Bring the selected pane back into view in the full list; searching
        // may have moved the selection far from the pre-search page.
        let page_size = self.page_size();
        let selected_index = self
            .selected_pane_id
            .as_deref()
            .and_then(|selected| self.panes.iter().position(|pane| pane.pane_id == selected));
        self.page_start = match selected_index {
            Some(pane_index) if page_size > 0 => (pane_index / page_size) * page_size,
            _ => 0,
        };
        true
    }

    pub(crate) fn push_search_char(&mut self, character: char) -> bool {
        let Some(query) = self.search_query.as_mut() else {
            return false;
        };
        if character.is_control() {
            return false;
        }

        query.push(character);
        self.page_start = 0;
        true
    }

    pub(crate) fn pop_search_char(&mut self) -> bool {
        let Some(query) = self.search_query.as_mut() else {
            return false;
        };
        if query.pop().is_none() {
            return false;
        }

        self.page_start = 0;
        true
    }

    // Indexes into `panes` for the rows the picker currently lists: all panes
    // normally, only query matches while searching. Paging and selection
    // operate on positions in this view.
    pub(super) fn view_pane_indices(&self) -> Vec<usize> {
        let Some(query) = self
            .search_query
            .as_deref()
            .filter(|query| !query.is_empty())
        else {
            return (0..self.panes.len()).collect();
        };

        self.panes
            .iter()
            .enumerate()
            .filter(|(_, pane)| self.pane_matches_search(pane, query))
            .map(|(pane_index, _)| pane_index)
            .collect()
    }

    // Match the text the row actually displays: the sanitized label and the
    // location label used for this grouping mode.
    fn pane_matches_search(&self, pane: &PaneRecord, query: &str) -> bool {
        if fuzzy_matches(
            &super::render::sanitize_tui_label(&pane.display.label),
            query,
        ) {
            return true;
        }
        match self.pane_location_labels.get(pane.pane_id.as_str()) {
            Some(location_label) => fuzzy_matches(location_label, query),
            None => fuzzy_matches(&pane.location.tag(), query),
        }
    }

    fn page_size(&self) -> usize {
        self.last_terminal_size
            .map(|terminal_size| page_size_for_terminal(terminal_size, &self.picker_keys))
            .unwrap_or_default()
    }

    /// Cancel the one-shot initial-selection seed: the user acted, so the
    /// highlight must never be yanked to a seed afterwards.
    pub(super) fn cancel_initial_selection_seed(&mut self) {
        self.initial_selection_seeded = true;
        self.initial_selection_hint = None;
    }

    /// One-shot initial-selection seed, applied on the first populated frame:
    /// caller-pane hint first, then focus recency (`last_focus_seq` argmax
    /// over the view), else fall through to the default first-visible-row
    /// reconciliation. Called from the frame builder after the page clamp,
    /// where a non-empty view and non-zero page size are structural
    /// guarantees — connecting/empty/undersized frames exit earlier and can
    /// never consume the seed.
    pub(super) fn seed_initial_selection(&mut self, view: &[usize], page_size: usize) {
        if self.initial_selection_seeded {
            return;
        }
        self.initial_selection_seeded = true;
        let hint = self.initial_selection_hint.take();
        if self.selected_pane_id.is_some() || self.search_query.is_some() {
            return;
        }
        let hint_position = hint.as_deref().and_then(|hint_id| {
            view.iter()
                .position(|&pane_index| self.panes[pane_index].pane_id == hint_id)
        });
        let seed_position =
            hint_position.or_else(|| self.most_recently_focused_view_position(view));
        if let Some(position) = seed_position {
            self.selected_pane_id = Some(self.panes[view[position]].pane_id.clone());
            // Page-align so the seeded row is on screen: selection
            // reconciliation only sees the visible page and would otherwise
            // snap a beyond-page-one seed back to the first row. This
            // pre-view `page_start` write is sanctioned by the render anchor
            // contract — the user has not seen a populated frame yet.
            self.page_start = (position / page_size) * page_size;
        }
    }

    /// View position of the pane most recently focused through an agentscan
    /// focus action. Ordinal comparison only; ties are impossible (the daemon
    /// issues strictly increasing seqs).
    fn most_recently_focused_view_position(&self, view: &[usize]) -> Option<usize> {
        view.iter()
            .enumerate()
            .filter_map(|(position, &pane_index)| {
                self.panes[pane_index]
                    .last_focus_seq
                    .map(|seq| (seq, position))
            })
            .max_by_key(|&(seq, _)| seq)
            .map(|(_, position)| position)
    }

    pub(crate) fn next_page(&mut self) -> bool {
        self.cancel_initial_selection_seed();
        let view_len = self.view_pane_indices().len();
        let page_size = self.page_size();
        if page_size == 0 || view_len == 0 {
            return false;
        }

        let next_page_start = self.page_start.saturating_add(page_size);
        if next_page_start >= view_len {
            return false;
        }

        self.page_start = next_page_start.min(last_non_empty_page_start(view_len, page_size));
        self.key_targets.clear();
        self.retired_key_targets.clear();
        true
    }

    pub(crate) fn previous_page(&mut self) -> bool {
        self.cancel_initial_selection_seed();
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
        self.cancel_initial_selection_seed();
        let view = self.view_pane_indices();
        let Some(visible) = self.visible_view_range(view.len()) else {
            return false;
        };

        match self.selected_view_position(&view, &visible) {
            None => self.select_view_pane(&view, visible.start),
            Some(position) if visible.start + position + 1 < visible.end => {
                self.select_view_pane(&view, visible.start + position + 1)
            }
            // Select the row below the current window, not the new page's first
            // row: next_page() clamps to the page-aligned boundary, so after a
            // live reanchor leaves `page_start` non-aligned near the tail the
            // new window can still contain the already-selected row.
            Some(_) => self.next_page() && self.select_view_pane(&view, visible.end),
        }
    }

    pub(crate) fn select_previous(&mut self) -> bool {
        self.cancel_initial_selection_seed();
        let view = self.view_pane_indices();
        let Some(visible) = self.visible_view_range(view.len()) else {
            return false;
        };

        match self.selected_view_position(&view, &visible) {
            None => self.select_view_pane(&view, visible.start),
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
                self.select_view_pane(&view, target)
            }
            Some(position) => self.select_view_pane(&view, visible.start + position - 1),
        }
    }

    fn visible_view_range(&self, view_len: usize) -> Option<std::ops::Range<usize>> {
        let page_size = self.page_size();
        if page_size == 0 || view_len == 0 {
            return None;
        }

        // Mirror the render path's clamp exactly: `page_start` is reclamped only
        // when it points past the end. A live reanchor can leave it at a
        // non-page-aligned index inside the final partial page, and clamping it
        // differently here would make arrow movement act on rows the frame does
        // not show.
        let start = if self.page_start >= view_len {
            last_non_empty_page_start(view_len, page_size)
        } else {
            self.page_start
        };
        let end = start.saturating_add(page_size).min(view_len);
        Some(start..end)
    }

    fn selected_view_position(
        &self,
        view: &[usize],
        visible: &std::ops::Range<usize>,
    ) -> Option<usize> {
        let selected_pane_id = self.selected_pane_id.as_deref()?;
        view[visible.clone()]
            .iter()
            .position(|&pane_index| self.panes[pane_index].pane_id == selected_pane_id)
    }

    fn select_view_pane(&mut self, view: &[usize], view_position: usize) -> bool {
        let Some(pane) = view
            .get(view_position)
            .and_then(|&pane_index| self.panes.get(pane_index))
        else {
            return false;
        };

        self.selected_pane_id = Some(pane.pane_id.clone());
        true
    }
}

// Case-insensitive subsequence match: every query character must appear in the
// haystack in order, though not necessarily adjacent.
fn fuzzy_matches(haystack: &str, query: &str) -> bool {
    let mut haystack_chars = haystack.chars().flat_map(char::to_lowercase);
    query
        .chars()
        .flat_map(char::to_lowercase)
        .all(|query_char| haystack_chars.any(|haystack_char| haystack_char == query_char))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{tmux_pane_row, tui_search_pane, tui_test_pane};

    fn tui_key_event(code: crossterm::event::KeyCode) -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }

    fn type_tui_search_query(state: &mut TuiState, query: &str) {
        for character in query.chars() {
            let action = crate::app::tui::handle_key_event(
                &tui_key_event(crossterm::event::KeyCode::Char(character)),
                state,
            )
            .expect("typing into search should not error");
            assert!(matches!(action, crate::app::tui::TuiLoopAction::Redraw));
        }
    }

    fn set_initial_selection_hint(state: &mut TuiState, hint: &str) {
        state.initial_selection_hint = Some(hint.to_string());
    }

    #[test]
    fn tui_key_assignments_reset_after_workspace_reorder() {
        let pane_one = tmux_pane_row(1)
            .session_name("work")
            .pane_id("%1")
            .command("codex")
            .title("Task 1")
            .current_path("/work/beta")
            .pane();
        let pane_two = tmux_pane_row(2)
            .session_name("work")
            .pane_id("%2")
            .command("codex")
            .title("Task 2")
            .current_path("/work/gamma")
            .pane();
        let mut state = crate::app::tui::TuiState::with_picker_config(
            crate::app::picker::PickerKeySet::default(),
            crate::app::picker::PickerGroupBy::Cwd,
        );
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 120,
            height: 10,
        };
        state.replace_panes(vec![pane_one.clone(), pane_two]);
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(state.key_targets.get(&'1').map(String::as_str), Some("%1"));
        assert_eq!(state.key_targets.get(&'2').map(String::as_str), Some("%2"));

        let moved_pane_two = tmux_pane_row(2)
            .session_name("work")
            .pane_id("%2")
            .command("codex")
            .title("Task 2")
            .current_path("/work/alpha")
            .pane();
        state.replace_panes(vec![pane_one, moved_pane_two]);
        let frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);

        assert_eq!(frame.visible_pane_ids, vec!["%2", "%1"]);
        assert_eq!(state.key_targets.get(&'1').map(String::as_str), Some("%2"));
        assert_eq!(state.key_targets.get(&'2').map(String::as_str), Some("%1"));
        assert_eq!(
            state.retired_key_targets.get(&'1').map(String::as_str),
            None
        );
    }

    #[test]
    fn tui_key_assignments_reset_when_workspace_insertion_shifts_visible_rows() {
        let pane_one = tmux_pane_row(1)
            .session_name("work")
            .pane_id("%1")
            .command("codex")
            .title("Task 1")
            .current_path("/work/alpha")
            .pane();
        let pane_two = tmux_pane_row(2)
            .session_name("work")
            .pane_id("%2")
            .command("codex")
            .title("Task 2")
            .current_path("/work/gamma")
            .pane();
        let inserted_pane = tmux_pane_row(3)
            .session_name("work")
            .pane_id("%3")
            .command("codex")
            .title("Task 3")
            .current_path("/work/beta")
            .pane();
        let mut state = crate::app::tui::TuiState::with_picker_config(
            crate::app::picker::PickerKeySet::default(),
            crate::app::picker::PickerGroupBy::Cwd,
        );
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 120,
            height: 10,
        };
        state.replace_panes(vec![pane_one.clone(), pane_two.clone()]);
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(state.key_targets.get(&'1').map(String::as_str), Some("%1"));
        assert_eq!(state.key_targets.get(&'2').map(String::as_str), Some("%2"));

        state.replace_panes(vec![pane_one, pane_two, inserted_pane]);
        let frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);

        assert_eq!(frame.visible_pane_ids, vec!["%1", "%3", "%2"]);
        assert_eq!(state.key_targets.get(&'1').map(String::as_str), Some("%1"));
        assert_eq!(state.key_targets.get(&'2').map(String::as_str), Some("%3"));
        assert_eq!(state.key_targets.get(&'3').map(String::as_str), Some("%2"));
    }

    #[test]
    fn tui_workspace_reorder_reanchors_to_previous_visible_pane_page() {
        let pane = |index: u32, cwd: String| {
            tmux_pane_row(index)
                .session_name("work")
                .pane_id(format!("%{index}"))
                .command("codex")
                .title(format!("Task {index}"))
                .current_path(cwd)
                .pane()
        };
        let panes = (1..=8)
            .map(|index| pane(index, format!("/work/p{index:02}")))
            .collect::<Vec<_>>();
        let mut state = crate::app::tui::TuiState::with_picker_config(
            crate::app::picker::PickerKeySet::default(),
            crate::app::picker::PickerGroupBy::Cwd,
        );
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 120,
            height: 6,
        };
        state.replace_panes(panes);
        let first_frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(first_frame.page_size, 4);
        assert!(state.next_page());
        let second_frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(second_frame.visible_pane_ids, vec!["%5", "%6", "%7", "%8"]);

        let reordered = (1..=8)
            .map(|index| {
                let cwd = if index == 5 {
                    "/work/p00".to_string()
                } else {
                    format!("/work/p{index:02}")
                };
                pane(index, cwd)
            })
            .collect::<Vec<_>>();
        state.replace_panes(reordered);
        let anchored_frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);

        assert_eq!(anchored_frame.page_start, 0);
        assert_eq!(anchored_frame.visible_pane_ids[0], "%5");
        assert_eq!(state.key_targets.get(&'1').map(String::as_str), Some("%5"));
    }

    #[test]
    fn tui_retains_retired_key_targets_for_missing_pane_selection() {
        let pane_one = tui_test_pane(1);
        let pane_two = tui_test_pane(2);
        let mut state = crate::app::tui::TuiState::default();
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 80,
            height: 12,
        };
        state.replace_panes(vec![pane_one.clone(), pane_two]);

        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(state.key_targets.get(&'2').map(String::as_str), Some("%2"));

        state.replace_panes(vec![pane_one]);
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);

        assert_eq!(
            state.retired_key_targets.get(&'2').map(String::as_str),
            Some("%2")
        );
    }

    #[test]
    fn tui_removal_does_not_reuse_missing_pane_key_before_retiring_it() {
        let pane_one = tui_test_pane(1);
        let pane_two = tui_test_pane(2);
        let mut state = crate::app::tui::TuiState::default();
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 80,
            height: 12,
        };
        state.replace_panes(vec![pane_one, pane_two.clone()]);

        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(state.key_targets.get(&'1').map(String::as_str), Some("%1"));
        assert_eq!(state.key_targets.get(&'2').map(String::as_str), Some("%2"));

        state.replace_panes(vec![pane_two]);
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);

        assert_eq!(state.key_targets.get(&'1').map(String::as_str), None);
        assert_eq!(state.key_targets.get(&'2').map(String::as_str), Some("%2"));
        assert_eq!(
            state.retired_key_targets.get(&'1').map(String::as_str),
            Some("%1")
        );
    }

    #[test]
    fn tui_non_session_removal_resets_shifted_visible_keys() {
        let pane_one = tmux_pane_row(1)
            .session_name("work")
            .pane_id("%1")
            .command("codex")
            .title("Task 1")
            .current_path("/work/alpha")
            .pane();
        let pane_two = tmux_pane_row(2)
            .session_name("work")
            .pane_id("%2")
            .command("codex")
            .title("Task 2")
            .current_path("/work/beta")
            .pane();
        let mut state = crate::app::tui::TuiState::with_picker_config(
            crate::app::picker::PickerKeySet::default(),
            crate::app::picker::PickerGroupBy::Cwd,
        );
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 80,
            height: 12,
        };
        state.replace_panes(vec![pane_one, pane_two.clone()]);

        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(state.key_targets.get(&'1').map(String::as_str), Some("%1"));
        assert_eq!(state.key_targets.get(&'2').map(String::as_str), Some("%2"));

        state.replace_panes(vec![pane_two]);
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);

        assert_eq!(state.key_targets.get(&'1').map(String::as_str), Some("%2"));
        assert_eq!(state.key_targets.get(&'2').map(String::as_str), None);
        assert_eq!(
            state.retired_key_targets.get(&'1').map(String::as_str),
            None
        );
    }

    #[test]
    fn tui_selection_defaults_to_first_visible_row() {
        let mut state = crate::app::tui::TuiState::default();
        state.replace_panes(vec![tui_test_pane(1), tui_test_pane(2)]);

        let frame = crate::app::tui::render_tui_frame_for_size(
            &mut state,
            crate::app::tui::TuiTerminalSize {
                width: 120,
                height: 10,
            },
        );

        assert_eq!(frame.selected_row, Some(0));
        assert_eq!(state.selected_pane_id.as_deref(), Some("%1"));
    }

    #[test]
    fn tui_arrow_selection_moves_within_visible_rows() {
        let mut state = crate::app::tui::TuiState::default();
        state.replace_panes(vec![tui_test_pane(1), tui_test_pane(2), tui_test_pane(3)]);
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 120,
            height: 10,
        };
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);

        assert!(state.select_next());
        assert_eq!(state.selected_pane_id.as_deref(), Some("%2"));
        let frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(frame.selected_row, Some(1));

        assert!(state.select_next());
        assert_eq!(state.selected_pane_id.as_deref(), Some("%3"));
        assert!(!state.select_next());
        assert_eq!(state.selected_pane_id.as_deref(), Some("%3"));

        assert!(state.select_previous());
        assert!(state.select_previous());
        assert_eq!(state.selected_pane_id.as_deref(), Some("%1"));
        assert!(!state.select_previous());
        assert_eq!(state.selected_pane_id.as_deref(), Some("%1"));
    }

    #[test]
    fn tui_arrow_selection_crosses_page_boundaries() {
        let mut state = crate::app::tui::TuiState::default();
        state.replace_panes((1..=8).map(tui_test_pane).collect());
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 120,
            height: 6,
        };
        let first_frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(first_frame.page_size, 4);

        for _ in 0..3 {
            assert!(state.select_next());
        }
        assert_eq!(state.selected_pane_id.as_deref(), Some("%4"));

        assert!(state.select_next());
        assert_eq!(state.selected_pane_id.as_deref(), Some("%5"));
        let second_page_frame =
            crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(second_page_frame.page_start, 4);
        assert_eq!(second_page_frame.selected_row, Some(0));

        assert!(state.select_previous());
        assert_eq!(state.selected_pane_id.as_deref(), Some("%4"));
        let first_page_frame =
            crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(first_page_frame.page_start, 0);
        assert_eq!(first_page_frame.selected_row, Some(3));
    }

    #[test]
    fn tui_selection_follows_pane_across_live_updates() {
        let mut state = crate::app::tui::TuiState::default();
        state.replace_panes(vec![tui_test_pane(1), tui_test_pane(2), tui_test_pane(3)]);
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 120,
            height: 10,
        };
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert!(state.select_next());
        assert_eq!(state.selected_pane_id.as_deref(), Some("%2"));

        state.replace_panes(vec![tui_test_pane(2), tui_test_pane(3)]);
        let frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);

        assert_eq!(state.selected_pane_id.as_deref(), Some("%2"));
        assert_eq!(frame.selected_row, Some(0));
    }

    #[test]
    fn tui_selection_snaps_to_first_visible_when_selected_pane_removed() {
        let mut state = crate::app::tui::TuiState::default();
        state.replace_panes(vec![tui_test_pane(1), tui_test_pane(2), tui_test_pane(3)]);
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 120,
            height: 10,
        };
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert!(state.select_next());
        assert_eq!(state.selected_pane_id.as_deref(), Some("%2"));

        state.replace_panes(vec![tui_test_pane(1), tui_test_pane(3)]);
        let frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);

        assert_eq!(state.selected_pane_id.as_deref(), Some("%1"));
        assert_eq!(frame.selected_row, Some(0));
    }

    #[test]
    fn tui_selection_clears_when_snapshot_empties() {
        let mut state = crate::app::tui::TuiState::default();
        state.replace_panes(vec![tui_test_pane(1)]);
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 120,
            height: 10,
        };
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(state.selected_pane_id.as_deref(), Some("%1"));

        state.replace_panes(Vec::new());
        let frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);

        assert_eq!(frame.selected_row, None);
        assert_eq!(state.selected_pane_id.as_deref(), None);
        assert!(!state.select_next());
        assert!(!state.select_previous());
    }

    #[test]
    fn tui_arrow_keys_move_selection_and_request_redraw() {
        let mut state = crate::app::tui::TuiState::default();
        state.replace_panes(vec![tui_test_pane(1), tui_test_pane(2)]);
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 120,
            height: 10,
        };
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        let key =
            |code| crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE);

        let down =
            crate::app::tui::handle_key_event(&key(crossterm::event::KeyCode::Down), &mut state)
                .expect("down arrow should not error");
        assert!(matches!(down, crate::app::tui::TuiLoopAction::Redraw));
        assert_eq!(state.selected_pane_id.as_deref(), Some("%2"));

        let up = crate::app::tui::handle_key_event(&key(crossterm::event::KeyCode::Up), &mut state)
            .expect("up arrow should not error");
        assert!(matches!(up, crate::app::tui::TuiLoopAction::Redraw));
        assert_eq!(state.selected_pane_id.as_deref(), Some("%1"));

        let clamped =
            crate::app::tui::handle_key_event(&key(crossterm::event::KeyCode::Up), &mut state)
                .expect("clamped up arrow should not error");
        assert!(matches!(clamped, crate::app::tui::TuiLoopAction::Continue));
        assert_eq!(state.selected_pane_id.as_deref(), Some("%1"));
    }

    #[test]
    fn tui_arrow_selection_moves_after_reanchor_to_non_aligned_page_start() {
        // A live update can reanchor page_start to a non-page-aligned index inside
        // the final partial page (here: index 9 of 10 with page_size 4). Arrow
        // movement must operate on exactly the rows the frame shows there, so Up
        // pages back instead of selecting an off-screen row and appearing dead.
        let pane = |index: u32, cwd: String| {
            tmux_pane_row(index)
                .session_name("work")
                .pane_id(format!("%{index}"))
                .command("codex")
                .title(format!("Task {index}"))
                .current_path(cwd)
                .pane()
        };
        let mut state = crate::app::tui::TuiState::with_picker_config(
            crate::app::picker::PickerKeySet::default(),
            crate::app::picker::PickerGroupBy::Cwd,
        );
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 120,
            height: 6,
        };
        state.replace_panes(
            (1..=10)
                .map(|index| pane(index, format!("/work/p{index:02}")))
                .collect(),
        );
        let first_frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(first_frame.page_size, 4);
        assert!(state.next_page());
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(state.selected_pane_id.as_deref(), Some("%5"));

        // Only %5 survives from the old visible page and nine new panes sort ahead
        // of it, so the reanchor lands on the non-aligned index 9.
        let mut updated = (11..=19)
            .map(|index| pane(index, format!("/work/a{index:02}")))
            .collect::<Vec<_>>();
        updated.push(pane(5, "/work/p05".to_string()));
        state.replace_panes(updated);
        let reanchored_frame =
            crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(reanchored_frame.page_start, 9);
        assert_eq!(reanchored_frame.visible_pane_ids, vec!["%5"]);
        assert_eq!(state.selected_pane_id.as_deref(), Some("%5"));

        assert!(state.select_previous());
        let paged_frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(paged_frame.page_start, 5);
        let selected_row = paged_frame
            .selected_row
            .expect("selection should stay on a visible row");
        assert_eq!(
            paged_frame.visible_pane_ids[selected_row].as_str(),
            state
                .selected_pane_id
                .as_deref()
                .expect("selection should exist"),
            "the highlighted row must be the selected pane"
        );
    }

    #[test]
    fn tui_down_from_last_row_selects_exactly_the_row_below_after_reanchor() {
        // With a non-aligned page_start near the tail (here 5 of 10, page_size 4),
        // next_page() clamps to the aligned boundary 8, so Down from the last
        // visible row used to re-select the already-highlighted row at index 8 and
        // skip the real next row.
        let pane = |index: u32, cwd: String| {
            tmux_pane_row(index)
                .session_name("work")
                .pane_id(format!("%{index}"))
                .command("codex")
                .title(format!("Task {index}"))
                .current_path(cwd)
                .pane()
        };
        let mut state = crate::app::tui::TuiState::with_picker_config(
            crate::app::picker::PickerKeySet::default(),
            crate::app::picker::PickerGroupBy::Cwd,
        );
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 120,
            height: 6,
        };
        state.replace_panes(
            (1..=10)
                .map(|index| pane(index, format!("/work/p{index:02}")))
                .collect(),
        );
        let first_frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(first_frame.page_size, 4);
        assert!(state.next_page());
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(state.selected_pane_id.as_deref(), Some("%5"));

        // Five new panes sort ahead; the previously visible %5..%8 survive with %5
        // landing at the non-aligned index 5.
        let mut updated = (11..=15)
            .map(|index| pane(index, format!("/work/a{index:02}")))
            .collect::<Vec<_>>();
        updated.extend((5..=9).map(|index| pane(index, format!("/work/p{index:02}"))));
        state.replace_panes(updated);
        let reanchored_frame =
            crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(reanchored_frame.page_start, 5);
        assert_eq!(
            reanchored_frame.visible_pane_ids,
            vec!["%5", "%6", "%7", "%8"]
        );

        for _ in 0..3 {
            assert!(state.select_next());
        }
        assert_eq!(state.selected_pane_id.as_deref(), Some("%8"));

        assert!(state.select_next());
        assert_eq!(state.selected_pane_id.as_deref(), Some("%9"));
        let frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(frame.page_start, 8);
        assert_eq!(frame.selected_row, Some(1));
        assert_eq!(frame.visible_pane_ids[1], "%9");
    }

    #[test]
    fn tui_up_from_first_row_reveals_exactly_the_row_above_after_reanchor() {
        // A live insert above the list can reanchor page_start to a small
        // non-aligned index (here 2). Up from the first visible row must reveal
        // exactly the row above the window and highlight it — not page back a full
        // window and select its last row, which was already on screen.
        let pane = |index: u32, cwd: String| {
            tmux_pane_row(index)
                .session_name("work")
                .pane_id(format!("%{index}"))
                .command("codex")
                .title(format!("Task {index}"))
                .current_path(cwd)
                .pane()
        };
        let mut state = crate::app::tui::TuiState::with_picker_config(
            crate::app::picker::PickerKeySet::default(),
            crate::app::picker::PickerGroupBy::Cwd,
        );
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 120,
            height: 6,
        };
        state.replace_panes(
            (1..=8)
                .map(|index| pane(index, format!("/work/p{index:02}")))
                .collect(),
        );
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(state.selected_pane_id.as_deref(), Some("%1"));

        // Two new panes sort ahead of the whole list; %1 (first surviving visible
        // pane) lands at index 2 and the reanchor keeps it at the window top.
        let mut updated = (11..=12)
            .map(|index| pane(index, format!("/work/a{index:02}")))
            .collect::<Vec<_>>();
        updated.extend((1..=8).map(|index| pane(index, format!("/work/p{index:02}"))));
        state.replace_panes(updated);
        let reanchored_frame =
            crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(reanchored_frame.page_start, 2);
        assert_eq!(reanchored_frame.visible_pane_ids[0], "%1");
        assert_eq!(state.selected_pane_id.as_deref(), Some("%1"));

        assert!(state.select_previous());
        assert_eq!(state.selected_pane_id.as_deref(), Some("%12"));
        let frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(frame.page_start, 0);
        assert_eq!(frame.selected_row, Some(1));
        assert_eq!(frame.visible_pane_ids[1], "%12");
    }

    #[test]
    fn tui_selection_snaps_to_first_visible_when_selected_pane_moves_off_page() {
        // Deliberate contract: when a live update pushes the still-existing selected
        // pane off the visible page, the highlight snaps to the first visible row
        // instead of re-paging to chase it (the page anchor follows the previously
        // visible rows, not the selection).
        let pane = |index: u32, cwd: String| {
            tmux_pane_row(index)
                .session_name("work")
                .pane_id(format!("%{index}"))
                .command("codex")
                .title(format!("Task {index}"))
                .current_path(cwd)
                .pane()
        };
        let mut state = crate::app::tui::TuiState::with_picker_config(
            crate::app::picker::PickerKeySet::default(),
            crate::app::picker::PickerGroupBy::Cwd,
        );
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 120,
            height: 6,
        };
        state.replace_panes(
            (1..=8)
                .map(|index| pane(index, format!("/work/p{index:02}")))
                .collect(),
        );
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert!(state.select_next());
        assert_eq!(state.selected_pane_id.as_deref(), Some("%2"));

        // %2 moves to a workspace that sorts last; it still exists but leaves page 1.
        let reordered = (1..=8)
            .map(|index| {
                let cwd = if index == 2 {
                    "/work/p99".to_string()
                } else {
                    format!("/work/p{index:02}")
                };
                pane(index, cwd)
            })
            .collect::<Vec<_>>();
        state.replace_panes(reordered);
        let frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);

        assert!(!frame.visible_pane_ids.contains(&"%2".to_string()));
        assert_eq!(frame.selected_row, Some(0));
        assert_eq!(
            state.selected_pane_id.as_deref(),
            Some(frame.visible_pane_ids[0].as_str())
        );
    }

    #[test]
    fn tui_slash_enters_search_mode_and_suspends_key_labels() {
        let mut state = crate::app::tui::TuiState::default();
        state.replace_panes(vec![tui_test_pane(1), tui_test_pane(2)]);
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 120,
            height: 10,
        };
        let normal_frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert!(normal_frame.lines[0].starts_with("❯[1]"));

        let action = crate::app::tui::handle_key_event(
            &tui_key_event(crossterm::event::KeyCode::Char('/')),
            &mut state,
        )
        .expect("slash should not error");
        assert!(matches!(action, crate::app::tui::TuiLoopAction::Redraw));
        assert_eq!(state.search_query.as_deref(), Some(""));

        let frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert!(frame.lines[0].starts_with("❯   "));
        assert_eq!(state.key_targets.get(&'1').map(String::as_str), None);
        let input_line = &frame.lines[frame.lines.len() - 2];
        assert!(input_line.starts_with("/▌"));
        let mode_line = &frame.lines[frame.lines.len() - 1];
        assert!(mode_line.contains("2/2 matched"));
        assert!(mode_line.contains("Esc cancel."));
    }

    #[test]
    fn tui_slash_without_panes_is_ignored() {
        let mut state = crate::app::tui::TuiState::default();

        let action = crate::app::tui::handle_key_event(
            &tui_key_event(crossterm::event::KeyCode::Char('/')),
            &mut state,
        )
        .expect("slash should not error");

        assert!(matches!(action, crate::app::tui::TuiLoopAction::Continue));
        assert_eq!(state.search_query.as_deref(), None);
    }

    #[test]
    fn tui_search_backspace_edits_query_and_esc_cancels_to_full_list() {
        let mut state = crate::app::tui::TuiState::default();
        state.replace_panes(vec![tui_test_pane(1), tui_test_pane(2)]);
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 120,
            height: 10,
        };
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert!(state.begin_search());

        type_tui_search_query(&mut state, "z/");
        assert_eq!(state.search_query.as_deref(), Some("z/"));
        let frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert!(frame.lines[0].contains("No panes match \"z/\"."));
        assert!(frame.visible_pane_ids.is_empty());
        assert_eq!(frame.selected_row, None);
        assert_eq!(state.selected_pane_id.as_deref(), None);

        let backspace = crate::app::tui::handle_key_event(
            &tui_key_event(crossterm::event::KeyCode::Backspace),
            &mut state,
        )
        .expect("backspace should not error");
        assert!(matches!(backspace, crate::app::tui::TuiLoopAction::Redraw));
        assert_eq!(state.search_query.as_deref(), Some("z"));
        assert!(state.pop_search_char());
        let frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(frame.visible_pane_ids, vec!["%1", "%2"]);

        // Backspace on an empty query stays in search mode without redrawing.
        let empty_backspace = crate::app::tui::handle_key_event(
            &tui_key_event(crossterm::event::KeyCode::Backspace),
            &mut state,
        )
        .expect("backspace should not error");
        assert!(matches!(
            empty_backspace,
            crate::app::tui::TuiLoopAction::Continue
        ));

        let escape = crate::app::tui::handle_key_event(
            &tui_key_event(crossterm::event::KeyCode::Esc),
            &mut state,
        )
        .expect("esc should not error");
        assert!(matches!(escape, crate::app::tui::TuiLoopAction::Redraw));
        assert_eq!(state.search_query.as_deref(), None);
        let frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert!(frame.lines[0].starts_with("❯[1]"));

        let close = crate::app::tui::handle_key_event(
            &tui_key_event(crossterm::event::KeyCode::Esc),
            &mut state,
        )
        .expect("esc should not error");
        assert!(matches!(close, crate::app::tui::TuiLoopAction::Close));
    }

    #[test]
    fn tui_search_suspends_letter_hotkeys() {
        let mut state = crate::app::tui::TuiState::default();
        state.replace_panes(vec![tui_test_pane(1), tui_test_pane(2)]);
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 120,
            height: 10,
        };
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(state.key_targets.get(&'2').map(String::as_str), Some("%2"));
        assert!(state.begin_search());

        // '2' focused %2 in normal mode; in search mode it types into the query
        // (and would have returned Close if the hotkey path had fired).
        let action = crate::app::tui::handle_key_event(
            &tui_key_event(crossterm::event::KeyCode::Char('2')),
            &mut state,
        )
        .expect("typing should not error");
        assert!(matches!(action, crate::app::tui::TuiLoopAction::Redraw));
        assert_eq!(state.search_query.as_deref(), Some("2"));

        let frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(frame.visible_pane_ids, vec!["%2"]);
    }

    #[test]
    fn tui_search_arrows_navigate_filtered_rows_across_pages() {
        let mut state = crate::app::tui::TuiState::default();
        state.replace_panes(
            (1..=10)
                .map(|index| {
                    let title = if index % 2 == 1 {
                        format!("redwood {index}")
                    } else {
                        format!("bluebell {index}")
                    };
                    tui_search_pane(index, &title)
                })
                .collect(),
        );
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 120,
            height: 6,
        };
        let first_frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(first_frame.page_size, 4);
        assert!(state.begin_search());
        type_tui_search_query(&mut state, "red");

        let frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(frame.visible_pane_ids, vec!["%1", "%3", "%5", "%7"]);
        assert_eq!(frame.selected_row, Some(0));
        assert_eq!(frame.page_count, 2);

        for _ in 0..3 {
            assert!(state.select_next());
        }
        assert_eq!(state.selected_pane_id.as_deref(), Some("%7"));

        assert!(state.select_next());
        assert_eq!(state.selected_pane_id.as_deref(), Some("%9"));
        let second_page_frame =
            crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(second_page_frame.page_start, 4);
        assert_eq!(second_page_frame.visible_pane_ids, vec!["%9"]);
        assert_eq!(second_page_frame.selected_row, Some(0));
        let mode_line = &second_page_frame.lines[second_page_frame.lines.len() - 1];
        assert!(mode_line.contains("Page 2/2"));
        assert!(mode_line.contains("5/10 matched"));

        assert!(state.select_previous());
        assert_eq!(state.selected_pane_id.as_deref(), Some("%7"));
        let first_page_frame =
            crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(first_page_frame.page_start, 0);
        assert_eq!(first_page_frame.selected_row, Some(3));
    }

    #[test]
    fn tui_search_live_updates_keep_filter_and_selection() {
        let mut state = crate::app::tui::TuiState::default();
        state.replace_panes(vec![
            tui_search_pane(1, "redwood 1"),
            tui_search_pane(2, "bluebell 2"),
        ]);
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 120,
            height: 10,
        };
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert!(state.begin_search());
        type_tui_search_query(&mut state, "red");
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(state.selected_pane_id.as_deref(), Some("%1"));

        state.replace_panes(vec![
            tui_search_pane(1, "redwood 1"),
            tui_search_pane(2, "bluebell 2"),
            tui_search_pane(3, "redwood 3"),
        ]);
        let frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(state.search_query.as_deref(), Some("red"));
        assert_eq!(frame.visible_pane_ids, vec!["%1", "%3"]);
        assert_eq!(state.selected_pane_id.as_deref(), Some("%1"));
        let mode_line = &frame.lines[frame.lines.len() - 1];
        assert!(mode_line.contains("2/3 matched"));

        state.replace_panes(vec![
            tui_search_pane(2, "bluebell 2"),
            tui_search_pane(3, "redwood 3"),
        ]);
        let frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(frame.visible_pane_ids, vec!["%3"]);
        assert_eq!(state.selected_pane_id.as_deref(), Some("%3"));
    }

    #[test]
    fn tui_search_live_inserts_ahead_reanchor_filtered_page_to_visible_rows() {
        // Filtered-view counterpart of the normal-mode reanchor contract: when a
        // live update inserts matches ahead of the visible filtered window, the
        // window must follow its previously visible rows so the pane-anchored
        // selection stays on screen instead of snapping to a different pane.
        let pane = |index: u32, cwd: String| {
            tmux_pane_row(index)
                .session_name("work")
                .pane_id(format!("%{index}"))
                .command("codex")
                .title(format!("redwood {index}"))
                .current_path(cwd)
                .pane()
        };
        let mut state = crate::app::tui::TuiState::with_picker_config(
            crate::app::picker::PickerKeySet::default(),
            crate::app::picker::PickerGroupBy::Cwd,
        );
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 120,
            height: 6,
        };
        state.replace_panes(
            (1..=6)
                .map(|index| pane(index, format!("/work/p{index:02}")))
                .collect(),
        );
        let first_frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(first_frame.page_size, 4);
        assert!(state.begin_search());
        type_tui_search_query(&mut state, "red");
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert!(state.next_page());
        let frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(frame.visible_pane_ids, vec!["%5", "%6"]);
        assert_eq!(state.selected_pane_id.as_deref(), Some("%5"));

        // Four new matching panes sort ahead of the whole list; without the
        // filtered reanchor the preserved page_start 4 would now show %1..%4 and
        // the selection would snap off %5.
        let mut updated = (11..=14)
            .map(|index| pane(index, format!("/work/a{index:02}")))
            .collect::<Vec<_>>();
        updated.extend((1..=6).map(|index| pane(index, format!("/work/p{index:02}"))));
        state.replace_panes(updated);
        let frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);

        assert_eq!(frame.page_start, 8);
        assert_eq!(frame.visible_pane_ids, vec!["%5", "%6"]);
        assert_eq!(state.selected_pane_id.as_deref(), Some("%5"));
        assert_eq!(frame.selected_row, Some(0));
    }

    #[test]
    fn tui_search_inserts_between_window_top_and_selection_snap_selection() {
        // Deliberate contract (mirrors normal mode): the filtered window anchors
        // on its first surviving visible row, not the selection. Matches inserted
        // between the window top and a selection further down push the selection
        // out of the window, and it snaps to the first visible row on redraw.
        let pane = |index: u32, cwd: String| {
            tmux_pane_row(index)
                .session_name("work")
                .pane_id(format!("%{index}"))
                .command("codex")
                .title(format!("redwood {index}"))
                .current_path(cwd)
                .pane()
        };
        let mut state = crate::app::tui::TuiState::with_picker_config(
            crate::app::picker::PickerKeySet::default(),
            crate::app::picker::PickerGroupBy::Cwd,
        );
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 120,
            height: 6,
        };
        state.replace_panes(
            (1..=4)
                .map(|index| pane(index, format!("/work/p{index:02}")))
                .collect(),
        );
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert!(state.begin_search());
        type_tui_search_query(&mut state, "red");
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        for _ in 0..3 {
            assert!(state.select_next());
        }
        assert_eq!(state.selected_pane_id.as_deref(), Some("%4"));

        // Three matching panes sort between %1 (window top) and the rest.
        let mut updated = vec![pane(1, "/work/p01".to_string())];
        updated.extend((11..=13).map(|index| pane(index, format!("/work/p01-{index}"))));
        updated.extend((2..=4).map(|index| pane(index, format!("/work/p{index:02}"))));
        state.replace_panes(updated);
        let frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);

        assert_eq!(frame.visible_pane_ids, vec!["%1", "%11", "%12", "%13"]);
        assert_eq!(state.selected_pane_id.as_deref(), Some("%1"));
        assert_eq!(frame.selected_row, Some(0));
    }

    #[test]
    fn tui_search_cancel_repages_to_keep_selection_visible() {
        let mut state = crate::app::tui::TuiState::default();
        state.replace_panes(
            (1..=10)
                .map(|index| {
                    let title = if index == 7 {
                        "target task".to_string()
                    } else {
                        format!("filler {index}")
                    };
                    tui_search_pane(index, &title)
                })
                .collect(),
        );
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 120,
            height: 6,
        };
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert!(state.begin_search());
        type_tui_search_query(&mut state, "target");
        let frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(frame.visible_pane_ids, vec!["%7"]);
        assert_eq!(state.selected_pane_id.as_deref(), Some("%7"));

        assert!(state.cancel_search());
        let frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(frame.page_start, 4);
        assert_eq!(frame.selected_row, Some(2));
        assert_eq!(frame.visible_pane_ids[2], "%7");
        assert!(frame.lines[0].starts_with(" [1]"));
        assert!(frame.lines[2].starts_with("❯[3]"));
    }

    #[test]
    fn tui_search_entry_and_cancel_preserve_selection_and_page() {
        // `/` immediately followed by Esc must be lossless: the empty query lists
        // every pane, so entering search keeps the current page and the redraw
        // must not snap an off-first-page selection away.
        let mut state = crate::app::tui::TuiState::default();
        state.replace_panes(
            (1..=10)
                .map(|index| tui_search_pane(index, &format!("task {index}")))
                .collect(),
        );
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 120,
            height: 6,
        };
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert!(state.next_page());
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert!(state.select_next());
        assert_eq!(state.selected_pane_id.as_deref(), Some("%6"));

        let action = crate::app::tui::handle_key_event(
            &tui_key_event(crossterm::event::KeyCode::Char('/')),
            &mut state,
        )
        .expect("slash should not error");
        assert!(matches!(action, crate::app::tui::TuiLoopAction::Redraw));
        let frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(frame.page_start, 4);
        assert_eq!(state.selected_pane_id.as_deref(), Some("%6"));
        assert_eq!(frame.selected_row, Some(1));

        let escape = crate::app::tui::handle_key_event(
            &tui_key_event(crossterm::event::KeyCode::Esc),
            &mut state,
        )
        .expect("esc should not error");
        assert!(matches!(escape, crate::app::tui::TuiLoopAction::Redraw));
        let frame = crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(state.selected_pane_id.as_deref(), Some("%6"));
        assert_eq!(frame.page_start, 4);
        assert_eq!(frame.selected_row, Some(1));
        assert!(frame.lines[0].starts_with(" [1]"));
        assert!(frame.lines[1].starts_with("❯[2]"));
    }

    #[test]
    fn tui_search_enter_without_matches_keeps_tui_open() {
        let mut state = crate::app::tui::TuiState::default();
        state.replace_panes(vec![tui_test_pane(1)]);
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 120,
            height: 10,
        };
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert!(state.begin_search());
        type_tui_search_query(&mut state, "zzz");
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(state.selected_pane_id.as_deref(), None);

        let action = crate::app::tui::handle_key_event(
            &tui_key_event(crossterm::event::KeyCode::Enter),
            &mut state,
        )
        .expect("enter without matches should not error");
        assert!(matches!(action, crate::app::tui::TuiLoopAction::Continue));
    }

    #[test]
    fn tui_search_drops_when_frame_leaves_the_pane_list() {
        let mut state = crate::app::tui::TuiState::default();
        state.replace_panes(vec![tui_test_pane(1)]);
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 120,
            height: 10,
        };
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert!(state.begin_search());

        // Snapshot empties: the empty frame drops search mode so Esc closes again.
        state.replace_panes(Vec::new());
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(state.search_query.as_deref(), None);
        assert!(!state.is_searching());
    }

    #[test]
    fn tui_initial_selection_seeds_to_caller_pane_hint() {
        let mut state = crate::app::tui::TuiState::default();
        set_initial_selection_hint(&mut state, "%2");
        state.replace_panes(vec![tui_test_pane(1), tui_test_pane(2), tui_test_pane(3)]);

        let frame = crate::app::tui::render_tui_frame_for_size(
            &mut state,
            crate::app::tui::TuiTerminalSize {
                width: 120,
                height: 10,
            },
        );

        assert_eq!(state.selected_pane_id.as_deref(), Some("%2"));
        assert_eq!(frame.selected_row, Some(1));
        assert_eq!(state.initial_selection_hint.as_deref(), None);
        assert!(state.initial_selection_seeded);
    }

    #[test]
    fn tui_initial_selection_hint_beyond_page_one_repositions_page() {
        let mut state = crate::app::tui::TuiState::default();
        set_initial_selection_hint(&mut state, "%5");
        state.replace_panes((1..=5).map(tui_test_pane).collect());

        // Height 4 leaves a two-row page; %5 sits at view position 4 (page 3).
        let frame = crate::app::tui::render_tui_frame_for_size(
            &mut state,
            crate::app::tui::TuiTerminalSize {
                width: 120,
                height: 4,
            },
        );

        assert_eq!(state.selected_pane_id.as_deref(), Some("%5"));
        assert_eq!(frame.page_start, 4);
        assert_eq!(frame.selected_row, Some(0));
        assert_eq!(frame.visible_pane_ids, vec!["%5".to_string()]);
    }

    #[test]
    fn tui_initial_selection_hint_missing_is_consumed_and_never_reapplied() {
        let mut state = crate::app::tui::TuiState::default();
        set_initial_selection_hint(&mut state, "%9");
        state.replace_panes(vec![tui_test_pane(1), tui_test_pane(2)]);
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 120,
            height: 10,
        };
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);

        // The hint matched nothing: default first-row selection, hint consumed.
        assert_eq!(state.selected_pane_id.as_deref(), Some("%1"));
        assert_eq!(state.initial_selection_hint.as_deref(), None);

        // A later snapshot that contains the hinted pane must NOT select it.
        state.replace_panes(vec![tui_test_pane(1), tui_test_pane(2), tui_test_pane(9)]);
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(state.selected_pane_id.as_deref(), Some("%1"));
    }

    #[test]
    fn tui_initial_selection_hint_survives_connecting_and_undersized_frames() {
        let mut state = crate::app::tui::TuiState::default();
        set_initial_selection_hint(&mut state, "%2");

        // Connecting/empty frame: exits before the seed site, hint intact.
        crate::app::tui::render_tui_frame_for_size(
            &mut state,
            crate::app::tui::TuiTerminalSize {
                width: 120,
                height: 10,
            },
        );
        assert_eq!(state.initial_selection_hint.as_deref(), Some("%2"));
        assert!(!state.initial_selection_seeded);

        // Undersized frame with panes: also exits before the seed site.
        state.replace_panes(vec![tui_test_pane(1), tui_test_pane(2)]);
        crate::app::tui::render_tui_frame_for_size(
            &mut state,
            crate::app::tui::TuiTerminalSize {
                width: 120,
                height: 2,
            },
        );
        assert_eq!(state.initial_selection_hint.as_deref(), Some("%2"));
        assert!(!state.initial_selection_seeded);

        // The terminal grows: the hint finally applies.
        let frame = crate::app::tui::render_tui_frame_for_size(
            &mut state,
            crate::app::tui::TuiTerminalSize {
                width: 120,
                height: 10,
            },
        );
        assert_eq!(state.selected_pane_id.as_deref(), Some("%2"));
        assert_eq!(frame.selected_row, Some(1));
    }

    #[test]
    fn tui_initial_selection_seed_cancelled_by_navigation() {
        let mut state = crate::app::tui::TuiState::default();
        set_initial_selection_hint(&mut state, "%2");

        // The user acts before the first populated frame arrives.
        state.select_next();
        assert!(state.initial_selection_seeded);
        assert_eq!(state.initial_selection_hint.as_deref(), None);

        state.replace_panes(vec![tui_test_pane(1), tui_test_pane(2)]);
        crate::app::tui::render_tui_frame_for_size(
            &mut state,
            crate::app::tui::TuiTerminalSize {
                width: 120,
                height: 10,
            },
        );
        assert_eq!(state.selected_pane_id.as_deref(), Some("%1"));
    }

    #[test]
    fn tui_initial_selection_seeds_to_most_recent_focus_without_hint() {
        let mut recent = tui_test_pane(4);
        recent.last_focus_seq = Some(12);
        let mut older = tui_test_pane(2);
        older.last_focus_seq = Some(5);
        let mut state = crate::app::tui::TuiState::default();
        state.replace_panes(vec![tui_test_pane(1), older, tui_test_pane(3), recent]);

        // Height 4 leaves a two-row page; %4 sits at view position 3 (page 2).
        let frame = crate::app::tui::render_tui_frame_for_size(
            &mut state,
            crate::app::tui::TuiTerminalSize {
                width: 120,
                height: 4,
            },
        );

        assert_eq!(state.selected_pane_id.as_deref(), Some("%4"));
        assert_eq!(frame.page_start, 2);
        assert_eq!(frame.selected_row, Some(1));
    }

    #[test]
    fn tui_initial_selection_hint_wins_over_focus_recency() {
        let mut recent = tui_test_pane(3);
        recent.last_focus_seq = Some(42);
        let mut state = crate::app::tui::TuiState::default();
        set_initial_selection_hint(&mut state, "%1");
        state.replace_panes(vec![tui_test_pane(1), tui_test_pane(2), recent]);

        crate::app::tui::render_tui_frame_for_size(
            &mut state,
            crate::app::tui::TuiTerminalSize {
                width: 120,
                height: 10,
            },
        );

        assert_eq!(state.selected_pane_id.as_deref(), Some("%1"));
    }

    #[test]
    fn tui_initial_selection_without_hint_or_recency_defaults_to_first_row() {
        let mut state = crate::app::tui::TuiState::default();
        state.replace_panes(vec![tui_test_pane(1), tui_test_pane(2)]);

        let frame = crate::app::tui::render_tui_frame_for_size(
            &mut state,
            crate::app::tui::TuiTerminalSize {
                width: 120,
                height: 10,
            },
        );

        assert_eq!(state.selected_pane_id.as_deref(), Some("%1"));
        assert_eq!(frame.selected_row, Some(0));
        assert!(state.initial_selection_seeded);
    }

    #[test]
    fn tui_initial_selection_seed_is_one_shot_after_first_populated_frame() {
        let mut state = crate::app::tui::TuiState::default();
        state.replace_panes(vec![tui_test_pane(1), tui_test_pane(2)]);
        let terminal_size = crate::app::tui::TuiTerminalSize {
            width: 120,
            height: 10,
        };
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(state.selected_pane_id.as_deref(), Some("%1"));

        // A hint arriving after the first populated frame (impossible in
        // production, but the latch must hold) is ignored.
        set_initial_selection_hint(&mut state, "%2");
        crate::app::tui::render_tui_frame_for_size(&mut state, terminal_size);
        assert_eq!(state.selected_pane_id.as_deref(), Some("%1"));
    }
}
