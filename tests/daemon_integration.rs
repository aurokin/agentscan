use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tempfile::TempDir;

const DAEMON_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

#[test]
fn daemon_updates_cache_when_titles_change() -> Result<()> {
    let harness = TestHarness::new()?;
    let pane_id = harness.start_session("title-updates", "sh")?;
    let mut daemon = harness.start_daemon()?;

    harness.wait_for_pane(&mut daemon, &pane_id, |_| true)?;

    harness.send_title_escape(&pane_id, "Claude Code | Working")?;
    harness.wait_for_pane(&mut daemon, &pane_id, |pane| {
        pane["provider"] == "claude"
            && pane["status"]["kind"] == "busy"
            && pane["display"]["label"] == "Working"
    })?;

    harness.send_title_escape(&pane_id, "Claude Code | Ready")?;
    harness.wait_for_pane(&mut daemon, &pane_id, |pane| {
        pane["provider"] == "claude"
            && pane["status"]["kind"] == "idle"
            && pane["display"]["label"] == "Ready"
    })?;

    daemon.shutdown()?;
    Ok(())
}

#[test]
fn daemon_updates_cache_when_metadata_changes() -> Result<()> {
    let harness = TestHarness::new()?;
    let pane_id = harness.start_session("metadata-updates", "sh")?;
    harness.send_title_escape(&pane_id, "metadata-updates")?;
    let mut daemon = harness.start_daemon()?;

    harness.wait_for_pane(&mut daemon, &pane_id, |_| true)?;

    harness.agentscan([
        "tmux",
        "set-metadata",
        "--pane-id",
        &pane_id,
        "--provider",
        "codex",
        "--label",
        "Wrapper Task",
        "--state",
        "busy",
    ])?;
    harness.wait_for_pane(&mut daemon, &pane_id, |pane| {
        pane["provider"] == "codex"
            && pane["display"]["label"] == "Wrapper Task"
            && pane["status"]["kind"] == "busy"
            && pane["status"]["source"] == "pane_metadata"
    })?;

    harness.agentscan([
        "tmux",
        "clear-metadata",
        "--pane-id",
        &pane_id,
        "--field",
        "provider",
        "--field",
        "label",
        "--field",
        "state",
    ])?;
    harness.wait_for_pane(&mut daemon, &pane_id, |pane| {
        pane["provider"].is_null()
            && pane["display"]["label"] == "metadata-updates"
            && pane["status"]["kind"] == "unknown"
    })?;

    daemon.shutdown()?;
    Ok(())
}

#[test]
fn daemon_updates_cache_when_panes_are_added() -> Result<()> {
    let harness = TestHarness::new()?;
    let root_pane_id = harness.start_session("pane-add", "sh")?;
    let mut daemon = harness.start_daemon()?;

    harness.wait_for_pane(&mut daemon, &root_pane_id, |_| true)?;

    let split_pane_id = harness.split_window("pane-add:0.0", "sleep 300")?;
    harness.wait_for_pane(&mut daemon, &split_pane_id, |pane| {
        pane["pane_id"].as_str() == Some(split_pane_id.as_str())
    })?;

    daemon.shutdown()?;
    Ok(())
}

#[test]
fn daemon_updates_cache_when_panes_are_removed() -> Result<()> {
    let harness = TestHarness::new()?;
    let root_pane_id = harness.start_session("pane-remove", "sh")?;
    let split_pane_id = harness.split_window("pane-remove:0.0", "sleep 300")?;
    let mut daemon = harness.start_daemon()?;

    harness.wait_for_pane(&mut daemon, &root_pane_id, |_| true)?;
    harness.wait_for_pane(&mut daemon, &split_pane_id, |_| true)?;

    harness.tmux(["kill-pane", "-t", &split_pane_id])?;
    harness.wait_for_cache(&mut daemon, |cache| {
        pane_from_cache(cache, &split_pane_id).is_none()
    })?;

    daemon.shutdown()?;
    Ok(())
}

#[test]
fn daemon_survives_when_attached_session_is_removed_but_server_remains() -> Result<()> {
    let harness = TestHarness::new()?;
    let attached_pane_id = harness.start_session("attached-session", "sleep 300")?;
    let surviving_pane_id = harness.start_session("surviving-session", "sleep 300")?;
    let mut daemon = harness.start_daemon()?;

    harness.wait_for_pane(&mut daemon, &attached_pane_id, |_| true)?;
    harness.wait_for_pane(&mut daemon, &surviving_pane_id, |_| true)?;

    harness.tmux(["kill-session", "-t", "attached-session"])?;
    harness.wait_for_pane(&mut daemon, &surviving_pane_id, |_| true)?;

    daemon.shutdown()?;
    Ok(())
}

