use std::cell::RefCell;
use std::path::{Path, PathBuf};

use anyhow::Context;
use proptest::{prelude::*, string::string_regex};
use unicode_width::UnicodeWidthStr;

const TMUX_SNAPSHOT_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/tmux_snapshot_titles.txt"
));
const CACHE_SNAPSHOT_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/cache_snapshot_v1.json"
));
const TMUX_METADATA_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/tmux_snapshot_with_metadata.txt"
));
const TMUX_AMBIGUOUS_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/tmux_snapshot_ambiguous.txt"
));

#[allow(unused_imports)]
use super::{
    CACHE_RELATIVE_PATH, CACHE_SCHEMA_VERSION, CLAUDE_SPINNER_GLYPHS, Cli,
    DAEMON_SUBSCRIPTION_FORMAT, IDLE_GLYPHS, OutputFormat, PaneRecord, PaneStatus, Provider,
    SnapshotEnvelope, SourceKind, StatusKind, TmuxMetadataField, cache, classify, daemon, ipc,
    output, proc, tmux,
};

include!("support.rs");
include!("daemon_socket.rs");
include!("classification.rs");
include!("tmux_cache.rs");
include!("tui.rs");
include!("cli.rs");
include!("ipc.rs");
