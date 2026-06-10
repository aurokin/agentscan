use super::*;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

// The last compatible tmux install resolved after a fresh client was dropped
// mid-handshake (see `tmux_dropped_fresh_client`). `Some` reroutes every later
// tmux exec — including the daemon's control-mode attach, which always follows
// a sync command like list-sessions that triggers the resolution — through the
// install that proved it can talk to the running server. Not a once-per-process
// cache: when the selected install is itself dropped (the server moved to yet
// another install mid-process), `refresh_compatible_tmux` re-resolves, so a
// long-lived daemon heals without a restart.
static COMPATIBLE_TMUX: Mutex<Option<PathBuf>> = Mutex::new(None);

fn compatible_tmux_cache() -> MutexGuard<'static, Option<PathBuf>> {
    COMPATIBLE_TMUX
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn tmux_command() -> Command {
    tmux_command_from_env(|name| env::var_os(name), |name| env::var(name).ok())
}

fn tmux_command_from_env(
    read_os: impl Fn(&str) -> Option<std::ffi::OsString>,
    read_string: impl Fn(&str) -> Option<String>,
) -> Command {
    // Program precedence: the user's explicit pin, then a compatible install
    // resolved after a dropped handshake, then plain `tmux` from PATH.
    let program = read_os(TMUX_BIN_ENV_VAR)
        .filter(|program| !program.is_empty())
        .or_else(|| compatible_tmux_cache().clone().map(PathBuf::into_os_string))
        .unwrap_or_else(|| std::ffi::OsString::from("tmux"));
    tmux_command_with_program(program, read_os, read_string)
}

fn tmux_command_with_program(
    program: std::ffi::OsString,
    read_os: impl Fn(&str) -> Option<std::ffi::OsString>,
    read_string: impl Fn(&str) -> Option<String>,
) -> Command {
    let mut command = Command::new(program);
    if let Some(socket_path) = read_os(TMUX_SOCKET_ENV_VAR).filter(|path| !path.is_empty()) {
        command.arg("-S").arg(socket_path);
        command.env_remove("TMUX");
    }
    if !env_has_utf8_locale(read_string) {
        command.env("LANG", "en_US.UTF-8");
    }
    command
}

pub(super) fn env_has_utf8_locale(read: impl Fn(&str) -> Option<String>) -> bool {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .find_map(|name| read(name).filter(|value| !value.is_empty()))
        .is_some_and(|value| {
            let normalized = value.replace('-', "").to_ascii_uppercase();
            normalized.contains("UTF8")
        })
}

pub(super) fn run_tmux_output(args: &[&str], context: &str) -> Result<std::process::Output> {
    let mut command = tmux_command();
    let program_used = command.get_program().to_os_string();
    let output = command
        .args(args)
        .output()
        .with_context(|| format!("failed to execute {context}"))?;
    if output.status.success()
        || !tmux_dropped_fresh_client(&String::from_utf8_lossy(&output.stderr))
    {
        return Ok(output);
    }

    // A dropped handshake usually means THIS tmux install's version differs
    // from the running server's (verified case: a linuxbrew 3.6b server
    // dropping the apt 3.4 client that non-interactive SSH resolves). Resolve
    // a compatible install — each candidate must complete a real handshake
    // against the same socket to win — and retry once. An explicit
    // AGENTSCAN_TMUX_BIN pin is honored as-is: the user chose it, so no
    // auto-resolution overrides it.
    if env::var_os(TMUX_BIN_ENV_VAR).is_some_and(|pin| !pin.is_empty()) {
        return Ok(output);
    }
    let Some(resolved) = refresh_compatible_tmux(&program_used) else {
        return Ok(output);
    };
    if resolved.as_os_str() == program_used {
        return Ok(output);
    }
    tmux_command()
        .args(args)
        .output()
        .with_context(|| format!("failed to execute {context} (compatible-tmux retry)"))
}