#[test]
fn daemon_updates_cache_when_sessions_are_added_and_removed() -> Result<()> {
    let harness = TestHarness::new()?;
    let root_pane_id = harness.start_session("session-root", "sleep 300")?;
    let mut daemon = harness.start_daemon()?;

    harness.wait_for_pane(&mut daemon, &root_pane_id, |_| true)?;

    let added_pane_id = harness.start_session("session-added", "sleep 300")?;
    harness.wait_for_pane(&mut daemon, &added_pane_id, |_| true)?;

    harness.tmux(["kill-session", "-t", "session-added"])?;
    harness.wait_for_cache(&mut daemon, |cache| {
        pane_from_cache(cache, &added_pane_id).is_none()
    })?;

    daemon.shutdown()?;
    Ok(())
}

#[test]
fn daemon_exits_when_tmux_server_disappears() -> Result<()> {
    let harness = TestHarness::new()?;
    let _pane_id = harness.start_session("server-exit", "sleep 300")?;
    let mut daemon = harness.start_daemon()?;

    harness.wait_for_cache(&mut daemon, |_| true)?;
    harness.tmux(["kill-server"])?;
    daemon.wait_for_exit(DAEMON_TIMEOUT)?;

    Ok(())
}

struct TestHarness {
    _tempdir: TempDir,
    tmux_tmpdir: PathBuf,
    cache_path: PathBuf,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

impl TestHarness {
    fn new() -> Result<Self> {
        let tempdir = tempfile::tempdir().context("failed to create tempdir")?;
        let tmux_tmpdir = tempdir.path().join("tmux");
        fs::create_dir_all(&tmux_tmpdir)
            .with_context(|| format!("failed to create {}", tmux_tmpdir.display()))?;

        Ok(Self {
            cache_path: tempdir.path().join("cache.json"),
            stdout_path: tempdir.path().join("daemon.stdout.log"),
            stderr_path: tempdir.path().join("daemon.stderr.log"),
            tmux_tmpdir,
            _tempdir: tempdir,
        })
    }

    fn start_session(&self, session_name: &str, command: &str) -> Result<String> {
        let status = self
            .tmux_command()
            .arg("-f")
            .arg("/dev/null")
            .args(["new-session", "-d", "-s", session_name, command])
            .status()
            .with_context(|| format!("failed to start tmux session {session_name}"))?;
        if !status.success() {
            bail!("tmux new-session failed for {session_name} with status {status}");
        }

        let output = self.tmux_output([
            "display-message",
            "-p",
            "-t",
            &format!("{session_name}:0.0"),
            "#{pane_id}",
        ])?;
        Ok(output.trim().to_string())
    }

    fn split_window(&self, target: &str, command: &str) -> Result<String> {
        let output = self.tmux_output([
            "split-window",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-t",
            target,
            command,
        ])?;
        Ok(output.trim().to_string())
    }

    fn start_daemon(&self) -> Result<DaemonHandle> {
        let stdout = fs::File::create(&self.stdout_path)
            .with_context(|| format!("failed to create {}", self.stdout_path.display()))?;
        let stderr = fs::File::create(&self.stderr_path)
            .with_context(|| format!("failed to create {}", self.stderr_path.display()))?;

        let child = Command::new(agentscan_bin()?)
            .args(["daemon", "run"])
            .env_remove("TMUX")
            .env("TMUX_TMPDIR", &self.tmux_tmpdir)
            .env("AGENTSCAN_CACHE_PATH", &self.cache_path)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .context("failed to start agentscan daemon")?;

        Ok(DaemonHandle {
            child,
            stdout_path: self.stdout_path.clone(),
            stderr_path: self.stderr_path.clone(),
        })
    }

    fn send_title_escape(&self, pane_id: &str, title: &str) -> Result<()> {
        self.tmux([
            "send-keys",
            "-t",
            pane_id,
            &format!("printf '\\033]2;{title}\\033\\\\'"),
            "Enter",
        ])
    }

    fn agentscan<I, S>(&self, args: I) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut command = Command::new(agentscan_bin()?);
        command.env_remove("TMUX");
        command.env("TMUX_TMPDIR", &self.tmux_tmpdir);
        command.env("AGENTSCAN_CACHE_PATH", &self.cache_path);
        for arg in args {
            command.arg(arg.as_ref());
        }

