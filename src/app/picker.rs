use anyhow::{Result, bail};
use serde::Serialize;

use super::{DisplayMetadata, PaneLocation, PaneRecord, PaneStatus, Provider};

pub(crate) const DEFAULT_PICKER_SELECTION_KEYS: [char; 16] = [
    '1', '2', '3', '4', '5', 'Q', 'E', 'R', 'F', 'G', 'T', 'Z', 'X', 'C', 'V', 'B',
];

const RESERVED_PICKER_KEYS: [char; 2] = ['N', 'P'];

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PickerKeySet {
    keys: Vec<char>,
}

impl PickerKeySet {
    pub(crate) fn from_config_values(values: &[String]) -> Result<Self> {
        let mut keys = Vec::with_capacity(values.len());
        for value in values {
            let key = normalize_config_picker_key(value)?;
            if RESERVED_PICKER_KEYS.contains(&key) {
                bail!("picker key {value:?} is reserved for TUI paging");
            }
            if keys.contains(&key) {
                bail!("picker key {value:?} duplicates another configured key");
            }
            keys.push(key);
        }

        if keys.len() != DEFAULT_PICKER_SELECTION_KEYS.len() {
            bail!(
                "picker_keys must contain exactly {} keys",
                DEFAULT_PICKER_SELECTION_KEYS.len()
            );
        }

        Ok(Self { keys })
    }

    pub(crate) fn keys(&self) -> &[char] {
        &self.keys
    }

    pub(crate) fn len(&self) -> usize {
        self.keys.len()
    }

    fn contains(&self, key: char) -> bool {
        self.keys.contains(&key)
    }

    fn key_list(&self) -> String {
        self.keys
            .iter()
            .map(char::to_string)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl Default for PickerKeySet {
    fn default() -> Self {
        Self {
            keys: DEFAULT_PICKER_SELECTION_KEYS.to_vec(),
        }
    }
}

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
    picker_keys: &PickerKeySet,
) -> Vec<PickerRow> {
    panes
        .iter()
        .zip(picker_keys.keys().iter().copied())
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

pub(crate) fn normalize_picker_key(raw_key: &str, picker_keys: &PickerKeySet) -> Result<char> {
    let trimmed = raw_key.trim();
    let mut characters = trimmed.chars();
    let Some(character) = characters.next() else {
        bail!("hotkey must be one of {}", picker_key_list(picker_keys));
    };
    if characters.next().is_some() {
        bail!(
            "hotkey {raw_key:?} must be a single key from {}",
            picker_key_list(picker_keys)
        );
    }

    let normalized = character.to_ascii_uppercase();
    if !picker_keys.contains(normalized) {
        bail!(
            "hotkey {raw_key:?} is not supported; expected one of {}",
            picker_key_list(picker_keys)
        );
    }

    Ok(normalized)
}

pub(crate) fn picker_key_list(picker_keys: &PickerKeySet) -> String {
    picker_keys.key_list()
}

fn normalize_config_picker_key(raw_key: &str) -> Result<char> {
    let mut characters = raw_key.chars();
    let Some(character) = characters.next() else {
        bail!("picker_keys entries must be single ASCII letters or digits");
    };
    if characters.next().is_some() {
        bail!("picker key {raw_key:?} must be a single character");
    }
    if !character.is_ascii_alphanumeric() {
        bail!("picker key {raw_key:?} must be an ASCII letter or digit");
    }

    Ok(character.to_ascii_uppercase())
}
