//! Single source of truth for the ordered tmux pane fields.
//!
//! A pane field used to be defined by position in three hand-maintained places:
//! the `list-panes -F` format string, the positional parser, and the daemon
//! subscription format. Keeping those in sync by hand caused real drift bugs.
//!
//! Every field is now declared exactly once, in parse order, in [`PANE_FIELDS`].
//! Three things are derived from that one table so they can never disagree:
//!   * [`PANE_FORMAT`] — the `list-panes -F` format string.
//!   * the parser's field indices and block lengths (see `tmux::parse`).
//!   * [`DAEMON_SUBSCRIPTION_FORMAT`] — the control-mode subscription payload.

/// Stable identity for each pane field, independent of its runtime position.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum PaneFieldId {
    SessionName,
    WindowIndex,
    PaneIndex,
    PaneId,
    PanePid,
    PaneCurrentCommand,
    PaneTitle,
    PaneTty,
    PaneCurrentPath,
    WindowName,
    SessionId,
    WindowId,
    AgentProvider,
    AgentLabel,
    AgentCwd,
    AgentState,
    AgentSessionId,
    AgentPid,
    AgentVersion,
    AgentModel,
    PaneActive,
    WindowActive,
}

/// Structural blocks of a pane row. `Core` and `Active` are always present;
/// `Ids` and `Agent` are optional, so older/narrower rows omit them.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum PaneFieldBlock {
    /// Always present at the front of every row.
    Core,
    /// `#{session_id}` + `#{window_id}`; optional.
    Ids,
    /// The `@agent.*` wrapper-metadata block; optional.
    Agent,
    /// `#{pane_active}` + `#{window_active}`; always the trailing fields.
    Active,
}

struct PaneField {
    id: PaneFieldId,
    block: PaneFieldBlock,
    /// tmux format directive, emitted verbatim into the format string.
    directive: &'static str,
    /// Whether this field is part of the daemon subscription payload.
    in_subscription: bool,
}

use PaneFieldBlock::*;
use PaneFieldId::*;

const fn entry(
    id: PaneFieldId,
    block: PaneFieldBlock,
    directive: &'static str,
    in_subscription: bool,
) -> PaneField {
    PaneField {
        id,
        block,
        directive,
        in_subscription,
    }
}

/// The ordered pane-field table. This order is *both* the tmux format order and
/// the positional parse order; do not reorder without understanding that both
/// derivations depend on it.
const PANE_FIELDS: &[PaneField] = &[
    entry(SessionName, Core, "#{session_name}", false),
    entry(WindowIndex, Core, "#{window_index}", false),
    entry(PaneIndex, Core, "#{pane_index}", false),
    entry(PaneId, Core, "#{pane_id}", true),
    entry(PanePid, Core, "#{pane_pid}", false),
    entry(PaneCurrentCommand, Core, "#{pane_current_command}", true),
    entry(PaneTitle, Core, "#{pane_title}", true),
    entry(PaneTty, Core, "#{pane_tty}", false),
    entry(PaneCurrentPath, Core, "#{pane_current_path}", false),
    entry(WindowName, Core, "#{window_name}", false),
    entry(SessionId, Ids, "#{session_id}", false),
    entry(WindowId, Ids, "#{window_id}", false),
    entry(AgentProvider, Agent, "#{@agent.provider}", true),
    entry(AgentLabel, Agent, "#{@agent.label}", true),
    entry(AgentCwd, Agent, "#{@agent.cwd}", true),
    entry(AgentState, Agent, "#{@agent.state}", true),
    entry(AgentSessionId, Agent, "#{@agent.session_id}", true),
    entry(AgentPid, Agent, "#{@agent.pid}", true),
    entry(AgentVersion, Agent, "#{@agent.v}", true),
    entry(AgentModel, Agent, "#{@agent.model}", true),
    entry(PaneActive, Active, "#{pane_active}", true),
    entry(WindowActive, Active, "#{window_active}", true),
];

// --- Block lengths, derived by counting the table ---------------------------

const fn block_len(block: PaneFieldBlock) -> usize {
    let mut count = 0;
    let mut i = 0;
    while i < PANE_FIELDS.len() {
        if PANE_FIELDS[i].block as u8 == block as u8 {
            count += 1;
        }
        i += 1;
    }
    count
}