// The tmux client's words for "the server closed my connection during the
// handshake": a fresh client of a mismatched version gets dropped this way by
// a healthy server (sometimes without even a version reply), while already-
// connected clients keep working. Distinct from "no server running" (nothing
// to talk to) and from missing-target errors (the handshake succeeded).
pub(super) fn tmux_dropped_fresh_client(stderr: &str) -> bool {
    stderr.contains("server exited unexpectedly")
        || stderr.contains("lost server")
        || stderr.contains("protocol version mismatch")
}

// Probe candidates only when the dropped install is the one the cache (or
// PATH) already selected; when another thread just resolved a different
// install, reuse it without re-probing. Holding the lock across the sweep
// serializes concurrent failers so each refresh probes once. Re-probing on
// every dropped command (rather than once per process) is the deliberate
// cost: it only happens on commands that already failed, the sweep is a
// handful of spawns at most, and it is what lets a long-lived daemon follow
// a server that moves between installs.
fn refresh_compatible_tmux(program_used: &std::ffi::OsStr) -> Option<PathBuf> {
    let mut cache = compatible_tmux_cache();
    refresh_compatible_tmux_with(&mut cache, program_used, || {
        resolve_compatible_tmux(&well_known_tmux_installs(), candidate_speaks_to_server)
    })
}

fn refresh_compatible_tmux_with(
    cache: &mut Option<PathBuf>,
    program_used: &std::ffi::OsStr,
    resolve: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    let cached_is_fresh = cache
        .as_ref()
        .is_some_and(|cached| cached.as_os_str() != program_used);
    if !cached_is_fresh {
        *cache = resolve();
    }
    cache.clone()
}

// First candidate that completes a handshake with the running server wins.
// Validation is what keeps false positives out: a candidate only wins by
// succeeding against the real socket, and the install that just failed simply
// fails its probe again.
pub(super) fn resolve_compatible_tmux(
    candidates: &[PathBuf],
    speaks_to_server: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    candidates
        .iter()
        .find(|candidate| speaks_to_server(candidate))
        .cloned()
}

// Where package managers put tmux: linuxbrew (system-wide and per-user
// prefixes), macOS Homebrew (arm and intel), MacPorts, and the system
// package manager. Version splits arise when a server was started from one
// of these while PATH resolves another.
fn well_known_tmux_installs() -> Vec<PathBuf> {
    let mut candidates = vec![PathBuf::from("/home/linuxbrew/.linuxbrew/bin/tmux")];
    if let Some(home) = env::var_os("HOME").filter(|home| !home.is_empty()) {
        candidates.push(PathBuf::from(home).join(".linuxbrew/bin/tmux"));
    }
    candidates.extend([
        PathBuf::from("/opt/homebrew/bin/tmux"),
        PathBuf::from("/usr/local/bin/tmux"),
        PathBuf::from("/opt/local/bin/tmux"),
        PathBuf::from("/usr/bin/tmux"),
    ]);
    candidates.retain(|candidate| candidate.is_file());
    candidates
}

// Read-only handshake probe: `display-message -p` completes the client/server
// handshake and prints to stdout without touching any session, window, or
// client. Built directly (not via run_tmux_output) so probing can't recurse
// into resolution.
fn candidate_speaks_to_server(candidate: &Path) -> bool {
    tmux_command_with_program(
        candidate.as_os_str().to_os_string(),
        |name| env::var_os(name),
        |name| env::var(name).ok(),
    )
    .args(["display-message", "-p", "agentscan-compat-probe"])
    .output()
    .map(|output| output.status.success())
    .unwrap_or(false)
}

pub(super) fn run_tmux_text_output(
    args: &[&str],
    context: &str,
    failure_context: &str,
    missing_target: impl Fn(&str) -> bool,
    utf8_context: &'static str,
) -> Result<Option<String>> {
    let output = run_tmux_output(args, context)?;
    if !output.status.success() {
        let stderr = tmux_stderr(&output);
        if missing_target(&stderr) {
            return Ok(None);
        }
        if stderr.is_empty() {
            bail!("{failure_context} failed with status {}", output.status);
        }
        bail!("{failure_context} failed: {stderr}");
    }

    String::from_utf8(output.stdout)
        .map(Some)
        .context(utf8_context)
}

