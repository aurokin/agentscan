use super::*;

pub(crate) fn tmux_command() -> Command {
    let mut command = Command::new("tmux");
    if !env_has_utf8_locale(|name| env::var(name).ok()) {
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
    tmux_command()
        .args(args)
        .output()
        .with_context(|| format!("failed to execute {context}"))
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

#[cfg(test)]
mod tests {
    use super::env_has_utf8_locale;

    fn read_from<'a>(entries: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            entries
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_string())
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
}