pub(crate) const CORE_FIELD_COUNT: usize = block_len(Core);
pub(crate) const IDS_FIELD_COUNT: usize = block_len(Ids);
pub(crate) const AGENT_FIELD_COUNT: usize = block_len(Agent);
pub(crate) const ACTIVE_FIELD_COUNT: usize = block_len(Active);

// --- Accepted row widths ----------------------------------------------------

// One width per (ids present?, agent present?) combination. Older PANE_FORMAT
// schema versions omitted the ids and/or `@agent` blocks (the parser tests still
// exercise those narrower shapes), so the parser stays backward compatible with
// every combination rather than hard-failing them. The active flags are always
// the trailing fields, so each width simply grows by ACTIVE_FIELD_COUNT and the
// ids/agent offsets are unchanged from the pre-active layout.
pub(crate) const PANE_ROW_MINIMAL: usize = CORE_FIELD_COUNT + ACTIVE_FIELD_COUNT;
pub(crate) const PANE_ROW_WITH_IDS: usize = CORE_FIELD_COUNT + IDS_FIELD_COUNT + ACTIVE_FIELD_COUNT;
pub(crate) const PANE_ROW_WITH_AGENT: usize =
    CORE_FIELD_COUNT + AGENT_FIELD_COUNT + ACTIVE_FIELD_COUNT;
pub(crate) const PANE_ROW_FULL: usize =
    CORE_FIELD_COUNT + IDS_FIELD_COUNT + AGENT_FIELD_COUNT + ACTIVE_FIELD_COUNT;

// --- Named field indices / offsets, derived from the table ------------------

const fn field_index(id: PaneFieldId) -> usize {
    let mut i = 0;
    while i < PANE_FIELDS.len() {
        if PANE_FIELDS[i].id as u8 == id as u8 {
            return i;
        }
        i += 1;
    }
    panic!("pane field not present in PANE_FIELDS table")
}

// Core fields sit at fixed positions at the front of every row.
pub(crate) const IDX_SESSION_NAME: usize = field_index(SessionName);
pub(crate) const IDX_WINDOW_INDEX: usize = field_index(WindowIndex);
pub(crate) const IDX_PANE_INDEX: usize = field_index(PaneIndex);
pub(crate) const IDX_PANE_ID: usize = field_index(PaneId);
pub(crate) const IDX_PANE_PID: usize = field_index(PanePid);
pub(crate) const IDX_PANE_CURRENT_COMMAND: usize = field_index(PaneCurrentCommand);
pub(crate) const IDX_PANE_TITLE: usize = field_index(PaneTitle);
pub(crate) const IDX_PANE_TTY: usize = field_index(PaneTty);
pub(crate) const IDX_PANE_CURRENT_PATH: usize = field_index(PaneCurrentPath);
pub(crate) const IDX_WINDOW_NAME: usize = field_index(WindowName);

// The ids block always sits immediately after the core block (only the agent
// and active blocks can follow it), so its indices never shift.
pub(crate) const IDX_SESSION_ID: usize = field_index(SessionId);
pub(crate) const IDX_WINDOW_ID: usize = field_index(WindowId);

// Agent and active blocks can shift left when an earlier optional block is
// absent, so the parser reads them relative to a computed block start.
const fn offset_within_block(id: PaneFieldId, first: PaneFieldId) -> usize {
    field_index(id) - field_index(first)
}

pub(crate) const AGENT_PROVIDER_OFFSET: usize = offset_within_block(AgentProvider, AgentProvider);
pub(crate) const AGENT_LABEL_OFFSET: usize = offset_within_block(AgentLabel, AgentProvider);
pub(crate) const AGENT_CWD_OFFSET: usize = offset_within_block(AgentCwd, AgentProvider);
pub(crate) const AGENT_STATE_OFFSET: usize = offset_within_block(AgentState, AgentProvider);
pub(crate) const AGENT_SESSION_ID_OFFSET: usize =
    offset_within_block(AgentSessionId, AgentProvider);
pub(crate) const AGENT_PID_OFFSET: usize = offset_within_block(AgentPid, AgentProvider);
pub(crate) const AGENT_VERSION_OFFSET: usize = offset_within_block(AgentVersion, AgentProvider);
pub(crate) const AGENT_MODEL_OFFSET: usize = offset_within_block(AgentModel, AgentProvider);

