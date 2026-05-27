use anyhow::{Result, bail};
use serde::Serialize;

use super::{DisplayMetadata, PaneLocation, PaneRecord, PaneStatus, Provider};

pub(crate) const PICKER_SELECTION_KEYS: [char; 16] = [
    '1', '2', '3', '4', '5', 'Q', 'E', 'R', 'F', 'G', 'T', 'Z', 'X', 'C', 'V', 'B',
];

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct PickerRow {
    pub(crate) key: char,
    pub(crate) pane_id: String,
    pub(crate) provider: Option<Provider>,
    pub(crate) status: PaneStatus,
    pub(crate) display: DisplayMetadata,
    pub(crate) display_label: String,
    pub(crate) location: PaneLocation,
    pub(crate) location_tag: String,
    /// Whether this pane is the active pane of its window (`pane_active &&
    /// window_active`). True for one pane per session, so clients should prefer
    /// `is_focused` to mark the single live pane.
    pub(crate) is_active: bool,
    /// Whether this is the single pane the user is currently focused on: the
    /// active pane of the session the most-recently-active tmux client is viewing.
    /// At most one row is `is_focused`; all are `false` when nothing is attached.
    pub(crate) is_focused: bool,
    /// Number of clients attached to the tmux server. A server-level fact echoed
    /// on every row (the picker output is a flat array, so there is no envelope to
    /// carry it once); `>1` signals best-effort focus and a multiple-clients hint.
    pub(crate) attached_client_count: u32,
}

pub(crate) fn picker_rows(
    panes: &[PaneRecord],
    focused_session: Option<&str>,
    attached_client_count: u32,
) -> Vec<PickerRow> {
    panes
        .iter()
        .zip(PICKER_SELECTION_KEYS)
        .map(|(pane, key)| {
            let is_active = pane.is_active();
            PickerRow {
                key,
                pane_id: pane.pane_id.clone(),
                provider: pane.provider,
                status: pane.status.clone(),
                display: pane.display.clone(),
                display_label: pane.display.label.clone(),
                location: pane.location.clone(),
                location_tag: pane.location.tag(),
                is_active,
                // The focused pane is the active pane of the focused session, so
                // require both signals — that yields exactly one row.
                is_focused: is_active
                    && focused_session.is_some_and(|session| session == pane.location.session_name),
                attached_client_count,
            }
        })
        .collect()
}

pub(crate) fn normalize_picker_key(raw_key: &str) -> Result<char> {
    let trimmed = raw_key.trim();
    let mut characters = trimmed.chars();
    let Some(character) = characters.next() else {
        bail!("hotkey must be one of {}", picker_key_list());
    };
    if characters.next().is_some() {
        bail!(
            "hotkey {raw_key:?} must be a single key from {}",
            picker_key_list()
        );
    }

    let normalized = character.to_ascii_uppercase();
    if !PICKER_SELECTION_KEYS.contains(&normalized) {
        bail!(
            "hotkey {raw_key:?} is not supported; expected one of {}",
            picker_key_list()
        );
    }

    Ok(normalized)
}

pub(crate) fn picker_key_list() -> String {
    PICKER_SELECTION_KEYS
        .iter()
        .map(char::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}
