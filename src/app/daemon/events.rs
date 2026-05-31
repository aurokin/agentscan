use super::*;
use control_mode::ControlModeLineSource;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ControlEvent {
    PaneChanged(String),
    TitleChanged { pane_id: String, title: String },
    WindowChanged(String),
    SessionChanged(String),
    Resnapshot,
    Exit,
    Ignored,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ControlEventOutcome {
    pub(super) changed: bool,
    pub(super) fallback_to_full: bool,
    pub(super) full_snapshot_refresh: bool,
    pub(super) targeted_title_updates: u64,
    pub(super) targeted_pane_refreshes: u64,
    pub(super) targeted_scope_refreshes: u64,
}

#[derive(Debug, Default, Eq, PartialEq)]
pub(super) struct ControlEventBatch {
    pub(super) should_exit: bool,
    pub(super) next_sequence: u64,
    pub(super) ignored_count: u64,
    pub(super) total_line_count: u64,
    pub(super) output_line_count: u64,
    pub(super) output_byte_count: u64,
    pub(super) resnapshot_sequence: Option<u64>,
    pub(super) sessions: BTreeMap<String, u64>,
    pub(super) windows: BTreeMap<String, u64>,
    pub(super) panes: BTreeMap<String, u64>,
    pub(super) titles: BTreeMap<String, SequencedTitle>,
    pub(super) control_sources: Vec<ipc::ControlModeSourceFrame>,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct SequencedTitle {
    pub(super) sequence: u64,
    pub(super) title: String,
}

impl ControlEventBatch {
    pub(super) fn from_lines(lines: &[String]) -> Self {
        let mut batch = Self::default();
        for line in lines {
            batch.push_line(line);
        }
        batch
    }

    pub(super) fn from_control_lines(lines: &[ControlModeLine]) -> Self {
        let mut batch = Self::default();
        let mut source_indexes = Vec::<(ControlModeLineSource, usize)>::new();
        for frame in lines {
            let event = batch.push_line(&frame.line);
            let source_index = if let Some((_, index)) = source_indexes
                .iter()
                .find(|(source, _)| source == &frame.source)
            {
                *index
            } else {
                let index = batch.control_sources.len();
                source_indexes.push((frame.source.clone(), index));
                batch.control_sources.push(frame.source_frame_seed());
                index
            };
            if let Some(source) = batch.control_sources.get_mut(source_index) {
                source.line_count = source.line_count.saturating_add(1);
                if !matches!(event, ControlEvent::Ignored) {
                    source.event_count = source.event_count.saturating_add(1);
                }
            }
        }
        batch
    }

    fn push_line(&mut self, line: &str) -> ControlEvent {
        self.total_line_count = self.total_line_count.saturating_add(1);
        // Count every `%output` line (title-bearing or not) so the firehose is
        // sized cheaply during the single parse pass; `%output` lines that yield
        // a terminal title are also counted in `titles`, so the firehose waste is
        // `output_line_count - title count`.
        if line.starts_with("%output") {
            self.output_line_count = self.output_line_count.saturating_add(1);
            // `str::len()` is the UTF-8 *byte* length (not a char count), which is
            // exactly the metric here: the on-the-wire size of the `%output` line
            // we read and process. This sizes the firehose volume we pay for, not
            // the decoded terminal payload (tmux octal-escapes non-printable bytes
            // in `%output`, so the escaped line is what actually costs us).
            self.output_byte_count = self
                .output_byte_count
                .saturating_add(u64::try_from(line.len()).unwrap_or(u64::MAX));
        }
        let event = control_event_from_line(line);
        self.push(event.clone());
        event
    }

    pub(super) fn push(&mut self, event: ControlEvent) {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        match event {
            ControlEvent::Exit => self.should_exit = true,
            ControlEvent::Ignored => self.ignored_count = self.ignored_count.saturating_add(1),
            ControlEvent::Resnapshot => self.resnapshot_sequence = Some(sequence),
            ControlEvent::SessionChanged(session_id) => {
                self.sessions.insert(session_id, sequence);
            }
            ControlEvent::WindowChanged(window_id) => {
                self.windows.insert(window_id, sequence);
            }
            ControlEvent::PaneChanged(pane_id) => {
                self.panes.insert(pane_id, sequence);
            }
            ControlEvent::TitleChanged { pane_id, title } => {
                self.titles
                    .insert(pane_id, SequencedTitle { sequence, title });
            }
        }
    }

    pub(super) fn can_refresh_full_snapshot(&self) -> bool {
        self.resnapshot_sequence.is_some() || !self.sessions.is_empty() || !self.windows.is_empty()
    }

    pub(super) fn publish_context(&self) -> Option<SnapshotPublishContext> {
        if self.resnapshot_sequence.is_some() {
            return ControlEvent::Resnapshot.publish_context();
        }

        let event_count = self.sessions.len()
            + self.windows.len()
            + self.panes.len()
            + self
                .titles
                .keys()
                .filter(|pane_id| !self.panes.contains_key(*pane_id))
                .count();
        if event_count != 1 {
            return (event_count > 1)
                .then(|| SnapshotPublishContext::new("control_event").with_detail("batch"));
        }

        if let Some((session_id, _)) = self.sessions.iter().next() {
            return ControlEvent::SessionChanged(session_id.clone()).publish_context();
        }
        if let Some((window_id, _)) = self.windows.iter().next() {
            return ControlEvent::WindowChanged(window_id.clone()).publish_context();
        }
        if let Some((pane_id, _)) = self.panes.iter().next() {
            return ControlEvent::PaneChanged(pane_id.clone()).publish_context();
        }
        if let Some((pane_id, title)) = self.titles.iter().next() {
            return ControlEvent::TitleChanged {
                pane_id: pane_id.clone(),
                title: title.title.clone(),
            }
            .publish_context();
        }

        None
    }

    pub(super) fn has_telemetry_event(&self) -> bool {
        self.should_exit
            || self.resnapshot_sequence.is_some()
            || !self.sessions.is_empty()
            || !self.windows.is_empty()
            || !self.panes.is_empty()
            || !self.titles.is_empty()
    }

    pub(super) fn observability_refresh(&self) -> &'static str {
        if self.resnapshot_sequence.is_some() {
            return "full_snapshot";
        }
        if !self.sessions.is_empty() || !self.windows.is_empty() {
            return "targeted_scope";
        }
        if !self.panes.is_empty() || !self.titles.is_empty() {
            return "targeted_pane";
        }
        "none"
    }

    pub(super) fn observability_detail(&self) -> ObservabilityDetail {
        if self.resnapshot_sequence.is_some() {
            return ObservabilityDetail::Static("resnapshot");
        }
        let event_count =
            self.sessions.len() + self.windows.len() + self.panes.len() + self.titles.len();
        if event_count > 1 {
            return ObservabilityDetail::Static("batch");
        }
        if let Some(session_id) = self.sessions.keys().next() {
            return ObservabilityDetail::Owned(format!("session:{session_id}"));
        }
        if let Some(window_id) = self.windows.keys().next() {
            return ObservabilityDetail::Owned(format!("window:{window_id}"));
        }
        if let Some(pane_id) = self.panes.keys().next() {
            return ObservabilityDetail::Owned(format!("pane:{pane_id}"));
        }
        if let Some(pane_id) = self.titles.keys().next() {
            return ObservabilityDetail::Owned(format!("title:{pane_id}"));
        }
        if self.ignored_count > 0 {
            return ObservabilityDetail::Ignored(self.ignored_count);
        }
        ObservabilityDetail::None
    }
}

impl ControlEvent {
    fn publish_context(&self) -> Option<SnapshotPublishContext> {
        match self {
            ControlEvent::PaneChanged(pane_id) => Some(
                SnapshotPublishContext::new("control_event").with_detail(format!("pane:{pane_id}")),
            ),
            ControlEvent::TitleChanged { pane_id, .. } => Some(
                SnapshotPublishContext::new("control_event")
                    .with_detail(format!("title:{pane_id}")),
            ),
            ControlEvent::WindowChanged(window_id) => Some(
                SnapshotPublishContext::new("control_event")
                    .with_detail(format!("window:{window_id}")),
            ),
            ControlEvent::SessionChanged(session_id) => Some(
                SnapshotPublishContext::new("control_event")
                    .with_detail(format!("session:{session_id}")),
            ),
            ControlEvent::Resnapshot => {
                Some(SnapshotPublishContext::new("control_event").with_detail("resnapshot"))
            }
            ControlEvent::Exit | ControlEvent::Ignored => None,
        }
    }
}

// A tmux control-mode `%exit` notification: either bare `%exit` or `%exit
// <reason>`. Matching the exact token (not a bare `%exit` prefix) avoids
// classifying a hypothetical `%exit`-prefixed token as an exit. The subscriber
// reader filter (`subscriber_local_exit`) reuses this so the per-session filter
// and this parser cannot diverge on what counts as an exit line.
pub(super) fn is_control_exit_line(line: &str) -> bool {
    line == "%exit" || line.starts_with("%exit ")
}

pub(super) fn control_event_from_line(line: &str) -> ControlEvent {
    if is_control_exit_line(line) {
        // A primary `%exit` stops the daemon. Note that tmux also emits `%exit` to a
        // control client when only its *attached* session is killed while the server
        // and other sessions survive (empirically confirmed) — so killing the
        // session the primary attached to stops the daemon even though other sessions
        // remain. This is long-standing behavior, not introduced here: `main` likewise
        // attached the single primary to one session and stopped on its `%exit`. The
        // daemon is re-spawned on the next CLI call (which re-resolves a live primary),
        // so monitoring resumes; true mid-session failover to a surviving session
        // would require un-pinning the primary and is a deliberate future enhancement,
        // not a regression in this diff. Subscriber `%exit` is filtered upstream
        // (`subscriber_local_exit`) and never reaches this parser.
        return ControlEvent::Exit;
    }

    if let Some(pane_id) = subscription_changed_pane_id(line) {
        return ControlEvent::PaneChanged(pane_id.to_string());
    }

    if let Some(change) = output_title_change(line) {
        return ControlEvent::TitleChanged {
            pane_id: change.pane_id.to_string(),
            title: change.title,
        };
    }

    if let Some(window_id) = window_notification_target(line) {
        return ControlEvent::WindowChanged(window_id.to_string());
    }

    if let Some(session_id) = session_notification_target(line) {
        return ControlEvent::SessionChanged(session_id.to_string());
    }

    if should_resnapshot_from_notification(line) {
        return ControlEvent::Resnapshot;
    }

    ControlEvent::Ignored
}

// True when a batch contains a notification indicating the set of sessions on
// the server changed (a session was created or destroyed), which is the trigger
// to re-derive the per-session subscriber clients.
pub(super) fn batch_changed_session_set(lines: &[ControlModeLine]) -> bool {
    lines
        .iter()
        .any(|line| notification_name(&line.line) == Some("%sessions-changed"))
}

pub(crate) fn should_resnapshot_from_notification(line: &str) -> bool {
    matches!(
        notification_name(line),
        Some(
            "%sessions-changed"
                | "%session-changed"
                | "%session-renamed"
                | "%session-window-changed"
                | "%layout-change"
                | "%window-add"
                | "%window-close"
                | "%unlinked-window-close"
                | "%window-pane-changed"
                | "%window-renamed"
        )
    )
}

pub(crate) fn subscription_changed_pane_id(line: &str) -> Option<&str> {
    let mut fields = line.split_whitespace();
    if fields.next()? != "%subscription-changed" {
        return None;
    }
    let _subscription_name = fields.next()?;
    let _session = fields.next()?;
    let _window = fields.next()?;
    let _flags = fields.next()?;
    let pane_id = fields.next()?;
    pane_id.starts_with('%').then_some(pane_id)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn output_title_change_pane_id(line: &str) -> Option<&str> {
    output_title_change(line).map(|change| change.pane_id)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn output_title_change_title(line: &str) -> Option<String> {
    output_title_change(line).map(|change| change.title)
}

struct OutputTitleChange<'a> {
    pane_id: &'a str,
    title: String,
}

fn output_title_change(line: &str) -> Option<OutputTitleChange<'_>> {
    let mut fields = line.splitn(3, ' ');
    if fields.next()? != "%output" {
        return None;
    }

    let pane_id = fields.next()?;
    let payload = fields.next()?;
    let title = terminal_title_from_control_payload(payload)?;
    if !pane_id.starts_with('%') {
        return None;
    }

    Some(OutputTitleChange { pane_id, title })
}

fn terminal_title_from_control_payload(payload: &str) -> Option<String> {
    if !payload_may_contain_terminal_title(payload) {
        return None;
    }
    let decoded = decode_tmux_control_payload(payload);
    terminal_title_from_decoded_output(&decoded)
}

fn payload_may_contain_terminal_title(payload: &str) -> bool {
    payload.contains("\\033]0;")
        || payload.contains("\\033]2;")
        || payload.contains("\u{1b}]0;")
        || payload.contains("\u{1b}]2;")
}

fn decode_tmux_control_payload(payload: &str) -> String {
    let bytes = payload.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'\\'
            && index + 3 < bytes.len()
            && is_octal_digit(bytes[index + 1])
            && is_octal_digit(bytes[index + 2])
            && is_octal_digit(bytes[index + 3])
        {
            let value = ((bytes[index + 1] - b'0') << 6)
                | ((bytes[index + 2] - b'0') << 3)
                | (bytes[index + 3] - b'0');
            decoded.push(value);
            index += 4;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }

    String::from_utf8_lossy(&decoded).into_owned()
}

const fn is_octal_digit(byte: u8) -> bool {
    byte >= b'0' && byte <= b'7'
}

fn terminal_title_from_decoded_output(output: &str) -> Option<String> {
    let bytes = output.as_bytes();
    let mut index = 0;
    let mut title = None;

    while index + 4 <= bytes.len() {
        if bytes[index] == 0x1b
            && bytes[index + 1] == b']'
            && matches!(bytes[index + 2], b'0' | b'2')
            && bytes[index + 3] == b';'
        {
            let title_start = index + 4;
            let mut title_end = title_start;
            while title_end < bytes.len() {
                if bytes[title_end] == 0x07 {
                    title =
                        Some(String::from_utf8_lossy(&bytes[title_start..title_end]).into_owned());
                    index = title_end + 1;
                    break;
                }
                if title_end + 1 < bytes.len()
                    && bytes[title_end] == 0x1b
                    && bytes[title_end + 1] == b'\\'
                {
                    title =
                        Some(String::from_utf8_lossy(&bytes[title_start..title_end]).into_owned());
                    index = title_end + 2;
                    break;
                }
                title_end += 1;
            }

            if title_end == bytes.len() {
                break;
            }
        } else {
            index += 1;
        }
    }

    title
}

pub(crate) fn window_notification_target(line: &str) -> Option<&str> {
    match notification_name(line) {
        Some(
            "%layout-change"
            | "%window-add"
            | "%window-close"
            | "%unlinked-window-close"
            | "%unlinked-window-renamed"
            | "%window-pane-changed"
            | "%window-renamed",
        ) => line
            .split_whitespace()
            .nth(1)
            .filter(|value| value.starts_with('@')),
        _ => None,
    }
}

pub(crate) fn session_notification_target(line: &str) -> Option<&str> {
    match notification_name(line) {
        Some("%session-renamed") => line
            .split_whitespace()
            .nth(1)
            .filter(|value| value.starts_with('$')),
        _ => None,
    }
}

pub(crate) fn notification_name(line: &str) -> Option<&str> {
    line.split_whitespace()
        .next()
        .filter(|token| token.starts_with('%'))
}