pub(crate) const ACTIVE_PANE_OFFSET: usize = offset_within_block(PaneActive, PaneActive);
pub(crate) const ACTIVE_WINDOW_OFFSET: usize = offset_within_block(WindowActive, PaneActive);

// --- Derived format strings -------------------------------------------------

const fn copy_into(out: &mut [u8], mut pos: usize, src: &[u8]) -> usize {
    let mut i = 0;
    while i < src.len() {
        out[pos] = src[i];
        pos += 1;
        i += 1;
    }
    pos
}

const fn pane_format_len() -> usize {
    let delim = super::TMUX_FORMAT_DELIM.len();
    let mut len = 0;
    let mut i = 0;
    while i < PANE_FIELDS.len() {
        if i > 0 {
            len += delim;
        }
        len += PANE_FIELDS[i].directive.len();
        i += 1;
    }
    len
}

const PANE_FORMAT_BYTES: [u8; pane_format_len()] = {
    let delim = super::TMUX_FORMAT_DELIM.as_bytes();
    let mut out = [0u8; pane_format_len()];
    let mut pos = 0;
    let mut i = 0;
    while i < PANE_FIELDS.len() {
        if i > 0 {
            pos = copy_into(&mut out, pos, delim);
        }
        pos = copy_into(&mut out, pos, PANE_FIELDS[i].directive.as_bytes());
        i += 1;
    }
    out
};

/// The `list-panes -F` format string. Fields are joined with the escaped `\037`
/// unit separator that the parser splits on.
pub(crate) const PANE_FORMAT: &str = match std::str::from_utf8(&PANE_FORMAT_BYTES) {
    Ok(value) => value,
    Err(_) => panic!("PANE_FORMAT is not valid UTF-8"),
};

// The subscription payload is sent to tmux verbatim (inserted as a `writeln!`
// named argument, not reprocessed), so the format directives use single braces
// `#{...}` exactly as tmux expects. Doubling them produced a subscription whose
// every field rendered as a literal `}`, so the payload was constant and
// `%subscription-changed` never fired on real field changes — detection silently
// relied on the reconcile poll and on `%output`/`%window-renamed` notifications
// instead. Deriving the string from the shared directives keeps it single-brace
// by construction; a unit test still guards against `#{{` regressing.
const SUBSCRIPTION_PREFIX: &str = "agentscan:%*:";
const SUBSCRIPTION_DELIM: &str = ":";

const fn subscription_format_len() -> usize {
    let mut len = SUBSCRIPTION_PREFIX.len();
    let delim = SUBSCRIPTION_DELIM.len();
    let mut count = 0;
    let mut i = 0;
    while i < PANE_FIELDS.len() {
        if PANE_FIELDS[i].in_subscription {
            if count > 0 {
                len += delim;
            }
            len += PANE_FIELDS[i].directive.len();
            count += 1;
        }
        i += 1;
    }
    len
}

const DAEMON_SUBSCRIPTION_FORMAT_BYTES: [u8; subscription_format_len()] = {
    let mut out = [0u8; subscription_format_len()];
    let mut pos = copy_into(&mut out, 0, SUBSCRIPTION_PREFIX.as_bytes());
    let delim = SUBSCRIPTION_DELIM.as_bytes();
    let mut count = 0;
    let mut i = 0;
    while i < PANE_FIELDS.len() {
        if PANE_FIELDS[i].in_subscription {
            if count > 0 {
                pos = copy_into(&mut out, pos, delim);
            }
            pos = copy_into(&mut out, pos, PANE_FIELDS[i].directive.as_bytes());
            count += 1;
        }
        i += 1;
    }
    out
};

/// The control-mode `refresh-client -B` subscription payload for identity and
/// wrapper-metadata changes.
pub(crate) const DAEMON_SUBSCRIPTION_FORMAT: &str =
    match std::str::from_utf8(&DAEMON_SUBSCRIPTION_FORMAT_BYTES) {
        Ok(value) => value,
        Err(_) => panic!("DAEMON_SUBSCRIPTION_FORMAT is not valid UTF-8"),
    };
