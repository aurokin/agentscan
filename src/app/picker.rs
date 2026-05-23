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
}

pub(crate) fn picker_rows(panes: &[PaneRecord]) -> Vec<PickerRow> {
    panes
        .iter()
        .zip(PICKER_SELECTION_KEYS)
        .map(|(pane, key)| PickerRow {
            key,
            pane_id: pane.pane_id.clone(),
            provider: pane.provider,
            status: pane.status.clone(),
            display: pane.display.clone(),
            display_label: pane.display.label.clone(),
            location: pane.location.clone(),
            location_tag: pane.location.tag(),
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
