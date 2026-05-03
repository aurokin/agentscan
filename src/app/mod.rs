use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use clap::Parser;
use serde::Serialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

pub mod bench_support;
mod cache;
mod classify;
mod cli;
mod commands;
mod daemon;
mod ipc;
mod model;
mod output;
mod path;
mod proc;
mod provider;
mod scanner;
#[cfg(test)]
mod tests;
mod tmux;
mod tui;

pub(crate) use cli::*;
pub use commands::run;
pub(crate) use model::*;
pub(crate) use path::*;
pub(crate) use provider::*;

const PANE_DELIM: &str = "\u{001f}";
const TMUX_FORMAT_DELIM: &str = r"\037";
const CACHE_ENV_VAR: &str = "AGENTSCAN_CACHE_PATH";
const TMUX_SOCKET_ENV_VAR: &str = "AGENTSCAN_TMUX_SOCKET";
const CACHE_RELATIVE_PATH: &str = "agentscan/cache-v1.json";
const CACHE_SCHEMA_VERSION: u32 = 4;
static CACHE_WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
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
    "#{@agent.session_id}"
);
const DAEMON_SUBSCRIPTION_FORMAT: &str = concat!(
    "agentscan:%*:",
    "#{{pane_id}}:",
    "#{{pane_current_command}}:",
    "#{{pane_title}}:",
    "#{{@agent.provider}}:",
    "#{{@agent.label}}:",
    "#{{@agent.cwd}}:",
    "#{{@agent.state}}:",
    "#{{@agent.session_id}}"
);

fn default_if_empty<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.trim().is_empty() {
        fallback
    } else {
        value
    }
}
