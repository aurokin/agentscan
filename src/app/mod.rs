use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use clap::Parser;
use serde::Serialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

pub mod bench_support;
mod classify;
mod cli;
mod commands;
mod config;
mod daemon;
mod doctor;
mod ipc;
mod live;
mod model;
mod output;
mod path;
mod picker;
mod proc;
mod provider;
mod scanner;
mod snapshot;
#[cfg(test)]
mod tests;
mod tmux;
mod tui;

pub(crate) use cli::*;
pub use commands::run;
pub(crate) use config::*;
pub(crate) use live::*;
pub(crate) use model::*;
pub(crate) use path::*;
pub(crate) use provider::*;

const PANE_DELIM: &str = "\u{001f}";
const TMUX_FORMAT_DELIM: &str = r"\037";
const TMUX_SOCKET_ENV_VAR: &str = "AGENTSCAN_TMUX_SOCKET";
// Explicit tmux binary override. When set, agentscan execs exactly this tmux
// and never auto-resolves a compatible install (see tmux::command).
const TMUX_BIN_ENV_VAR: &str = "AGENTSCAN_TMUX_BIN";
const CACHE_SCHEMA_VERSION: u32 = 5;
const CLAUDE_SPINNER_GLYPHS: &[char] = &[
    '⠁', '⠂', '⠄', '⡀', '⢀', '⠠', '⠐', '⠈', '⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏', '⣾',
    '⣽', '⣻', '⢿', '⡿', '⣟', '⣯', '⣷',
];
const IDLE_GLYPHS: &[char] = &['✳'];
const PANE_FORMAT: &str = concat!(
    "#{session_name}",
    r"\037",
    "#{window_index}",
    r"\037",
    "#{pane_index}",
    r"\037",
    "#{pane_id}",
    r"\037",
    "#{pane_pid}",
    r"\037",
    "#{pane_current_command}",
    r"\037",
    "#{pane_title}",
    r"\037",
    "#{pane_tty}",
    r"\037",
    "#{pane_current_path}",
    r"\037",
    "#{window_name}",
    r"\037",
    "#{session_id}",
    r"\037",
    "#{window_id}",
    r"\037",
    "#{@agent.provider}",
    r"\037",
    "#{@agent.label}",
    r"\037",
    "#{@agent.cwd}",
    r"\037",
    "#{@agent.state}",
    r"\037",
    "#{@agent.session_id}",
    r"\037",
    "#{pane_active}",
    r"\037",
    "#{window_active}"
);
// This string is sent to tmux verbatim (inserted as a `writeln!` named argument,
// not reprocessed), so the format directives use single braces `#{...}` exactly
// as tmux expects. Doubling them produced a subscription whose every field
// rendered as a literal `}`, so the payload was constant and `%subscription-changed`
// never fired on real field changes — detection silently relied on the reconcile
// poll and on `%output`/`%window-renamed` notifications instead.
const DAEMON_SUBSCRIPTION_FORMAT: &str = concat!(
    "agentscan:%*:",
    "#{pane_id}:",
    "#{pane_current_command}:",
    "#{pane_title}:",
    "#{@agent.provider}:",
    "#{@agent.label}:",
    "#{@agent.cwd}:",
    "#{@agent.state}:",
    "#{@agent.session_id}:",
    "#{pane_active}:",
    "#{window_active}"
);
// Keep output activity separate from identity and metadata changes. `window_activity` is a
// second-resolution timestamp and is the only usable tmux signal for providers whose busy/idle
// state appears solely in captured pane output (including alternate-screen TUIs). It is also
// window-scoped, so every noisy pane produces an activity notification for each quiet sibling.
// A distinct subscription lets the daemon discard those notifications unless the pane's current
// status path can require captured output, without missing agent launches or metadata changes in
// unknown panes. `pane_activity` would be more precise but is not populated by tmux 3.x.
const DAEMON_ACTIVITY_SUBSCRIPTION_FORMAT: &str =
    "agentscan-activity:%*:#{pane_id}:#{window_activity}";

fn default_if_empty<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.trim().is_empty() {
        fallback
    } else {
        value
    }
}