pub(super) fn run_tmux_status(args: &[&str], context: &str, failure_context: &str) -> Result<()> {
    let output = run_tmux_output(args, context)?;
    if output.status.success() {
        return Ok(());
    }

    let stderr = tmux_stderr(&output);
    if stderr.is_empty() {
        bail!("{failure_context} failed with status {}", output.status);
    }
    bail!("{failure_context} failed: {stderr}");
}

fn tmux_stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_string()
}

// tmux prints "no server running on <socket>" when no server is up, which means
// there are simply zero sessions rather than a failure.
pub(super) fn tmux_no_server_running(stderr: &str) -> bool {
    stderr.contains("no server running")
}

pub(super) fn tmux_scope_target_is_missing(stderr: &str) -> bool {
    tmux_pane_target_is_missing(stderr) || stderr.contains("can't find session")
}

pub(super) fn tmux_pane_target_is_missing(stderr: &str) -> bool {
    stderr.contains("can't find pane") || stderr.contains("can't find window")
}

pub(crate) fn tmux_target_is_missing(stderr: &[u8]) -> bool {
    let stderr = String::from_utf8_lossy(stderr);
    let stderr = stderr.trim();
    stderr.contains("can't find pane") || stderr.contains("can't find window")
}

pub(crate) fn tmux_version() -> Option<String> {
    let stdout = run_tmux_text_output(
        &["-V"],
        "tmux -V",
        "tmux -V",
        |_| false,
        "tmux version output was not valid UTF-8",
    )
    .ok()??;
    stdout
        .trim()
        .strip_prefix("tmux ")
        .map(|version| version.to_string())
        .or_else(|| Some(stdout.trim().to_string()))
}

pub(crate) fn default_session_target() -> Result<String> {
    if env::var_os("TMUX").is_some()
        && let Some(stdout) = run_tmux_text_output(
            &["display-message", "-p", "#{session_id}"],
            "current tmux session",
            "tmux display-message for current session",
            |_| true,
            "current session was not UTF-8",
        )?
    {
        let session = stdout.trim();
        if !session.is_empty() {
            return Ok(session.to_string());
        }
    }

    let stdout = run_tmux_text_output(
        &["list-sessions", "-F", "#{session_id}"],
        "tmux list-sessions",
        "tmux list-sessions",
        |_| false,
        "tmux sessions output was not UTF-8",
    )?
    .context("tmux list-sessions unexpectedly returned no output")?;
    let session = stdout
        .lines()
        .find(|line| !line.trim().is_empty())
        .context("no tmux sessions available for daemon attach")?;
    Ok(session.trim().to_string())
}