        let output = command
            .output()
            .context("failed to execute agentscan command")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("agentscan command failed: {}", stderr.trim());
        }

        Ok(())
    }

    fn tmux<I, S>(&self, args: I) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut command = self.tmux_command();
        for arg in args {
            command.arg(arg.as_ref());
        }

        let status = command.status().context("failed to execute tmux command")?;
        if !status.success() {
            bail!("tmux command failed with status {status}");
        }

        Ok(())
    }

    fn tmux_output<I, S>(&self, args: I) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut command = self.tmux_command();
        for arg in args {
            command.arg(arg.as_ref());
        }

        let output = command.output().context("failed to execute tmux command")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("tmux command failed: {}", stderr.trim());
        }

        String::from_utf8(output.stdout).context("tmux output was not valid UTF-8")
    }

    fn wait_for_cache<F>(&self, daemon: &mut DaemonHandle, predicate: F) -> Result<Value>
    where
        F: Fn(&Value) -> bool,
    {
        let deadline = Instant::now() + DAEMON_TIMEOUT;
        loop {
            daemon.ensure_running()?;

            if let Ok(contents) = fs::read_to_string(&self.cache_path)
                && let Ok(cache) = serde_json::from_str::<Value>(&contents)
                && predicate(&cache)
            {
                return Ok(cache);
            }

            if Instant::now() >= deadline {
                bail!(
                    "timed out waiting for cache update at {}",
                    self.cache_path.display()
                );
            }

            sleep(POLL_INTERVAL);
        }
    }

    fn wait_for_pane<F>(
        &self,
        daemon: &mut DaemonHandle,
        pane_id: &str,
        predicate: F,
    ) -> Result<Value>
    where
        F: Fn(&Value) -> bool,
    {
        self.wait_for_cache(daemon, |cache| {
            pane_from_cache(cache, pane_id).is_some_and(&predicate)
        })
        .and_then(|cache| {
            pane_from_cache(&cache, pane_id)
                .cloned()
                .with_context(|| format!("pane {pane_id} not found in cache"))
        })
    }

    fn tmux_command(&self) -> Command {
        let mut command = Command::new("tmux");
        command.env_remove("TMUX");
        command.env("TMUX_TMPDIR", &self.tmux_tmpdir);
        command
    }
}

struct DaemonHandle {
    child: Child,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

impl DaemonHandle {
    fn ensure_running(&mut self) -> Result<()> {
        if let Some(status) = self
            .child
            .try_wait()
            .context("failed to poll daemon child")?
        {
            bail!(self.exit_message(status));
        }
        Ok(())
    }

    fn shutdown(&mut self) -> Result<()> {
        if self.child.try_wait()?.is_some() {
            return Ok(());
        }

        let _ = self.child.kill();
        let _ = self.child.wait();
        Ok(())
    }

    fn wait_for_exit(&mut self, timeout: Duration) -> Result<ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self
                .child
                .try_wait()
                .context("failed to poll daemon child")?
            {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                bail!("timed out waiting for daemon to exit");
            }
            sleep(POLL_INTERVAL);
        }
    }

    fn exit_message(&self, status: ExitStatus) -> String {
        let stdout = read_log(&self.stdout_path);
        let stderr = read_log(&self.stderr_path);
        format!(
            "agentscan daemon exited unexpectedly with status {status}\nstdout:\n{stdout}\nstderr:\n{stderr}"
        )
    }
}

impl Drop for DaemonHandle {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn pane_from_cache<'a>(cache: &'a Value, pane_id: &str) -> Option<&'a Value> {
    cache["panes"]
        .as_array()?
        .iter()
        .find(|pane| pane["pane_id"].as_str() == Some(pane_id))
}

fn read_log(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn agentscan_bin() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_agentscan") {
        return Ok(PathBuf::from(path));
    }

    let current_exe = std::env::current_exe().context("failed to resolve current test binary")?;
    let debug_dir = current_exe
        .parent()
        .and_then(Path::parent)
        .context("failed to derive target debug directory")?;
    let candidate = debug_dir.join(format!("agentscan{}", std::env::consts::EXE_SUFFIX));
    if candidate.is_file() {
        return Ok(candidate);
    }

    bail!(
        "failed to find agentscan binary via CARGO_BIN_EXE_agentscan or {}",
        candidate.display()
    )
}
