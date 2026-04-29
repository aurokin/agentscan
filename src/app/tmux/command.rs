use super::*;

pub(super) fn run_tmux_output(args: &[&str], context: &str) -> Result<std::process::Output> {
    Command::new("tmux")
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