pub(crate) fn list_session_ids() -> Result<Vec<String>> {
    let Some(stdout) = run_tmux_text_output(
        &["list-sessions", "-F", "#{session_id}"],
        "tmux list-sessions",
        "tmux list-sessions",
        // Only "no server running" legitimately means zero sessions. Any other
        // failure is a real error and must propagate so the caller keeps the
        // current subscriber set rather than dropping all subscribers and
        // mis-marking coverage as complete.
        tmux_no_server_running,
        "tmux sessions output was not UTF-8",
    )?
    else {
        return Ok(Vec::new());
    };
    Ok(stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{
        env_has_utf8_locale, refresh_compatible_tmux_with, resolve_compatible_tmux,
        tmux_command_from_env, tmux_dropped_fresh_client, tmux_no_server_running,
    };
    use crate::app::{TMUX_BIN_ENV_VAR, TMUX_SOCKET_ENV_VAR};
    use std::ffi::OsString;
    use std::path::PathBuf;

    #[test]
    fn dropped_fresh_client_matches_only_handshake_failures() {
        assert!(tmux_dropped_fresh_client("server exited unexpectedly"));
        assert!(tmux_dropped_fresh_client("lost server"));
        assert!(tmux_dropped_fresh_client(
            "protocol version mismatch (client 8, server 9)"
        ));
        // Not a handshake drop: nothing to talk to, or the handshake succeeded
        // and the target was simply missing.
        assert!(!tmux_dropped_fresh_client(
            "no server running on /tmp/tmux-501/default"
        ));
        assert!(!tmux_dropped_fresh_client("can't find pane: %42"));
        assert!(!tmux_dropped_fresh_client(""));
    }

    #[test]
    fn refresh_resolves_when_the_cache_is_empty() {
        let mut cache = None;
        let resolved =
            refresh_compatible_tmux_with(&mut cache, OsString::from("tmux").as_os_str(), || {
                Some(PathBuf::from("/fake/working/tmux"))
            });
        assert_eq!(resolved, Some(PathBuf::from("/fake/working/tmux")));
        assert_eq!(cache, Some(PathBuf::from("/fake/working/tmux")));
    }

    #[test]
    fn refresh_reresolves_when_the_cached_install_is_the_one_that_was_dropped() {
        // The server moved to yet another install mid-process: the cached
        // binary itself got dropped, so the cache must not be trusted.
        let mut cache = Some(PathBuf::from("/fake/stale/tmux"));
        let resolved = refresh_compatible_tmux_with(
            &mut cache,
            OsString::from("/fake/stale/tmux").as_os_str(),
            || Some(PathBuf::from("/fake/new/tmux")),
        );
        assert_eq!(resolved, Some(PathBuf::from("/fake/new/tmux")));
        assert_eq!(cache, Some(PathBuf::from("/fake/new/tmux")));
    }

    #[test]
    fn refresh_clears_the_cache_when_no_candidate_handshakes() {
        let mut cache = Some(PathBuf::from("/fake/stale/tmux"));
        let resolved = refresh_compatible_tmux_with(
            &mut cache,
            OsString::from("/fake/stale/tmux").as_os_str(),
            || None,
        );
        assert_eq!(resolved, None);
        assert_eq!(cache, None);
    }

    #[test]
    fn refresh_reuses_a_cached_install_that_was_not_the_dropped_one() {
        // Another thread already resolved a different install; reuse it for
        // the retry instead of probing again.
        let mut cache = Some(PathBuf::from("/fake/working/tmux"));
        let resolved =
            refresh_compatible_tmux_with(&mut cache, OsString::from("tmux").as_os_str(), || {
                panic!("a fresh cache entry must be reused without re-probing")
            });
        assert_eq!(resolved, Some(PathBuf::from("/fake/working/tmux")));
        assert_eq!(cache, Some(PathBuf::from("/fake/working/tmux")));
    }

    #[test]
    fn compatible_tmux_resolution_picks_the_first_candidate_that_handshakes() {
        let candidates = [
            PathBuf::from("/fake/broken/tmux"),
            PathBuf::from("/fake/working/tmux"),
            PathBuf::from("/fake/other/tmux"),
        ];
        let resolved = resolve_compatible_tmux(&candidates, |candidate| {
            candidate == PathBuf::from("/fake/working/tmux").as_path()
        });
        assert_eq!(resolved, Some(PathBuf::from("/fake/working/tmux")));

        // No candidate speaks to the server: resolution yields nothing and the
        // original failure stands.
        assert_eq!(resolve_compatible_tmux(&candidates, |_| false), None);
        assert_eq!(resolve_compatible_tmux(&[], |_| true), None);
    }

    #[test]
    fn explicit_tmux_bin_pin_overrides_the_program() {
        let command = tmux_command_from_env(
            read_os_from(&[(TMUX_BIN_ENV_VAR, "/custom/bin/tmux")]),
            read_from(&[("LANG", "en_US.UTF-8")]),
        );
        assert_eq!(command.get_program(), "/custom/bin/tmux");
    }

    #[test]
    fn empty_tmux_bin_pin_falls_back_to_path_resolution() {
        let command = tmux_command_from_env(
            read_os_from(&[(TMUX_BIN_ENV_VAR, "")]),
            read_from(&[("LANG", "en_US.UTF-8")]),
        );
        assert_eq!(command.get_program(), "tmux");
    }

    #[test]
    fn no_server_running_matches_only_the_empty_server_case() {
        assert!(tmux_no_server_running(
            "no server running on /tmp/tmux-501/default"
        ));
        // Real failures must not be mistaken for "zero sessions".
        assert!(!tmux_no_server_running("error connecting to server"));
        assert!(!tmux_no_server_running("can't find session: foo"));
        assert!(!tmux_no_server_running(""));
    }

    fn read_from<'a>(entries: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            entries
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_string())
        }
    }

    fn read_os_from<'a>(
        entries: &'a [(&'a str, &'a str)],
    ) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| {
            entries
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    #[test]
    fn empty_env_is_not_utf8() {
        assert!(!env_has_utf8_locale(read_from(&[])));
    }

    #[test]
    fn lang_with_utf8_is_recognised() {
        assert!(env_has_utf8_locale(read_from(&[("LANG", "en_US.UTF-8")])));
    }

    #[test]
    fn lc_ctype_with_lowercase_utf8_is_recognised() {
        assert!(env_has_utf8_locale(read_from(&[(
            "LC_CTYPE",
            "en_US.utf-8"
        )])));
    }

    #[test]
    fn dashless_utf8_form_is_recognised() {
        assert!(env_has_utf8_locale(read_from(&[("LANG", "C.UTF8")])));
        assert!(env_has_utf8_locale(read_from(&[(
            "LC_CTYPE",
            "en_US.utf8"
        )])));
    }

    #[test]
    fn lc_all_overrides_other_vars() {
        assert!(env_has_utf8_locale(read_from(&[
            ("LC_ALL", "C.UTF-8"),
            ("LANG", "POSIX"),
        ])));
    }

    #[test]
    fn lc_all_takes_precedence_over_utf8_lang() {
        assert!(!env_has_utf8_locale(read_from(&[
            ("LC_ALL", "C"),
            ("LANG", "en_US.UTF-8"),
        ])));
    }

    #[test]
    fn lc_ctype_takes_precedence_over_lang_when_lc_all_unset() {
        assert!(!env_has_utf8_locale(read_from(&[
            ("LC_CTYPE", "C"),
            ("LANG", "en_US.UTF-8"),
        ])));
    }

    #[test]
    fn empty_lc_all_falls_through_to_lower_priority_var() {
        assert!(env_has_utf8_locale(read_from(&[
            ("LC_ALL", ""),
            ("LANG", "en_US.UTF-8"),
        ])));
    }

    #[test]
    fn non_utf8_locale_is_not_recognised() {
        assert!(!env_has_utf8_locale(read_from(&[("LANG", "POSIX")])));
        assert!(!env_has_utf8_locale(read_from(&[("LC_ALL", "C")])));
    }

    #[test]
    fn empty_value_is_ignored() {
        assert!(!env_has_utf8_locale(read_from(&[("LANG", "")])));
    }

    #[test]
    fn explicit_agentscan_tmux_socket_adds_socket_arg_and_removes_tmux_env() {
        let command = tmux_command_from_env(
            read_os_from(&[
                (TMUX_SOCKET_ENV_VAR, "/tmp/agentscan-tmux.sock"),
                ("TMUX", "/tmp/other.sock,1,0"),
            ]),
            read_from(&[("LANG", "en_US.UTF-8")]),
        );
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert_eq!(args, vec!["-S", "/tmp/agentscan-tmux.sock"]);
        assert!(
            command
                .get_envs()
                .any(|(name, value)| name == "TMUX" && value.is_none()),
            "explicit tmux socket should remove inherited TMUX for child commands"
        );
    }

    #[test]
    fn empty_agentscan_tmux_socket_is_ignored() {
        let command = tmux_command_from_env(
            read_os_from(&[(TMUX_SOCKET_ENV_VAR, "")]),
            read_from(&[("LANG", "en_US.UTF-8")]),
        );

        assert_eq!(command.get_args().count(), 0);
    }
}
