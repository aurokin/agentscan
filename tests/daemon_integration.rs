use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tempfile::TempDir;

const DAEMON_TIMEOUT: Duration = Duration::from_secs(40);
const POLL_INTERVAL: Duration = Duration::from_millis(50);
const CACHE_SNAPSHOT_FIXTURE: &str = include_str!("fixtures/cache_snapshot_v1.json");
static FAKE_DAEMON_CLEANUP_REQUESTED: AtomicBool = AtomicBool::new(false);

extern "C" fn fake_daemon_sigterm_handler(_signal: libc::c_int) {
    FAKE_DAEMON_CLEANUP_REQUESTED.store(true, Ordering::SeqCst);
}

#[test]
#[ignore]
fn fake_incompatible_daemon_process() -> Result<()> {
    let Some(socket_path) = std::env::var_os("FAKE_DAEMON_SOCKET").map(PathBuf::from) else {
        return Ok(());
    };
    let lock_path = fake_daemon_env_path("FAKE_DAEMON_LOCK")?;
    let identity_path = fake_daemon_env_path("FAKE_DAEMON_IDENTITY")?;
    let ready_pid_path = fake_daemon_env_path("FAKE_DAEMON_READY")?;
    let protocol_version = fake_daemon_env_u32("FAKE_DAEMON_PROTOCOL")?;
    let client_snapshot_schema_version = fake_daemon_env_u32("FAKE_DAEMON_CLIENT_SCHEMA")?;
    let daemon_snapshot_schema_version = fake_daemon_env_u32("FAKE_DAEMON_SCHEMA")?;
    let cleanup_on_sigterm =
        std::env::var("FAKE_DAEMON_CLEANUP_ON_SIGTERM").is_ok_and(|value| value == "1");
    let legacy_identity_sidecar =
        std::env::var("FAKE_DAEMON_LEGACY_IDENTITY").is_ok_and(|value| value == "1");

    let _ = fs::remove_file(&socket_path);
    let mut lock_file = Some(
        fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("failed to open fake daemon lock {}", lock_path.display()))?,
    );
    let result = unsafe {
        libc::flock(
            lock_file.as_ref().expect("lock should exist").as_raw_fd(),
            libc::LOCK_EX | libc::LOCK_NB,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("failed to lock {}", lock_path.display()));
    }

    let executable = std::env::current_exe().context("failed to resolve fake daemon executable")?;
    let mut identity = serde_json::json!({
        "pid": std::process::id(),
        "daemon_start_time": "2026-05-03T00:00:00Z",
        "executable": executable.display().to_string(),
        "executable_canonical": fs::canonicalize(&executable)
            .ok()
            .map(|path| path.display().to_string()),
        "socket_path": socket_path.display().to_string(),
        "protocol_version": protocol_version,
        "snapshot_schema_version": daemon_snapshot_schema_version
    });
    if legacy_identity_sidecar {
        let identity = identity
            .as_object_mut()
            .expect("fake daemon identity should be an object");
        identity.remove("executable_canonical");
        identity.remove("protocol_version");
        identity.remove("snapshot_schema_version");
    }
    fs::write(
        &identity_path,
        serde_json::to_vec_pretty(&identity).context("failed to encode fake daemon identity")?,
    )
    .with_context(|| {
        format!(
            "failed to write fake daemon identity {}",
            identity_path.display()
        )
    })?;

    let mut listener = Some(UnixListener::bind(&socket_path).with_context(|| {
        format!(
            "failed to bind fake daemon socket {}",
            socket_path.display()
        )
    })?);
    if cleanup_on_sigterm {
        unsafe {
            libc::signal(
                libc::SIGTERM,
                fake_daemon_sigterm_handler as *const () as usize,
            );
        }
    }
    fs::write(&ready_pid_path, std::process::id().to_string())
        .with_context(|| format!("failed to write {}", ready_pid_path.display()))?;

    let (mut stream, _) = listener
        .as_ref()
        .expect("listener should exist")
        .accept()
        .context("fake daemon failed to accept lifecycle client")?;
    let mut request = String::new();
    let _ = BufReader::new(
        stream
            .try_clone()
            .context("fake daemon stream should clone")?,
    )
    .read_line(&mut request);
    let frame = serde_json::json!({
        "type": "shutdown",
        "reason": "schema_mismatch",
        "message": format!(
            "unsupported snapshot schema version {client_snapshot_schema_version} (expected {daemon_snapshot_schema_version})"
        )
    });
    writeln!(stream, "{frame}").context("failed to write fake daemon schema mismatch")?;
    stream
        .flush()
        .context("failed to flush fake daemon response")?;

    let mut cleaned = false;
    loop {
        if cleanup_on_sigterm && !cleaned && FAKE_DAEMON_CLEANUP_REQUESTED.load(Ordering::SeqCst) {
            listener.take();
            let _ = fs::remove_file(&socket_path);
            let _ = fs::remove_file(&identity_path);
            lock_file.take();
            cleaned = true;
        }
        sleep(POLL_INTERVAL);
    }
}

fn fake_daemon_env_path(name: &str) -> Result<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .with_context(|| format!("{name} was not set"))
}

fn fake_daemon_env_u32(name: &str) -> Result<u32> {
    std::env::var(name)
        .with_context(|| format!("{name} was not set"))?
        .parse()
        .with_context(|| format!("{name} was not a u32"))
}

#[test]
fn agentscan_uses_explicit_test_tmux_socket_when_default_tmux_tmpdir_is_poisoned() -> Result<()> {
    let harness = TestHarness::new()?;
    let pane_id = harness.start_session("socket-isolation-guard", "sleep 300")?;
    let poisoned_default = tempfile::tempdir().context("failed to create poisoned tmux tmpdir")?;

    let stdout = harness.agentscan_output_with_tmux_tmpdir(
        ["scan", "--all", "--format", "json"],
        poisoned_default.path(),
    )?;
    let snapshot: Value = serde_json::from_str(&stdout).context("scan output was not JSON")?;

    assert!(
        snapshot["panes"]
            .as_array()
            .context("snapshot panes were not an array")?
            .iter()
            .any(|pane| pane["pane_id"] == pane_id),
        "agentscan did not read from the harness tmux socket; scan output was:\n{stdout}"
    );

    Ok(())
}

#[test]
fn agentscan_falls_back_to_a_compatible_tmux_when_path_tmux_is_dropped() -> Result<()> {
    let harness = TestHarness::new()?;
    let pane_id = harness.start_session("compat-fallback", "sleep 300")?;

    // The production fallback only probes well-known install paths, so the
    // retry can only succeed on machines where the real tmux lives at one of
    // them. Skip (not fail) elsewhere.
    if !well_known_tmux_install_can_handshake(&harness) {
        eprintln!(
            "skipping compat-fallback test: no well-known tmux install handshakes with the harness server"
        );
        return Ok(());
    }

    // A fake `tmux` first on PATH reproduces the version-split symptom: the
    // running server drops the fresh client mid-handshake.
    let fake_bin = tempfile::tempdir().context("failed to create fake tmux bin dir")?;
    let fake_tmux = fake_bin.path().join("tmux");
    fs::write(
        &fake_tmux,
        "#!/bin/sh\necho 'server exited unexpectedly' >&2\nexit 1\n",
    )
    .with_context(|| format!("failed to write {}", fake_tmux.display()))?;
    fs::set_permissions(&fake_tmux, fs::Permissions::from_mode(0o755))
        .with_context(|| format!("failed to chmod {}", fake_tmux.display()))?;
    let poisoned_path = format!(
        "{}:{}",
        fake_bin.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let mut command = harness.agentscan_command()?;
    command.env("PATH", &poisoned_path);
    command.args(["scan", "--all", "--format", "json"]);
    let output = command
        .output()
        .context("failed to execute agentscan scan")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "agentscan scan did not recover from the dropped PATH tmux: {}",
            stderr.trim()
        );
    }
    let stdout =
        String::from_utf8(output.stdout).context("agentscan scan output was not valid UTF-8")?;
    let snapshot: Value = serde_json::from_str(&stdout).context("scan output was not JSON")?;

    assert!(
        snapshot["panes"]
            .as_array()
            .context("snapshot panes were not an array")?
            .iter()
            .any(|pane| pane["pane_id"] == pane_id),
        "recovered scan did not read from the harness tmux server; scan output was:\n{stdout}"
    );

    Ok(())
}

// Mirrors the production well-known install list (src/app/tmux/command.rs).
// The per-user linuxbrew candidate is omitted because the agentscan subprocess
// runs with HOME pointed at the harness home directory.
fn well_known_tmux_install_can_handshake(harness: &TestHarness) -> bool {
    [
        "/home/linuxbrew/.linuxbrew/bin/tmux",
        "/opt/homebrew/bin/tmux",
        "/usr/local/bin/tmux",
        "/opt/local/bin/tmux",
        "/usr/bin/tmux",
    ]
    .iter()
    .map(Path::new)
    .filter(|candidate| candidate.is_file())
    .any(|candidate| {
        Command::new(candidate)
            .arg("-S")
            .arg(&harness.tmux_socket_path)
            .args(["display-message", "-p", "compat-probe"])
            .env_remove("TMUX")
            .env("TMUX_TMPDIR", &harness.tmux_tmpdir)
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    })
}

#[test]
fn daemon_serves_snapshot_over_owned_socket_path() -> Result<()> {
    let harness = TestHarness::new()?;
    let pane_id = harness.start_session("socket-snapshot", "sleep 300")?;
    let mut daemon = harness.start_daemon()?;
    harness.wait_for_daemon_snapshot(&mut daemon, |snapshot| {
        pane_from_snapshot(snapshot, &pane_id).is_some()
    })?;

    let mut stream = connect_agentscan_socket(&harness.agentscan_socket_path)?;
    writeln!(
        stream,
        "{}",
        serde_json::json!({
            "type": "hello",
            "protocol_version": daemon_protocol_version(),
            "snapshot_schema_version": snapshot_schema_version(),
            "mode": "snapshot",
        })
    )
    .context("failed to write daemon socket hello")?;
    stream
        .shutdown(std::net::Shutdown::Write)
        .context("failed to close daemon socket write side")?;

    let mut reader = BufReader::new(stream);
    let ack = read_daemon_socket_json_line(&mut reader)?;
    assert_eq!(ack["type"], "hello_ack");
    assert_eq!(ack["protocol_version"], daemon_protocol_version());
    assert_eq!(ack["snapshot_schema_version"], snapshot_schema_version());

    let snapshot_frame = read_daemon_socket_json_line(&mut reader)?;
    assert_eq!(snapshot_frame["type"], "snapshot");
    validate_snapshot_json(&snapshot_frame["snapshot"])?;
    assert_eq!(snapshot_frame["snapshot"]["source"]["kind"], "daemon");
    assert!(
        snapshot_frame["snapshot"]["panes"]
            .as_array()
            .context("snapshot frame panes were not an array")?
            .iter()
            .any(|pane| pane["pane_id"] == pane_id),
        "daemon socket snapshot did not include pane {pane_id}: {snapshot_frame}"
    );

    let mut eof = String::new();
    assert_eq!(
        reader
            .read_line(&mut eof)
            .context("failed to read daemon socket EOF")?,
        0,
        "snapshot client should receive EOF after one snapshot frame"
    );

    daemon.shutdown()?;
    Ok(())
}

#[test]
fn daemon_auto_start_helper_starts_daemon_and_reads_snapshot() -> Result<()> {
    let harness = TestHarness::new()?;
    if cfg!(target_os = "macos") {
        return Ok(());
    }
    if !harness.detached_daemon_start_supported()? {
        return Ok(());
    }
    let pane_id = harness.start_session("auto-start-helper", "sleep 300")?;
    let executable_path = agentscan_bin()?;
    let envs = vec![
        (
            OsString::from("TMUX_TMPDIR"),
            harness.tmux_tmpdir.as_os_str().to_owned(),
        ),
        (
            OsString::from(AGENTSCAN_TMUX_SOCKET_ENV_VAR),
            harness.tmux_socket_path.as_os_str().to_owned(),
        ),
        (
            OsString::from("AGENTSCAN_SOCKET_PATH"),
            harness.agentscan_socket_path.as_os_str().to_owned(),
        ),
        (
            OsString::from("XDG_CACHE_HOME"),
            harness.cache_home.as_os_str().to_owned(),
        ),
        (
            OsString::from("HOME"),
            harness.home_dir.as_os_str().to_owned(),
        ),
    ];
    let env_removes = vec![OsString::from("TMUX")];

    let snapshot_json = agentscan::app::bench_support::daemon_snapshot_via_socket_path_for_tests(
        &harness.agentscan_socket_path,
        &executable_path,
        &envs,
        &env_removes,
    )?;
    let snapshot: Value =
        serde_json::from_str(&snapshot_json).context("helper snapshot should be JSON")?;

    assert_eq!(snapshot["source"]["kind"], "daemon");
    assert!(
        snapshot["panes"]
            .as_array()
            .context("snapshot panes should be an array")?
            .iter()
            .any(|pane| pane["pane_id"] == pane_id),
        "auto-start helper snapshot did not include pane {pane_id}: {snapshot_json}"
    );

    let _ = harness.agentscan(["daemon", "stop"]);
    Ok(())
}

#[test]
fn daemon_fans_out_live_updates_to_subscriber_socket() -> Result<()> {
    let harness = TestHarness::new()?;
    let pane_id = harness.start_session("socket-subscribe", "sh")?;
    let mut daemon = harness.start_daemon()?;
    harness.wait_for_daemon_snapshot(&mut daemon, |snapshot| {
        pane_from_snapshot(snapshot, &pane_id).is_some()
    })?;

    let mut stream = connect_agentscan_socket(&harness.agentscan_socket_path)?;
    stream
        .set_read_timeout(Some(DAEMON_TIMEOUT))
        .context("failed to set subscriber socket read timeout")?;
    writeln!(
        stream,
        "{}",
        serde_json::json!({
            "type": "hello",
            "protocol_version": daemon_protocol_version(),
            "snapshot_schema_version": snapshot_schema_version(),
            "mode": "subscribe",
        })
    )
    .context("failed to write daemon socket subscribe hello")?;

    let mut reader = BufReader::new(
        stream
            .try_clone()
            .context("failed to clone subscriber socket")?,
    );
    let ack = read_daemon_socket_json_line(&mut reader)?;
    assert_eq!(ack["type"], "hello_ack");
    assert_eq!(ack["protocol_version"], daemon_protocol_version());
    assert_eq!(ack["snapshot_schema_version"], snapshot_schema_version());
    let bootstrap = read_daemon_socket_json_line(&mut reader)?;
    assert_eq!(bootstrap["type"], "snapshot");
    validate_snapshot_json(&bootstrap["snapshot"])?;
    assert!(
        bootstrap["snapshot"]["panes"]
            .as_array()
            .context("bootstrap panes were not an array")?
            .iter()
            .any(|pane| pane["pane_id"] == pane_id),
        "subscriber bootstrap did not include pane {pane_id}: {bootstrap}"
    );

    harness.send_title_escape(&pane_id, "Claude Code | Working")?;
    let deadline = Instant::now() + DAEMON_TIMEOUT;
    loop {
        let frame = read_daemon_socket_json_line(&mut reader)?;
        if frame["type"] == "snapshot"
            && frame["snapshot"]["panes"]
                .as_array()
                .context("subscriber update panes were not an array")?
                .iter()
                .any(|pane| {
                    pane["pane_id"] == pane_id
                        && pane["provider"] == "claude"
                        && pane["status"]["kind"] == "busy"
                        && pane["display"]["label"] == "Working"
                })
        {
            break;
        }
        if Instant::now() >= deadline {
            bail!("subscriber did not receive title update before timeout; last frame: {frame}");
        }
    }

    stream
        .shutdown(std::net::Shutdown::Write)
        .context("failed to close subscriber write side")?;
    daemon.shutdown()?;
    Ok(())
}

#[test]
fn focus_event_fans_out_snapshot_without_material_pane_change() -> Result<()> {
    let harness = TestHarness::new()?;
    let pane_id = harness.start_session("focus-event-publish", "sleep 300")?;
    let client = harness.attach_client("focus-event-publish")?;
    let mut daemon = harness.start_daemon()?;
    harness.wait_for_daemon_snapshot(&mut daemon, |snapshot| {
        pane_from_snapshot(snapshot, &pane_id).is_some()
    })?;

    let mut stream = connect_agentscan_socket(&harness.agentscan_socket_path)?;
    stream
        .set_read_timeout(Some(DAEMON_TIMEOUT))
        .context("failed to set subscriber socket read timeout")?;
    writeln!(
        stream,
        "{}",
        serde_json::json!({
            "type": "hello",
            "protocol_version": daemon_protocol_version(),
            "snapshot_schema_version": snapshot_schema_version(),
            "mode": "subscribe",
        })
    )
    .context("failed to write daemon socket subscribe hello")?;

    let mut reader = BufReader::new(
        stream
            .try_clone()
            .context("failed to clone subscriber socket")?,
    );
    let ack = read_daemon_socket_json_line(&mut reader)?;
    assert_eq!(ack["type"], "hello_ack");
    let bootstrap = read_daemon_socket_json_line(&mut reader)?;
    assert_eq!(bootstrap["type"], "snapshot");
    validate_snapshot_json(&bootstrap["snapshot"])?;

    harness.agentscan(["focus", "--client-tty", &client.tty, &pane_id])?;
    let update = read_daemon_socket_json_line(&mut reader)?;
    assert_eq!(update["type"], "snapshot");
    validate_snapshot_json(&update["snapshot"])?;
    assert!(
        update["snapshot"]["panes"]
            .as_array()
            .context("focus event update panes were not an array")?
            .iter()
            .any(|pane| pane["pane_id"] == pane_id),
        "focus event subscriber update did not include pane {pane_id}: {update}"
    );

    stream
        .shutdown(std::net::Shutdown::Write)
        .context("failed to close subscriber write side")?;
    daemon.shutdown()?;
    Ok(())
}

#[test]
fn daemon_does_not_fan_out_noop_reconcile_to_subscriber_socket() -> Result<()> {
    let harness = TestHarness::new()?;
    let pane_id = harness.start_session("socket-noop-reconcile", "sleep 300")?;
    let mut daemon = harness.start_daemon()?;
    harness.wait_for_daemon_snapshot(&mut daemon, |snapshot| {
        pane_from_snapshot(snapshot, &pane_id).is_some()
    })?;

    let mut stream = connect_agentscan_socket(&harness.agentscan_socket_path)?;
    stream
        .set_read_timeout(Some(Duration::from_millis(1500)))
        .context("failed to set subscriber socket read timeout")?;
    writeln!(
        stream,
        "{}",
        serde_json::json!({
            "type": "hello",
            "protocol_version": daemon_protocol_version(),
            "snapshot_schema_version": snapshot_schema_version(),
            "mode": "subscribe",
        })
    )
    .context("failed to write daemon socket subscribe hello")?;

    let mut reader = BufReader::new(
        stream
            .try_clone()
            .context("failed to clone subscriber socket")?,
    );
    let ack = read_daemon_socket_json_line(&mut reader)?;
    assert_eq!(ack["type"], "hello_ack");
    let bootstrap = read_daemon_socket_json_line(&mut reader)?;
    assert_eq!(bootstrap["type"], "snapshot");
    validate_snapshot_json(&bootstrap["snapshot"])?;

    stream
        .set_read_timeout(Some(Duration::from_millis(250)))
        .context("failed to set subscriber socket drain timeout")?;
    let drain_deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let mut pending_line = String::new();
        match reader.read_line(&mut pending_line) {
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if Instant::now() >= drain_deadline {
                    break;
                }
            }
            Ok(0) => bail!("subscriber socket closed while draining setup frames"),
            Ok(_) => {}
            Err(error) => return Err(error).context("failed while draining setup frames"),
        }
    }

    stream
        .set_read_timeout(Some(Duration::from_millis(1500)))
        .context("failed to set subscriber socket no-op reconcile timeout")?;
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) => {}
        Ok(0) => bail!("subscriber socket closed while waiting for no-op reconcile"),
        Ok(_) => bail!("no-op reconcile should not publish subscriber frame, got: {line}"),
        Err(error) => return Err(error).context("failed while waiting for no-op reconcile"),
    }

    stream
        .shutdown(std::net::Shutdown::Write)
        .context("failed to close subscriber write side")?;
    daemon.shutdown()?;
    Ok(())
}

#[test]
fn subscribe_command_streams_bootstrap_and_live_updates() -> Result<()> {
    let harness = TestHarness::new()?;
    let pane_id = harness.start_session("cli-subscribe", "sh")?;
    let mut daemon = harness.start_daemon()?;
    harness.wait_for_daemon_snapshot(&mut daemon, |snapshot| {
        pane_from_snapshot(snapshot, &pane_id).is_some()
    })?;

    let mut child = harness.agentscan_command()?;
    child.args(["subscribe", "--format", "json"]);
    child.stdout(Stdio::piped());
    let mut child = child
        .spawn()
        .context("failed to spawn agentscan subscribe")?;
    let stdout = child
        .stdout
        .take()
        .context("subscribe child did not expose stdout")?;
    set_fd_nonblocking(stdout.as_raw_fd())?;
    let mut reader = BufReader::new(stdout);

    let bootstrap = read_subscribe_stream_until(&mut reader, |frame| {
        frame["type"] == "snapshot"
            && frame["snapshot"]["panes"]
                .as_array()
                .is_some_and(|panes| panes.iter().any(|pane| pane["pane_id"] == pane_id))
    })?;
    validate_snapshot_json(&bootstrap["snapshot"])?;

    harness.send_title_escape(&pane_id, "Claude Code | Working")?;
    let update = read_subscribe_stream_until(&mut reader, |frame| {
        frame["type"] == "snapshot"
            && frame["snapshot"]["panes"].as_array().is_some_and(|panes| {
                panes.iter().any(|pane| {
                    pane["pane_id"] == pane_id
                        && pane["provider"] == "claude"
                        && pane["status"]["kind"] == "busy"
                })
            })
    })?;
    validate_snapshot_json(&update["snapshot"])?;

    child.kill().context("failed to stop subscribe child")?;
    let _ = child.wait();
    daemon.shutdown()?;
    Ok(())
}

#[test]
fn subscribe_command_exits_when_stdout_consumer_closes() -> Result<()> {
    let harness = TestHarness::new()?;
    let pane_id = harness.start_session("cli-subscribe-close", "sh")?;
    let mut daemon = harness.start_daemon()?;
    harness.wait_for_daemon_snapshot(&mut daemon, |snapshot| {
        pane_from_snapshot(snapshot, &pane_id).is_some()
    })?;

    let mut child = harness.agentscan_command()?;
    child.args(["subscribe", "--format", "json"]);
    child.stdout(Stdio::piped());
    let mut child = child
        .spawn()
        .context("failed to spawn agentscan subscribe")?;
    let stdout = child
        .stdout
        .take()
        .context("subscribe child did not expose stdout")?;
    set_fd_nonblocking(stdout.as_raw_fd())?;
    let mut reader = BufReader::new(stdout);

    let bootstrap = read_subscribe_stream_until(&mut reader, |frame| {
        frame["type"] == "snapshot"
            && frame["snapshot"]["panes"]
                .as_array()
                .is_some_and(|panes| panes.iter().any(|pane| pane["pane_id"] == pane_id))
    })?;
    validate_snapshot_json(&bootstrap["snapshot"])?;

    drop(reader);
    let status = wait_for_child_exit(&mut child, DAEMON_TIMEOUT)
        .context("subscribe command should exit after stdout closes")?;
    assert!(
        status.success(),
        "subscribe command should exit successfully after stdout closes, got {status}"
    );
    daemon.shutdown()?;
    Ok(())
}

#[test]
fn daemon_lifecycle_start_status_stop() -> Result<()> {
    let harness = TestHarness::new()?;
    if !harness.detached_daemon_start_supported()? {
        return Ok(());
    }
    let pane_id = harness.start_session("lifecycle-start", "sleep 300")?;

    let start_output = harness.agentscan_output(["daemon", "start"])?;
    assert!(
        start_output.contains("agentscan daemon started"),
        "expected start confirmation, got:\n{start_output}"
    );
    assert!(
        start_output.contains("daemon_state: ready"),
        "expected ready status, got:\n{start_output}"
    );

    let status_output = harness.agentscan_output(["daemon", "status"])?;
    assert!(
        status_output.contains("daemon_state: ready"),
        "expected ready daemon status, got:\n{status_output}"
    );
    assert!(
        status_output.contains("pid:"),
        "expected live identity pid in status, got:\n{status_output}"
    );
    assert!(
        status_output.contains("latest_snapshot_pane_count:"),
        "expected snapshot details in status, got:\n{status_output}"
    );
    assert!(
        status_output.contains("latest_snapshot_update_source:"),
        "expected snapshot update telemetry in status, got:\n{status_output}"
    );
    assert!(
        status_output.contains("control_event_refresh_count:"),
        "expected runtime telemetry in status, got:\n{status_output}"
    );
    assert!(
        status_output.contains("reconcile_attempt_count:"),
        "expected reconcile telemetry in status, got:\n{status_output}"
    );
    assert!(
        status_output.contains("broker_fallback_count:"),
        "expected broker fallback telemetry in status, got:\n{status_output}"
    );

    let status_json_output = harness.agentscan_output(["daemon", "status", "--format", "json"])?;
    let status_json: Value =
        serde_json::from_str(&status_json_output).context("daemon status JSON should parse")?;
    assert_eq!(status_json["daemon_state"], "ready");
    assert!(
        status_json["pid"].as_u64().is_some_and(|pid| pid > 0),
        "expected live identity pid in status JSON, got:\n{status_json_output}"
    );
    assert_eq!(
        status_json["socket_path"],
        harness.agentscan_socket_path.display().to_string()
    );
    assert!(
        status_json["protocol_version"]
            .as_u64()
            .is_some_and(|version| version > 0)
    );
    assert!(
        status_json["snapshot_schema_version"]
            .as_u64()
            .is_some_and(|version| version > 0)
    );
    assert!(
        status_json["latest_snapshot_pane_count"].as_u64().is_some(),
        "expected snapshot details in status JSON, got:\n{status_json_output}"
    );
    assert!(
        status_json["latest_snapshot_update_source"]
            .as_str()
            .is_some_and(|source| !source.is_empty()),
        "expected snapshot update telemetry in status JSON, got:\n{status_json_output}"
    );
    for key in [
        "control_event_refresh_count",
        "reconcile_attempt_count",
        "reconcile_noop_count",
        "reconcile_changed_snapshot_count",
        "targeted_refresh_fallback_to_full_count",
        "broker_fallback_count",
    ] {
        assert!(
            status_json[key].as_u64().is_some(),
            "expected numeric {key} in status JSON, got:\n{status_json_output}"
        );
    }

    let scan_output = harness.agentscan_output(["scan", "--all", "--format", "json"])?;
    assert!(
        scan_output.contains(&pane_id),
        "expected test pane to exist while lifecycle daemon runs"
    );

    let stop_output = harness.agentscan_output(["daemon", "stop"])?;
    assert!(
        stop_output.contains("agentscan daemon stopped"),
        "expected stop confirmation, got:\n{stop_output}"
    );

    let stopped_output = harness.agentscan_output(["daemon", "status"])?;
    assert!(
        stopped_output.contains("daemon_state: not_running"),
        "expected not-running status after stop, got:\n{stopped_output}"
    );

    Ok(())
}

#[test]
fn daemon_lifecycle_status_reports_not_running() -> Result<()> {
    let harness = TestHarness::new()?;

    let status_output = harness.agentscan_output(["daemon", "status"])?;

    assert!(
        status_output.contains("daemon_state: not_running"),
        "expected not-running daemon status, got:\n{status_output}"
    );
    Ok(())
}

#[test]
fn daemon_lifecycle_stop_is_idempotent() -> Result<()> {
    let harness = TestHarness::new()?;

    let first = harness.agentscan_output(["daemon", "stop"])?;
    let second = harness.agentscan_output(["daemon", "stop"])?;

    assert!(first.contains("daemon_state: not_running"));
    assert!(second.contains("daemon_state: not_running"));
    Ok(())
}

#[test]
fn daemon_lifecycle_restart() -> Result<()> {
    let harness = TestHarness::new()?;
    if !harness.detached_daemon_start_supported()? {
        return Ok(());
    }
    let _pane_id = harness.start_session("lifecycle-restart", "sleep 300")?;

    harness.agentscan(["daemon", "start"])?;
    let before = harness.agentscan_output(["daemon", "status"])?;
    let before_pid = lifecycle_status_value(&before, "pid")
        .context("status before restart did not include pid")?;
    let before_started_at = lifecycle_status_value(&before, "daemon_start_time")
        .context("status before restart did not include start time")?;

    let restart = harness.agentscan_output(["daemon", "restart"])?;
    assert!(
        restart.contains("agentscan daemon started"),
        "expected restart to start daemon, got:\n{restart}"
    );
    let after = harness.agentscan_output(["daemon", "status"])?;
    let after_pid = lifecycle_status_value(&after, "pid")
        .context("status after restart did not include pid")?;
    let after_started_at = lifecycle_status_value(&after, "daemon_start_time")
        .context("status after restart did not include start time")?;
    assert!(
        before_pid != after_pid || before_started_at != after_started_at,
        "restart should replace daemon identity"
    );

    harness.agentscan(["daemon", "stop"])?;
    Ok(())
}

#[test]
fn daemon_lifecycle_start_reuses_running_daemon() -> Result<()> {
    let harness = TestHarness::new()?;
    if !harness.detached_daemon_start_supported()? {
        return Ok(());
    }
    let _pane_id = harness.start_session("lifecycle-reuse", "sleep 300")?;

    harness.agentscan(["daemon", "start"])?;
    let before = harness.agentscan_output(["daemon", "status"])?;
    let before_pid = lifecycle_status_value(&before, "pid")
        .context("status before second start did not include pid")?;

    let second_start = harness.agentscan_output(["daemon", "start"])?;
    assert!(
        second_start.contains("agentscan daemon already running"),
        "expected second start to reuse daemon, got:\n{second_start}"
    );
    let after_pid = lifecycle_status_value(&second_start, "pid")
        .context("second start status did not include pid")?;
    assert_eq!(
        before_pid, after_pid,
        "second start should not replace daemon"
    );

    harness.agentscan(["daemon", "stop"])?;
    Ok(())
}

#[test]
fn daemon_lifecycle_concurrent_start_uses_single_daemon() -> Result<()> {
    let harness = TestHarness::new()?;
    if !harness.detached_daemon_start_supported()? {
        return Ok(());
    }
    let _pane_id = harness.start_session("lifecycle-concurrent", "sleep 300")?;

    let first = harness
        .agentscan_command()?
        .args(["daemon", "start"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn first daemon start")?;
    let second = harness
        .agentscan_command()?
        .args(["daemon", "start"])
        .output()
        .context("failed to run second daemon start")?;
    let first = first
        .wait_with_output()
        .context("first start wait failed")?;

    assert!(
        first.status.success(),
        "first start should succeed; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        second.status.success(),
        "second start should succeed; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );

    let status = harness.agentscan_output(["daemon", "status"])?;
    assert!(
        lifecycle_status_value(&status, "pid").is_some(),
        "status should include a single live daemon pid, got:\n{status}"
    );

    harness.agentscan(["daemon", "stop"])?;
    Ok(())
}

#[test]
fn daemon_lifecycle_stop_keeps_mismatched_identity_sidecar() -> Result<()> {
    let harness = TestHarness::new()?;
    if !harness.detached_daemon_start_supported()? {
        return Ok(());
    }
    let _pane_id = harness.start_session("lifecycle-identity", "sleep 300")?;

    harness.agentscan(["daemon", "start"])?;
    let identity_path = harness
        .agentscan_socket_path
        .with_extension("sock.identity.json");
    let mismatched_identity = serde_json::json!({
        "pid": 0,
        "daemon_start_time": "not-this-daemon",
        "executable": "/tmp/other-agentscan",
        "executable_canonical": null,
        "socket_path": harness.agentscan_socket_path.display().to_string(),
        "protocol_version": 1,
        "snapshot_schema_version": 1
    });
    fs::write(
        &identity_path,
        serde_json::to_vec_pretty(&mismatched_identity)
            .context("failed to encode mismatched identity")?,
    )
    .context("failed to overwrite daemon identity")?;

    harness.agentscan(["daemon", "stop"])?;

    let identity_after_stop =
        fs::read_to_string(&identity_path).context("mismatched identity should remain")?;
    assert!(
        identity_after_stop.contains("not-this-daemon"),
        "stop should not remove or replace mismatched identity, got:\n{identity_after_stop}"
    );
    Ok(())
}

#[test]
fn daemon_lifecycle_stop_uses_guarded_sigkill_after_sigterm_timeout() -> Result<()> {
    let harness = TestHarness::new()?;
    let pid_path = harness.agentscan_socket_path.with_extension("fake.pid");
    let launch_script = "sh -c 'trap \"\" TERM; echo $$ > \"$PID_PATH\"; while true; do sleep 1; done' \
        >/dev/null 2>&1 </dev/null &";
    let launch_status = Command::new("sh")
        .env("PID_PATH", &pid_path)
        .args(["-c", launch_script])
        .status()
        .context("failed to launch SIGTERM-resistant process")?;
    assert!(
        launch_status.success(),
        "failed to launch SIGTERM-resistant process: {launch_status}"
    );
    let pid = wait_for_pid_file(&pid_path)?;
    let mut kill_guard = KillPidGuard::new(pid);
    let socket_path = harness.agentscan_socket_path.clone();
    let socket_path_text = socket_path.display().to_string();
    let listener = UnixListener::bind(&socket_path).context("failed to bind fake daemon socket")?;
    listener
        .set_nonblocking(true)
        .context("failed to make fake daemon listener nonblocking")?;
    let handle = std::thread::spawn(move || {
        for _ in 0..2 {
            let mut stream = accept_fake_daemon_connection(&listener);
            let mut request = String::new();
            let _ = BufReader::new(stream.try_clone().expect("stream should clone"))
                .read_line(&mut request);
            write_fake_lifecycle_status(&mut stream, pid, &socket_path_text, "fake-start");
        }
    });

    let output = harness
        .agentscan_command()?
        .args(["daemon", "stop"])
        .output()
        .context("failed to run daemon stop")?;

    handle.join().expect("fake daemon should join");
    assert!(
        output.status.success(),
        "daemon stop should use guarded SIGKILL; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    kill_guard.disarm();
    Ok(())
}

#[test]
fn daemon_lifecycle_refuses_non_socket_collision() -> Result<()> {
    let harness = TestHarness::new()?;
    fs::write(&harness.agentscan_socket_path, "not a socket")
        .context("failed to create non-socket collision")?;

    let output = harness
        .agentscan_command()?
        .args(["daemon", "start"])
        .output()
        .context("failed to run daemon start")?;

    assert!(
        !output.status.success(),
        "daemon start should fail for non-socket collision"
    );
    assert!(
        harness.agentscan_socket_path.exists(),
        "daemon start must not unlink non-socket collision"
    );
    let stderr = String::from_utf8(output.stderr).context("stderr was not valid UTF-8")?;
    assert!(
        stderr.contains("non-socket path"),
        "expected non-socket refusal, got:\n{stderr}"
    );
    Ok(())
}

#[test]
fn daemon_lifecycle_cleans_stale_socket() -> Result<()> {
    let harness = TestHarness::new()?;
    if !harness.detached_daemon_start_supported()? {
        return Ok(());
    }
    let _pane_id = harness.start_session("lifecycle-stale", "sleep 300")?;
    {
        let _listener = UnixListener::bind(&harness.agentscan_socket_path)
            .context("failed to create stale socket")?;
    }
    assert!(harness.agentscan_socket_path.exists());

    harness.agentscan(["daemon", "start"])?;
    let status = harness.agentscan_output(["daemon", "status"])?;
    assert!(status.contains("daemon_state: ready"));

    harness.agentscan(["daemon", "stop"])?;
    Ok(())
}

#[test]
fn daemon_lifecycle_stop_cleans_stale_socket() -> Result<()> {
    let harness = TestHarness::new()?;
    {
        let _listener = UnixListener::bind(&harness.agentscan_socket_path)
            .context("failed to create stale socket")?;
    }
    assert!(harness.agentscan_socket_path.exists());

    let output = harness.agentscan_output(["daemon", "stop"])?;
    assert!(
        output.contains("daemon_state: not_running"),
        "expected not-running stop output, got:\n{output}"
    );
    assert!(
        !harness.agentscan_socket_path.exists(),
        "daemon stop should unlink refused stale Unix socket"
    );
    Ok(())
}

#[test]
fn daemon_lifecycle_stop_signals_incompatible_daemon_from_sidecar() -> Result<()> {
    let harness = TestHarness::new()?;
    let fake_daemon_pid = start_fake_incompatible_daemon(
        &harness.agentscan_socket_path,
        snapshot_schema_version() - 1,
    )?;
    let mut kill_guard = KillPidGuard::new(fake_daemon_pid);

    let output = harness
        .agentscan_command()?
        .args(["daemon", "stop"])
        .output()
        .context("failed to run daemon stop")?;

    assert!(
        output.status.success(),
        "daemon stop should signal incompatible daemon from sidecar; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("agentscan daemon stopped"),
        "stop output should report success"
    );
    assert!(
        !harness
            .agentscan_socket_path
            .with_extension("sock.identity.json")
            .exists(),
        "stop should remove matching identity sidecar"
    );
    kill_guard.disarm();
    Ok(())
}

#[test]
fn daemon_lifecycle_stop_signals_incompatible_daemon_with_legacy_sidecar() -> Result<()> {
    let harness = TestHarness::new()?;
    let fake_daemon_pid = start_fake_incompatible_daemon_with_legacy_identity_sidecar(
        &harness.agentscan_socket_path,
        snapshot_schema_version() - 1,
    )?;
    let mut kill_guard = KillPidGuard::new(fake_daemon_pid);

    let output = harness
        .agentscan_command()?
        .args(["daemon", "stop"])
        .output()
        .context("failed to run daemon stop")?;

    assert!(
        output.status.success(),
        "daemon stop should signal incompatible daemon from legacy sidecar; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !harness
            .agentscan_socket_path
            .with_extension("sock.identity.json")
            .exists(),
        "stop should remove matching legacy identity sidecar"
    );
    kill_guard.disarm();
    Ok(())
}

#[test]
fn daemon_lifecycle_stop_sigkills_incompatible_daemon_after_socket_cleanup() -> Result<()> {
    let harness = TestHarness::new()?;
    let fake_daemon_pid = start_fake_incompatible_daemon_with_options(
        &harness.agentscan_socket_path,
        snapshot_schema_version() - 1,
        true,
        false,
    )?;
    let mut kill_guard = KillPidGuard::new(fake_daemon_pid);

    let output = harness
        .agentscan_command()?
        .args(["daemon", "stop"])
        .output()
        .context("failed to run daemon stop")?;

    assert!(
        output.status.success(),
        "daemon stop should SIGKILL incompatible daemon after socket cleanup; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !process_is_live_for_test(fake_daemon_pid),
        "daemon stop should terminate fake daemon pid {fake_daemon_pid}"
    );
    kill_guard.disarm();
    Ok(())
}

#[test]
fn daemon_lifecycle_stop_refuses_incompatible_daemon_with_stale_sidecar() -> Result<()> {
    let harness = TestHarness::new()?;
    let pid_path = harness
        .agentscan_socket_path
        .with_extension("stale-sidecar.pid");
    let pid = launch_background_sleep(&pid_path)?;
    let _kill_guard = KillPidGuard::new(pid);
    write_daemon_identity_sidecar(
        &harness.agentscan_socket_path,
        pid,
        snapshot_schema_version() - 1,
    )?;
    let handle = serve_incompatible_lifecycle_once(&harness.agentscan_socket_path);

    let output = harness
        .agentscan_command()?
        .args(["daemon", "stop"])
        .output()
        .context("failed to run daemon stop")?;

    handle.join().expect("fake daemon should join");
    assert!(
        !output.status.success(),
        "daemon stop should reject stale incompatible sidecar"
    );
    let stderr = String::from_utf8(output.stderr).context("stderr was not valid UTF-8")?;
    assert!(
        stderr.contains("identity sidecar is not safe to signal"),
        "expected sidecar safety error, got:\n{stderr}"
    );
    assert!(
        process_is_live_for_test(pid),
        "stale sidecar rejection should not signal pid {pid}"
    );
    Ok(())
}

#[test]
fn daemon_lifecycle_status_reports_incompatible_daemon_guidance() -> Result<()> {
    let harness = TestHarness::new()?;
    let listener = UnixListener::bind(&harness.agentscan_socket_path)
        .context("failed to bind fake daemon socket")?;
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("fake daemon should accept");
        let mut request = String::new();
        let _ = BufReader::new(stream.try_clone().expect("stream should clone"))
            .read_line(&mut request);
        stream
            .write_all(
                br#"{"type":"shutdown","reason":"protocol_mismatch","message":"old daemon"}"#,
            )
            .expect("shutdown frame should write");
        stream.write_all(b"\n").expect("newline should write");
        stream.flush().expect("shutdown frame should flush");
    });

    let output = harness
        .agentscan_command()?
        .args(["daemon", "status"])
        .output()
        .context("failed to run daemon status")?;

    handle.join().expect("fake daemon should join");
    assert!(
        !output.status.success(),
        "status should reject incompatible daemon"
    );
    let stderr = String::from_utf8(output.stderr).context("stderr was not valid UTF-8")?;
    assert!(
        stderr.contains("stop the incompatible daemon manually"),
        "expected manual incompatible-daemon guidance, got:\n{stderr}"
    );
    Ok(())
}

#[test]
fn daemon_lifecycle_start_failure_reports_log_and_cleans_socket() -> Result<()> {
    let harness = TestHarness::new()?;
    if !harness.detached_daemon_start_supported()? {
        return Ok(());
    }

    let output = harness
        .agentscan_command()?
        .args(["daemon", "start"])
        .output()
        .context("failed to run daemon start")?;

    assert!(
        !output.status.success(),
        "daemon start without tmux server should fail"
    );
    let stderr = String::from_utf8(output.stderr).context("stderr was not valid UTF-8")?;
    assert!(
        stderr.contains("see log"),
        "expected startup failure to mention log path, got:\n{stderr}"
    );
    assert!(
        !harness.agentscan_socket_path.exists(),
        "failed detached start should clean owned socket"
    );
    Ok(())
}

#[test]
fn daemon_updates_socket_snapshot_when_titles_change() -> Result<()> {
    let harness = TestHarness::new()?;
    let pane_id = harness.start_session("title-updates", "sh")?;
    let mut daemon = harness.start_daemon()?;

    harness.wait_for_daemon_pane(&mut daemon, &pane_id, |_| true)?;

    harness.send_title_escape(&pane_id, "Claude Code | Working")?;
    harness.wait_for_daemon_pane(&mut daemon, &pane_id, |pane| {
        pane["provider"] == "claude"
            && pane["status"]["kind"] == "busy"
            && pane["display"]["label"] == "Working"
    })?;

    harness.send_title_escape(&pane_id, "Claude Code | Ready")?;
    harness.wait_for_daemon_pane(&mut daemon, &pane_id, |pane| {
        pane["provider"] == "claude"
            && pane["status"]["kind"] == "idle"
            && pane["display"]["label"] == "Ready"
    })?;

    daemon.shutdown()?;
    Ok(())
}

#[test]
fn daemon_updates_socket_snapshot_when_metadata_changes() -> Result<()> {
    let harness = TestHarness::new()?;
    let pane_id = harness.start_session("metadata-updates", "sh")?;
    harness.send_title_escape(&pane_id, "metadata-updates")?;
    let mut daemon = harness.start_daemon()?;

    harness.wait_for_daemon_pane(&mut daemon, &pane_id, |_| true)?;

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
    harness.wait_for_daemon_pane(&mut daemon, &pane_id, |pane| {
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
    harness.wait_for_daemon_pane(&mut daemon, &pane_id, |pane| {
        pane["provider"].is_null()
            && pane["display"]["label"] == "metadata-updates"
            && pane["status"]["kind"] == "unknown"
    })?;

    daemon.shutdown()?;
    Ok(())
}

// Exercises two separate tmux sessions. tmux control mode is scoped to the
// attached session, so the metadata pane in the non-primary session is covered
// by a per-session event subscriber client (both sessions exist before the daemon
// starts, so the startup subscriber attachment covers it). Sessions created or
// destroyed at runtime are handled by the session lifecycle (Phase 2).
#[test]
fn metadata_helpers_survive_unrelated_daemon_updates() -> Result<()> {
    let harness = TestHarness::new()?;
    let metadata_pane_id = harness.start_session("metadata-survives", "sh")?;
    let trigger_pane_id = harness.start_session("metadata-trigger", "sh")?;
    harness.send_title_escape(&metadata_pane_id, "metadata-survives")?;
    let mut daemon = harness.start_daemon()?;

    harness.wait_for_daemon_pane(&mut daemon, &metadata_pane_id, |_| true)?;
    harness.wait_for_daemon_pane(&mut daemon, &trigger_pane_id, |_| true)?;

    harness.agentscan([
        "tmux",
        "set-metadata",
        "--pane-id",
        &metadata_pane_id,
        "--provider",
        "codex",
        "--label",
        "Persistent Metadata",
        "--state",
        "busy",
    ])?;
    harness.wait_for_daemon_pane(&mut daemon, &metadata_pane_id, |pane| {
        pane["provider"] == "codex"
            && pane["display"]["label"] == "Persistent Metadata"
            && pane["status"]["kind"] == "busy"
    })?;

    harness.send_title_escape(&trigger_pane_id, "Claude Code | Working")?;
    harness.wait_for_daemon_pane(&mut daemon, &trigger_pane_id, |pane| {
        pane["provider"] == "claude"
            && pane["status"]["kind"] == "busy"
            && pane["display"]["label"] == "Working"
    })?;
    harness.wait_for_daemon_pane(&mut daemon, &metadata_pane_id, |pane| {
        pane["provider"] == "codex"
            && pane["display"]["label"] == "Persistent Metadata"
            && pane["status"]["kind"] == "busy"
    })?;
    let metadata_snapshot = harness.wait_for_daemon_snapshot(&mut daemon, |snapshot| {
        pane_from_snapshot(snapshot, &metadata_pane_id).is_some_and(|pane| {
            pane["provider"] == "codex"
                && pane["display"]["label"] == "Persistent Metadata"
                && pane["status"]["kind"] == "busy"
        })
    })?;
    let metadata_generated_at = metadata_snapshot["generated_at"]
        .as_str()
        .context("metadata snapshot was missing generated_at")?
        .to_string();

    harness.send_title_escape(&trigger_pane_id, "Claude Code | Still Working")?;
    harness.wait_for_daemon_snapshot(&mut daemon, |snapshot| {
        snapshot["generated_at"].as_str() != Some(metadata_generated_at.as_str())
            && pane_from_snapshot(snapshot, &metadata_pane_id).is_some_and(|pane| {
                pane["provider"] == "codex"
                    && pane["display"]["label"] == "Persistent Metadata"
                    && pane["status"]["kind"] == "busy"
            })
    })?;

    daemon.shutdown()?;
    Ok(())
}

// A session created after the daemon is already running must get an event-only
// subscriber client attached via the `%sessions-changed` lifecycle, so an
// in-place metadata change on its pane propagates through events rather than
// waiting for the infrequent self-heal reconcile.
#[test]
fn runtime_created_session_gets_event_subscriber() -> Result<()> {
    let harness = TestHarness::new()?;
    let primary_pane_id = harness.start_session("primary-session", "sh")?;
    let mut daemon = harness.start_daemon()?;
    harness.wait_for_daemon_pane(&mut daemon, &primary_pane_id, |_| true)?;

    // Create a brand-new session at runtime. `%sessions-changed` triggers both a
    // resnapshot (picks up the pane) and subscriber attachment for the session.
    let runtime_pane_id = harness.start_session("runtime-session", "sh")?;
    harness.wait_for_daemon_pane(&mut daemon, &runtime_pane_id, |_| true)?;

    // This in-place change emits no topology notification, so it can only reach
    // the daemon through the newly-attached subscriber's event stream.
    harness.agentscan([
        "tmux",
        "set-metadata",
        "--pane-id",
        &runtime_pane_id,
        "--provider",
        "codex",
        "--label",
        "Runtime Session",
        "--state",
        "busy",
    ])?;
    harness.wait_for_daemon_pane(&mut daemon, &runtime_pane_id, |pane| {
        pane["provider"] == "codex"
            && pane["status"]["kind"] == "busy"
            && pane["display"]["label"] == "Runtime Session"
            && pane["status"]["source"] == "pane_metadata"
    })?;

    daemon.shutdown()?;
    Ok(())
}

#[test]
fn multiple_sessions_each_get_event_subscriber_and_propagate_in_place() -> Result<()> {
    let harness = TestHarness::new()?;
    let primary_pane_id = harness.start_session("scale-primary", "sh")?;
    let mut daemon = harness.start_daemon()?;
    harness.wait_for_daemon_pane(&mut daemon, &primary_pane_id, |_| true)?;

    // Stand up several additional sessions. Each non-primary session should get
    // its own event-only subscriber client so in-place status changes propagate
    // without the periodic reconcile (disabled by default).
    let mut extra_pane_ids = Vec::new();
    for index in 0..3 {
        let pane_id = harness.start_session(&format!("scale-extra-{index}"), "sh")?;
        harness.wait_for_daemon_pane(&mut daemon, &pane_id, |_| true)?;
        extra_pane_ids.push(pane_id);
    }

    // The subscriber set is everything except the primary's own session, so with
    // four sessions total the broker should report three subscriber clients.
    let deadline = Instant::now() + DAEMON_TIMEOUT;
    loop {
        let status_json_output =
            harness.agentscan_output(["daemon", "status", "--format", "json"])?;
        let status_json: Value =
            serde_json::from_str(&status_json_output).context("daemon status JSON should parse")?;
        if status_json["control_mode_broker_subscriber_count"].as_u64() == Some(3) {
            break;
        }
        if Instant::now() >= deadline {
            anyhow::bail!("expected three subscriber clients, got:\n{status_json_output}");
        }
        sleep(POLL_INTERVAL);
    }

    // Flip status in a non-primary session via an in-place metadata write (no
    // topology notification). It can only reach the daemon through that session's
    // subscriber event stream, with `%output` globally paused.
    let target_pane_id = &extra_pane_ids[2];
    harness.agentscan([
        "tmux",
        "set-metadata",
        "--pane-id",
        target_pane_id,
        "--provider",
        "codex",
        "--label",
        "Scaled Session",
        "--state",
        "busy",
    ])?;
    harness.wait_for_daemon_pane(&mut daemon, target_pane_id, |pane| {
        pane["provider"] == "codex"
            && pane["status"]["kind"] == "busy"
            && pane["display"]["label"] == "Scaled Session"
            && pane["status"]["source"] == "pane_metadata"
    })?;

    daemon.shutdown()?;
    Ok(())
}

#[test]
fn focus_targets_explicit_client_tty() -> Result<()> {
    let harness = TestHarness::new()?;
    let _root_pane_id = harness.start_session("focus-explicit", "sleep 300")?;
    let split_pane_id = harness.split_window("focus-explicit:0.0", "sleep 300")?;
    let mut client = harness.attach_client("focus-explicit")?;

    harness.agentscan(["-f", "focus", "--client-tty", &client.tty, &split_pane_id])?;
    harness.wait_for_client_pane(&mut client, &split_pane_id)?;

    Ok(())
}

#[test]
fn one_shot_list_reads_daemon_socket() -> Result<()> {
    let harness = TestHarness::new()?;
    let snapshot: Value =
        serde_json::from_str(CACHE_SNAPSHOT_FIXTURE).context("fixture should be JSON")?;
    let fake_daemon = serve_fake_snapshot(&harness.agentscan_socket_path, snapshot);

    let stdout = harness.agentscan_output(["list", "--format", "json"])?;
    let snapshot: Value = serde_json::from_str(&stdout).context("list output should be JSON")?;

    assert!(
        snapshot["panes"]
            .as_array()
            .context("snapshot panes should be an array")?
            .iter()
            .any(|pane| pane["pane_id"] == "%67" && pane["provider"] == "codex"),
        "list should read the daemon socket snapshot, got:\n{stdout}"
    );
    fake_daemon.join();
    Ok(())
}

#[test]
fn one_shot_inspect_reads_daemon_socket() -> Result<()> {
    let harness = TestHarness::new()?;
    let snapshot: Value =
        serde_json::from_str(CACHE_SNAPSHOT_FIXTURE).context("fixture should be JSON")?;
    let fake_daemon = serve_fake_snapshot(&harness.agentscan_socket_path, snapshot);

    let stdout = harness.agentscan_output(["inspect", "%67", "--format", "json"])?;
    let pane: Value = serde_json::from_str(&stdout).context("inspect output should be JSON")?;

    assert_eq!(pane["pane_id"], "%67");
    assert_eq!(pane["provider"], "codex");
    fake_daemon.join();
    Ok(())
}

#[test]
fn one_shot_snapshot_reads_daemon_socket() -> Result<()> {
    let harness = TestHarness::new()?;
    let snapshot: Value =
        serde_json::from_str(CACHE_SNAPSHOT_FIXTURE).context("fixture should be JSON")?;
    let fake_daemon = serve_fake_snapshot(&harness.agentscan_socket_path, snapshot);

    let stdout = harness.agentscan_output(["snapshot", "--format", "json"])?;
    let snapshot: Value =
        serde_json::from_str(&stdout).context("snapshot output should be JSON")?;

    assert_eq!(snapshot["source"]["kind"], "daemon");
    assert!(
        snapshot["panes"]
            .as_array()
            .context("snapshot panes should be an array")?
            .iter()
            .any(|pane| pane["pane_id"] == "%67" && pane["provider"] == "codex"),
        "snapshot should read the daemon socket snapshot, got:\n{stdout}"
    );
    fake_daemon.join();
    Ok(())
}

#[test]
fn one_shot_focus_validates_with_daemon_socket_before_focusing() -> Result<()> {
    let harness = TestHarness::new()?;
    let _root_pane_id = harness.start_session("one-shot-focus", "sleep 300")?;
    let split_pane_id = harness.split_window("one-shot-focus:0.0", "sleep 300")?;
    let mut client = harness.attach_client("one-shot-focus")?;
    let stdout = harness.agentscan_output(["scan", "--all", "--format", "json"])?;
    let snapshot: Value = serde_json::from_str(&stdout).context("scan output should be JSON")?;
    let fake_daemon = serve_fake_snapshot(&harness.agentscan_socket_path, snapshot);

    harness.agentscan(["focus", "--client-tty", &client.tty, &split_pane_id])?;
    harness.wait_for_client_pane(&mut client, &split_pane_id)?;

    fake_daemon.join();
    Ok(())
}

#[test]
fn no_auto_start_fails_when_daemon_socket_is_missing() -> Result<()> {
    let harness = TestHarness::new()?;

    let output = harness
        .agentscan_command()?
        .args(["list", "--no-auto-start", "--format", "json"])
        .output()
        .context("failed to run list with no-auto-start")?;
    assert!(
        !output.status.success(),
        "list should fail without daemon when auto-start is disabled; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8(output.stderr).context("stderr should be UTF-8")?;
    assert!(
        stderr.contains("daemon auto-start is disabled"),
        "expected auto-start disabled error, got:\n{stderr}"
    );
    Ok(())
}

#[test]
fn env_no_auto_start_fails_when_daemon_socket_is_missing() -> Result<()> {
    let harness = TestHarness::new()?;

    let output = harness
        .agentscan_command()?
        .args(["list", "--format", "json"])
        .env("AGENTSCAN_NO_AUTO_START", "1")
        .output()
        .context("failed to run list with env opt-out")?;
    assert!(
        !output.status.success(),
        "list should fail without daemon when env disables auto-start; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8(output.stderr).context("stderr should be UTF-8")?;
    assert!(
        stderr.contains("daemon auto-start is disabled"),
        "expected auto-start disabled error, got:\n{stderr}"
    );
    Ok(())
}

#[test]
fn one_shot_refresh_and_scan_refresh_bypass_daemon_socket() -> Result<()> {
    let harness = TestHarness::new()?;
    let pane_id = harness.start_session("refresh-bypass-daemon", "sleep 300")?;
    fs::write(&harness.agentscan_socket_path, b"not a socket")
        .context("failed to poison daemon socket path")?;

    for args in [
        ["list", "--refresh", "--format", "json"].as_slice(),
        ["snapshot", "--refresh", "--format", "json"].as_slice(),
        ["scan", "--refresh", "--format", "json"].as_slice(),
        ["inspect", &pane_id, "--refresh", "--format", "json"].as_slice(),
    ] {
        harness.agentscan(args)?;
    }

    Ok(())
}

#[test]
fn hotkeys_json_exposes_current_picker_assignments() -> Result<()> {
    let harness = TestHarness::new()?;
    let root_pane_id = harness.start_session("hotkeys-json", "sleep 300")?;
    let split_pane_id = harness.split_window("hotkeys-json:0.0", "sleep 300")?;
    harness.seed_tui_two_pane_metadata(&root_pane_id, &split_pane_id)?;
    let mut daemon = harness.start_daemon()?;
    harness.wait_for_daemon_pane(&mut daemon, &split_pane_id, |_| true)?;

    let stdout = harness.agentscan_output(["hotkeys", "--format", "json"])?;
    let envelope: Value = serde_json::from_str(&stdout).context("hotkeys output should be JSON")?;
    assert_eq!(envelope["schema_version"], 1);
    let rows = envelope["rows"]
        .as_array()
        .context("hotkeys envelope should carry a `rows` array")?;

    assert_eq!(rows[0]["key"], "1");
    assert_eq!(rows[0]["pane_id"], root_pane_id);
    assert_eq!(rows[0]["provider"], "codex");
    assert_eq!(rows[0]["status"]["kind"], "idle");
    assert_eq!(rows[0]["display_label"], "Root Task");
    assert_eq!(rows[0]["location_tag"], "hotkeys-json:0.0");
    assert_eq!(rows[0]["location"]["session_name"], "hotkeys-json");
    assert_eq!(rows[1]["key"], "2");
    assert_eq!(rows[1]["pane_id"], split_pane_id);
    assert_eq!(rows[1]["provider"], "claude");

    Ok(())
}

#[test]
fn hotkey_focuses_assigned_pane_from_current_picker() -> Result<()> {
    let harness = TestHarness::new()?;
    let root_pane_id = harness.start_session("hotkey-focus", "sleep 300")?;
    let split_pane_id = harness.split_window("hotkey-focus:0.0", "sleep 300")?;
    harness.seed_tui_two_pane_metadata(&root_pane_id, &split_pane_id)?;
    let mut daemon = harness.start_daemon()?;
    harness.wait_for_daemon_pane(&mut daemon, &split_pane_id, |_| true)?;
    let mut client = harness.attach_client("hotkey-focus")?;

    harness.agentscan(["hotkey", "2"])?;
    harness.wait_for_client_pane(&mut client, &split_pane_id)?;

    Ok(())
}

#[test]
fn hotkey_reports_invalid_or_unassigned_keys() -> Result<()> {
    let harness = TestHarness::new()?;
    let root_pane_id = harness.start_session("hotkey-invalid", "sleep 300")?;
    harness.agentscan([
        "tmux",
        "set-metadata",
        "--pane-id",
        &root_pane_id,
        "--provider",
        "codex",
        "--label",
        "Root Task",
        "--state",
        "idle",
    ])?;
    let mut daemon = harness.start_daemon()?;
    harness.wait_for_daemon_pane(&mut daemon, &root_pane_id, |_| true)?;

    let invalid = harness
        .agentscan_command()?
        .args(["hotkey", "a"])
        .output()
        .context("failed to run hotkey with invalid key")?;
    assert!(
        !invalid.status.success(),
        "invalid hotkey should fail; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&invalid.stdout),
        String::from_utf8_lossy(&invalid.stderr)
    );
    let stderr = String::from_utf8(invalid.stderr).context("stderr should be UTF-8")?;
    assert!(
        stderr.contains("is not supported"),
        "expected unsupported key error, got:\n{stderr}"
    );

    let unassigned = harness
        .agentscan_command()?
        .args(["hotkey", "q"])
        .output()
        .context("failed to run hotkey with unassigned key")?;
    assert!(
        !unassigned.status.success(),
        "unassigned hotkey should fail; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&unassigned.stdout),
        String::from_utf8_lossy(&unassigned.stderr)
    );
    let stderr = String::from_utf8(unassigned.stderr).context("stderr should be UTF-8")?;
    assert!(
        stderr.contains("hotkey Q is not assigned"),
        "expected unassigned key error, got:\n{stderr}"
    );

    Ok(())
}

#[test]
fn tmux_hotkey_binding_reports_unassigned_key_without_entering_view_mode() -> Result<()> {
    let harness = TestHarness::new()?;
    let root_pane_id = harness.start_session("tmux-hotkey-unassigned", "sleep 300")?;
    harness.agentscan([
        "tmux",
        "set-metadata",
        "--pane-id",
        &root_pane_id,
        "--provider",
        "codex",
        "--label",
        "Root Task",
        "--state",
        "idle",
    ])?;
    let mut daemon = harness.start_daemon()?;
    harness.wait_for_daemon_pane(&mut daemon, &root_pane_id, |_| true)?;
    let mut client = harness.attach_client("tmux-hotkey-unassigned")?;
    let done_path = harness._tempdir.path().join("tmux-hotkey-done");

    let output = harness
        .agentscan_command()?
        .args(["tmux", "hotkey", "z", "--client-tty", &client.tty])
        .output()
        .context("failed to run tmux hotkey with unassigned key")?;
    assert!(
        output.status.success(),
        "tmux hotkey should return success for expected picker misses"
    );
    assert!(
        output.stdout.is_empty() && output.stderr.is_empty(),
        "tmux hotkey should report misses through display-message only; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let run_shell_command = format!(
        "TMUX_TMPDIR={} AGENTSCAN_TMUX_SOCKET={} AGENTSCAN_SOCKET_PATH={} AGENTSCAN_CONTROL_MODE_ACTIVE_RECONCILE_INTERVAL_MS=1000 XDG_CACHE_HOME={} HOME={} {} tmux hotkey z --client-tty '#{{client_tty}}' && touch {}",
        shell_escape_path(&harness.tmux_tmpdir),
        shell_escape_path(&harness.tmux_socket_path),
        shell_escape_path(&harness.agentscan_socket_path),
        shell_escape_path(&harness.cache_home),
        shell_escape_path(&harness.home_dir),
        shell_escape_path(&agentscan_bin()?),
        shell_escape_path(&done_path),
    );

    harness.tmux([
        "bind-key",
        "-T",
        "agentscan-test",
        "z",
        "run-shell",
        &run_shell_command,
    ])?;
    harness.tmux(["switch-client", "-c", &client.tty, "-T", "agentscan-test"])?;
    harness.send_keys_to_client(&client.tty, ["z"])?;
    harness.wait_for_path(&done_path)?;

    assert!(
        !harness.pane_in_mode(&root_pane_id)?,
        "tmux hotkey failures should use display-message instead of command output view"
    );
    client.ensure_running()?;

    Ok(())
}

#[test]
fn focus_refresh_bypasses_daemon_socket() -> Result<()> {
    let harness = TestHarness::new()?;
    let _root_pane_id = harness.start_session("focus-refresh-bypass-daemon", "sleep 300")?;
    let split_pane_id = harness.split_window("focus-refresh-bypass-daemon:0.0", "sleep 300")?;
    let mut client = harness.attach_client("focus-refresh-bypass-daemon")?;
    fs::write(&harness.agentscan_socket_path, b"not a socket")
        .context("failed to poison daemon socket path")?;

    harness.agentscan([
        "focus",
        "--client-tty",
        &client.tty,
        &split_pane_id,
        "--refresh",
    ])?;
    harness.wait_for_client_pane(&mut client, &split_pane_id)?;

    Ok(())
}

#[test]
fn env_no_auto_start_does_not_disable_daemon_start_command() -> Result<()> {
    let harness = TestHarness::new()?;
    if !harness.detached_daemon_start_supported()? {
        return Ok(());
    }
    let _pane_id = harness.start_session("env-no-auto-start-daemon-start", "sleep 300")?;

    let output = harness
        .agentscan_command()?
        .args(["daemon", "start"])
        .env("AGENTSCAN_NO_AUTO_START", "1")
        .output()
        .context("failed to run daemon start with env opt-out")?;
    assert!(
        output.status.success(),
        "daemon start should ignore consumer opt-out; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).context("daemon start output should be UTF-8")?;
    assert!(
        stdout.contains("agentscan daemon started"),
        "expected daemon start confirmation, got:\n{stdout}"
    );

    harness.agentscan(["daemon", "stop"])?;
    Ok(())
}

#[test]
fn tui_focuses_selected_pane_from_interactive_tmux_pane() -> Result<()> {
    let harness = TestHarness::new()?;
    let root_pane_id = harness.start_session("tui-focus", "sleep 300")?;
    let split_pane_id = harness.split_window("tui-focus:0.0", "sleep 300")?;
    harness.agentscan([
        "tmux",
        "set-metadata",
        "--pane-id",
        &root_pane_id,
        "--provider",
        "codex",
        "--label",
        "Root Task",
        "--state",
        "idle",
    ])?;
    harness.agentscan([
        "tmux",
        "set-metadata",
        "--pane-id",
        &split_pane_id,
        "--provider",
        "claude",
        "--label",
        "Split Task",
        "--state",
        "busy",
    ])?;
    let mut daemon = harness.start_daemon()?;
    harness.wait_for_daemon_pane(&mut daemon, &split_pane_id, |_| true)?;
    let mut client = harness.attach_client("tui-focus")?;

    let tui_pane_id = harness.start_agentscan_tui_pane("tui-focus:0.0", &[])?;
    harness.wait_for_pane_contents(&tui_pane_id, |contents| contents.contains("Split Task"))?;
    harness.tmux(["send-keys", "-t", &tui_pane_id, "2"])?;
    harness.wait_for_client_pane(&mut client, &split_pane_id)?;

    Ok(())
}

#[test]
fn display_popup_focuses_selected_pane_from_attached_client() -> Result<()> {
    let harness = TestHarness::new()?;
    if !harness.supports_display_popup_key_injection()? {
        return Ok(());
    }
    let root_pane_id = harness.start_session("display-tui-focus", "sleep 300")?;
    let split_pane_id = harness.split_window("display-tui-focus:0.0", "sleep 300")?;
    harness.seed_tui_two_pane_metadata(&root_pane_id, &split_pane_id)?;
    let mut daemon = harness.start_daemon()?;
    harness.wait_for_daemon_pane(&mut daemon, &split_pane_id, |_| true)?;
    harness.agentscan(["snapshot", "--format", "json"])?;
    let mut client = harness.attach_client("display-tui-focus")?;

    let mut display_popup = harness.start_agentscan_display_popup(&client.tty, &[])?;
    display_popup.wait_until_ready()?;
    sleep(Duration::from_millis(300));
    harness.send_keys_to_client(&client.tty, ["2"])?;
    display_popup.wait_for_exit()?;
    harness.wait_for_client_pane(&mut client, &split_pane_id)?;

    Ok(())
}

#[test]
fn tui_displays_message_when_selected_pane_no_longer_exists() -> Result<()> {
    let harness = TestHarness::new()?;
    let root_pane_id = harness.start_session("tui-missing", "sleep 300")?;
    let split_pane_id = harness.split_window("tui-missing:0.0", "sleep 300")?;
    harness.agentscan([
        "tmux",
        "set-metadata",
        "--pane-id",
        &root_pane_id,
        "--provider",
        "codex",
        "--label",
        "Root Task",
        "--state",
        "idle",
    ])?;
    harness.agentscan([
        "tmux",
        "set-metadata",
        "--pane-id",
        &split_pane_id,
        "--provider",
        "claude",
        "--label",
        "Split Task",
        "--state",
        "busy",
    ])?;
    harness.tmux(["kill-pane", "-t", &split_pane_id])?;
    let mut daemon = harness.start_daemon()?;
    harness.wait_for_daemon_pane(&mut daemon, &root_pane_id, |_| true)?;
    let mut client = harness.attach_client("tui-missing")?;

    let tui_pane_id = harness.start_agentscan_tui_pane("tui-missing:0.0", &[])?;
    harness.wait_for_pane_contents(&tui_pane_id, |contents| contents.contains("Root Task"))?;
    harness.tmux(["send-keys", "-t", &tui_pane_id, "2"])?;
    harness.wait_for_client_pane(&mut client, &root_pane_id)?;

    Ok(())
}

#[test]
fn display_popup_closes_when_selected_pane_no_longer_exists() -> Result<()> {
    let harness = TestHarness::new()?;
    if !harness.supports_display_popup_key_injection()? {
        return Ok(());
    }
    let root_pane_id = harness.start_session("display-tui-missing", "sleep 300")?;
    let split_pane_id = harness.split_window("display-tui-missing:0.0", "sleep 300")?;
    harness.seed_tui_two_pane_metadata(&root_pane_id, &split_pane_id)?;
    let mut daemon = harness.start_daemon()?;
    harness.wait_for_daemon_pane(&mut daemon, &split_pane_id, |_| true)?;
    harness.agentscan(["snapshot", "--format", "json"])?;
    let mut client = harness.attach_client("display-tui-missing")?;

    let mut display_popup = harness.start_agentscan_display_popup(&client.tty, &[])?;
    display_popup.wait_until_ready()?;
    sleep(Duration::from_millis(300));
    harness.tmux(["kill-pane", "-t", &split_pane_id])?;
    harness.send_keys_to_client(&client.tty, ["2"])?;
    display_popup.wait_for_exit()?;
    harness.wait_for_client_pane(&mut client, &root_pane_id)?;

    Ok(())
}

#[test]
fn tui_ctrl_b_passthrough_returns_to_tmux_prefix_table() -> Result<()> {
    let harness = TestHarness::new()?;
    let _pane_id = harness.start_session("tui-prefix", "sleep 300")?;
    let _daemon = harness.start_daemon()?;
    let client = harness.attach_client("tui-prefix")?;

    let tui_pane_id = harness.start_agentscan_tui_pane("tui-prefix:0.0", &[])?;
    sleep(Duration::from_millis(200));
    harness.tmux(["send-keys", "-t", &tui_pane_id, "C-b"])?;
    harness.wait_for_client_key_table(&client.tty, "prefix")?;
    assert!(harness.pane_exists(&tui_pane_id)?);
    harness.tmux(["send-keys", "-t", &tui_pane_id, "Escape"])?;
    harness.wait_for_pane_closed(&tui_pane_id)?;

    Ok(())
}

#[test]
fn display_popup_ctrl_b_passthrough_returns_to_tmux_prefix_table() -> Result<()> {
    let harness = TestHarness::new()?;
    if !harness.supports_display_popup_key_injection()? {
        return Ok(());
    }
    let _pane_id = harness.start_session("display-tui-prefix", "sleep 300")?;
    let _daemon = harness.start_daemon()?;
    let client = harness.attach_client("display-tui-prefix")?;

    let mut display_popup = harness.start_agentscan_display_popup(&client.tty, &[])?;
    display_popup.wait_until_ready()?;
    harness.send_keys_to_client(&client.tty, ["C-b"])?;
    harness.wait_for_client_key_table(&client.tty, "prefix")?;
    harness.send_keys_to_client(&client.tty, ["Escape"])?;
    display_popup.wait_for_exit()?;

    Ok(())
}

#[test]
fn tui_bootstraps_from_socket() -> Result<()> {
    let harness = TestHarness::new()?;
    let pane_id = harness.start_session("tui-socket-bootstrap", "sleep 300")?;
    harness.agentscan([
        "tmux",
        "set-metadata",
        "--pane-id",
        &pane_id,
        "--provider",
        "claude",
        "--label",
        "Socket Task",
        "--state",
        "busy",
    ])?;
    let mut daemon = harness.start_daemon()?;
    harness.wait_for_daemon_pane(&mut daemon, &pane_id, |_| true)?;

    let tui_pane_id = harness.start_agentscan_tui_pane("tui-socket-bootstrap:0.0", &[])?;
    harness.wait_for_pane_contents(&tui_pane_id, |contents| contents.contains("Socket Task"))?;
    let contents = harness.capture_pane(&tui_pane_id)?;

    assert!(
        contents.contains("[live]"),
        "expected live socket indicator, got:\n{contents}"
    );
    assert!(
        !contents.contains("agentscan tui unavailable"),
        "TUI should use socket bootstrap, got:\n{contents}"
    );

    harness.tmux(["send-keys", "-t", &tui_pane_id, "Escape"])?;
    harness.wait_for_pane_closed(&tui_pane_id)?;
    Ok(())
}

#[test]
fn tui_rerenders_when_socket_snapshot_changes() -> Result<()> {
    let harness = TestHarness::new()?;
    let pane_id = harness.start_session("tui-rerender", "sleep 300")?;
    harness.agentscan([
        "tmux",
        "set-metadata",
        "--pane-id",
        &pane_id,
        "--provider",
        "claude",
        "--label",
        "Initial Task",
        "--state",
        "busy",
    ])?;
    let mut daemon = harness.start_daemon()?;
    harness.wait_for_daemon_pane(&mut daemon, &pane_id, |_| true)?;

    let tui_pane_id = harness.start_agentscan_tui_pane("tui-rerender:0.0", &[])?;
    harness.wait_for_pane_contents(&tui_pane_id, |contents| contents.contains("Initial Task"))?;

    harness.agentscan([
        "tmux",
        "set-metadata",
        "--pane-id",
        &pane_id,
        "--provider",
        "claude",
        "--label",
        "Updated Task",
        "--state",
        "busy",
    ])?;
    harness.wait_for_daemon_pane(&mut daemon, &pane_id, |pane| {
        pane["display"]["label"] == "Updated Task"
    })?;
    harness.wait_for_pane_contents(&tui_pane_id, |contents| contents.contains("Updated Task"))?;

    harness.tmux(["send-keys", "-t", &tui_pane_id, "Escape"])?;
    harness.wait_for_pane_closed(&tui_pane_id)?;

    Ok(())
}

#[test]
fn tui_reconnects_after_post_bootstrap_daemon_eof() -> Result<()> {
    let harness = TestHarness::new()?;
    if !harness.detached_daemon_start_supported()? {
        return Ok(());
    }
    let pane_id = harness.start_session("tui-reconnect", "sleep 300")?;
    harness.agentscan([
        "tmux",
        "set-metadata",
        "--pane-id",
        &pane_id,
        "--provider",
        "claude",
        "--label",
        "Reconnect Task",
        "--state",
        "busy",
    ])?;
    let mut daemon = harness.start_daemon()?;
    harness.wait_for_daemon_snapshot(&mut daemon, |snapshot| {
        pane_from_snapshot(snapshot, &pane_id).is_some()
    })?;

    let tui_pane_id = harness.start_agentscan_tui_pane("tui-reconnect:0.0", &[])?;
    harness.wait_for_pane_contents(&tui_pane_id, |contents| {
        contents.contains("Reconnect Task") && contents.contains("[live]")
    })?;

    daemon.shutdown()?;
    harness.wait_for_pane_contents(&tui_pane_id, |contents| {
        contents.contains("Reconnect Task")
            && (contents.contains("[reconnecting]") || contents.contains("[live]"))
    })?;
    harness.wait_for_pane_contents(&tui_pane_id, |contents| {
        contents.contains("Reconnect Task") && contents.contains("[live]")
    })?;

    harness.tmux(["send-keys", "-t", &tui_pane_id, "Escape"])?;
    harness.wait_for_pane_closed(&tui_pane_id)?;
    Ok(())
}

#[test]
fn tui_ignores_non_selection_keys_until_escape() -> Result<()> {
    let harness = TestHarness::new()?;
    let _pane_id = harness.start_session("tui-ignore", "sleep 300")?;
    let _daemon = harness.start_daemon()?;

    let tui_pane_id = harness.start_agentscan_tui_pane("tui-ignore:0.0", &[])?;
    sleep(Duration::from_millis(200));
    harness.tmux(["send-keys", "-t", &tui_pane_id, "A"])?;
    sleep(Duration::from_millis(200));
    assert!(harness.pane_exists(&tui_pane_id)?);

    harness.tmux(["send-keys", "-t", &tui_pane_id, "Escape"])?;
    harness.wait_for_pane_closed(&tui_pane_id)?;

    Ok(())
}

#[test]
fn tui_ctrl_c_closes() -> Result<()> {
    let harness = TestHarness::new()?;
    let _pane_id = harness.start_session("tui-ctrl-c", "sleep 300")?;
    let _daemon = harness.start_daemon()?;

    let tui_pane_id = harness.start_agentscan_tui_pane("tui-ctrl-c:0.0", &[])?;
    sleep(Duration::from_millis(200));
    harness.tmux(["send-keys", "-t", &tui_pane_id, "C-c"])?;
    harness.wait_for_pane_closed(&tui_pane_id)?;

    Ok(())
}

#[test]
fn tui_pages_to_overflow_rows() -> Result<()> {
    let harness = TestHarness::new()?;
    let root_pane_id = harness.start_session("tui-paging", "sleep 300")?;
    let mut pane_ids = vec![root_pane_id.clone()];

    for _ in 1..18 {
        pane_ids.push(harness.new_window("tui-paging", "sleep 300")?);
    }

    for (index, pane_id) in pane_ids.iter().enumerate() {
        harness.agentscan([
            "tmux",
            "set-metadata",
            "--pane-id",
            pane_id,
            "--provider",
            "claude",
            "--label",
            &format!("Task {:02}", index + 1),
            "--state",
            "busy",
        ])?;
    }
    let mut daemon = harness.start_daemon()?;
    harness.wait_for_daemon_pane(&mut daemon, &root_pane_id, |_| true)?;

    let _client = harness.attach_client("tui-paging")?;
    let tui_pane_id = harness.start_agentscan_tui_pane("tui-paging:0.0", &[])?;

    harness.wait_for_pane_contents(&tui_pane_id, |contents| contents.contains("Page 1/2"))?;
    harness.tmux(["send-keys", "-t", &tui_pane_id, "Right"])?;
    harness.wait_for_pane_contents(&tui_pane_id, |contents| {
        contents.contains("Page 2/2") && contents.contains("Task 17")
    })?;
    harness.tmux(["send-keys", "-t", &tui_pane_id, "Escape"])?;
    harness.wait_for_pane_closed(&tui_pane_id)?;

    Ok(())
}

#[test]
fn display_popup_pages_to_overflow_rows_and_focuses_selection() -> Result<()> {
    let harness = TestHarness::new()?;
    if !harness.supports_display_popup_key_injection()? {
        return Ok(());
    }
    let root_pane_id = harness.start_session("display-tui-paging", "sleep 300")?;
    let mut pane_ids = vec![root_pane_id];

    for _ in 1..18 {
        pane_ids.push(harness.new_window("display-tui-paging", "sleep 300")?);
    }

    for (index, pane_id) in pane_ids.iter().enumerate() {
        harness.agentscan([
            "tmux",
            "set-metadata",
            "--pane-id",
            pane_id,
            "--provider",
            "claude",
            "--label",
            &format!("Task {:02}", index + 1),
            "--state",
            "busy",
        ])?;
    }
    let mut daemon = harness.start_daemon()?;
    harness.wait_for_daemon_pane(&mut daemon, &pane_ids[16], |_| true)?;
    harness.agentscan(["snapshot", "--format", "json"])?;
    let target_pane_id = pane_ids[16].clone();
    let mut client = harness.attach_client("display-tui-paging")?;

    let mut display_popup = harness.start_agentscan_display_popup(&client.tty, &[])?;
    display_popup.wait_until_ready()?;
    sleep(Duration::from_millis(300));
    harness.send_keys_to_client(&client.tty, ["n"])?;
    sleep(Duration::from_millis(200));
    harness.send_keys_to_client(&client.tty, ["1"])?;
    display_popup.wait_for_exit()?;
    harness.wait_for_client_pane(&mut client, &target_pane_id)?;

    Ok(())
}

#[test]
fn tmux_version_parser_handles_prefixed_development_versions() {
    assert_eq!(parse_tmux_version("3.6a"), Some((3, 6)));
    assert_eq!(parse_tmux_version("next-3.6"), Some((3, 6)));
    assert_eq!(parse_tmux_version("tmux next-3.7"), Some((3, 7)));
    assert_eq!(parse_tmux_version("unknown"), None);
}

#[test]
fn focus_uses_attached_client_fallback_when_no_tty_is_given() -> Result<()> {
    let harness = TestHarness::new()?;
    let _root_pane_id = harness.start_session("focus-fallback", "sleep 300")?;
    let split_pane_id = harness.split_window("focus-fallback:0.0", "sleep 300")?;
    let mut client = harness.attach_client("focus-fallback")?;

    harness.agentscan(["-f", "focus", &split_pane_id])?;
    harness.wait_for_client_pane(&mut client, &split_pane_id)?;

    Ok(())
}

#[test]
fn focus_prefers_most_recent_attached_client_when_multiple_are_present() -> Result<()> {
    let harness = TestHarness::new()?;
    let older_root_pane_id = harness.start_session("focus-multi-older", "sleep 300")?;
    let _newer_root_pane_id = harness.start_session("focus-multi-newer", "sleep 300")?;
    let split_pane_id = harness.split_window("focus-multi-newer:0.0", "sleep 300")?;
    let older_client = harness.attach_client("focus-multi-older")?;
    let mut newer_client = harness.attach_client("focus-multi-newer")?;

    harness.agentscan(["-f", "focus", &split_pane_id])?;
    harness.wait_for_client_pane(&mut newer_client, &split_pane_id)?;

    let older_pane_id = harness
        .client_pane_id(&older_client.tty)?
        .context("older attached client disappeared before verification")?;
    assert_eq!(older_pane_id, older_root_pane_id);

    Ok(())
}

#[test]
fn daemon_updates_socket_snapshot_when_panes_are_added() -> Result<()> {
    let harness = TestHarness::new()?;
    let root_pane_id = harness.start_session("pane-add", "sh")?;
    let mut daemon = harness.start_daemon()?;

    harness.wait_for_daemon_pane(&mut daemon, &root_pane_id, |_| true)?;

    let split_pane_id = harness.split_window("pane-add:0.0", "sleep 300")?;
    harness.wait_for_daemon_pane(&mut daemon, &split_pane_id, |pane| {
        pane["pane_id"].as_str() == Some(split_pane_id.as_str())
    })?;

    daemon.shutdown()?;
    Ok(())
}

#[test]
fn daemon_updates_socket_snapshot_when_panes_are_removed() -> Result<()> {
    let harness = TestHarness::new()?;
    let root_pane_id = harness.start_session("pane-remove", "sh")?;
    let split_pane_id = harness.split_window("pane-remove:0.0", "sleep 300")?;
    let mut daemon = harness.start_daemon()?;

    harness.wait_for_daemon_pane(&mut daemon, &root_pane_id, |_| true)?;
    harness.wait_for_daemon_pane(&mut daemon, &split_pane_id, |_| true)?;

    harness.tmux(["kill-pane", "-t", &split_pane_id])?;
    harness.wait_for_daemon_snapshot(&mut daemon, |snapshot| {
        pane_from_snapshot(snapshot, &split_pane_id).is_none()
    })?;

    daemon.shutdown()?;
    Ok(())
}

#[test]
fn daemon_survives_when_attached_session_is_removed_but_server_remains() -> Result<()> {
    let harness = TestHarness::new()?;
    let surviving_pane_id = harness.start_session("surviving-session", "sleep 300")?;
    let attached_pane_id = harness.start_session("zz-removed-session", "sleep 300")?;
    let mut daemon = harness.start_daemon()?;

    harness.wait_for_daemon_pane(&mut daemon, &attached_pane_id, |_| true)?;
    harness.wait_for_daemon_pane(&mut daemon, &surviving_pane_id, |_| true)?;

    harness.tmux(["kill-session", "-t", "zz-removed-session"])?;
    harness.wait_for_daemon_pane(&mut daemon, &surviving_pane_id, |_| true)?;

    daemon.shutdown()?;
    Ok(())
}

#[test]
fn daemon_keeps_processing_events_after_a_subscribed_session_is_killed() -> Result<()> {
    // A killed non-primary session's control client emits `%exit`. That must not
    // be treated as a server-wide exit that stops the daemon loop: the daemon has
    // to keep delivering events for the remaining and future sessions.
    let harness = TestHarness::new()?;
    let primary_pane_id = harness.start_session("survivor-primary", "sh")?;
    let mut daemon = harness.start_daemon()?;
    harness.wait_for_daemon_pane(&mut daemon, &primary_pane_id, |_| true)?;

    // A runtime session gets its own subscriber client; killing it emits `%exit`
    // on that subscriber's stream.
    let doomed_pane_id = harness.start_session("survivor-doomed", "sh")?;
    harness.wait_for_daemon_pane(&mut daemon, &doomed_pane_id, |_| true)?;
    harness.tmux(["kill-session", "-t", "survivor-doomed"])?;
    harness.wait_for_daemon_snapshot(&mut daemon, |snapshot| {
        pane_from_snapshot(snapshot, &doomed_pane_id).is_none()
    })?;

    // Prove the event loop is still alive: a session created after the kill must
    // still be picked up, and an in-place metadata write on it (no topology
    // notification) must still propagate through its newly-attached subscriber.
    let later_pane_id = harness.start_session("survivor-later", "sh")?;
    harness.wait_for_daemon_pane(&mut daemon, &later_pane_id, |_| true)?;
    harness.agentscan([
        "tmux",
        "set-metadata",
        "--pane-id",
        &later_pane_id,
        "--provider",
        "codex",
        "--label",
        "Still Alive",
        "--state",
        "busy",
    ])?;
    harness.wait_for_daemon_pane(&mut daemon, &later_pane_id, |pane| {
        pane["provider"] == "codex"
            && pane["status"]["kind"] == "busy"
            && pane["status"]["source"] == "pane_metadata"
    })?;

    daemon.shutdown()?;
    Ok(())
}

#[test]
fn daemon_updates_socket_snapshot_when_sessions_are_added_and_removed() -> Result<()> {
    let harness = TestHarness::new()?;
    let root_pane_id = harness.start_session("session-root", "sleep 300")?;
    let mut daemon = harness.start_daemon()?;

    harness.wait_for_daemon_pane(&mut daemon, &root_pane_id, |_| true)?;

    let added_pane_id = harness.start_session("session-added", "sleep 300")?;
    harness.wait_for_daemon_pane(&mut daemon, &added_pane_id, |_| true)?;

    harness.tmux(["kill-session", "-t", "session-added"])?;
    harness.wait_for_daemon_snapshot(&mut daemon, |snapshot| {
        pane_from_snapshot(snapshot, &added_pane_id).is_none()
    })?;

    daemon.shutdown()?;
    Ok(())
}

#[test]
fn daemon_updates_socket_snapshot_when_windows_are_added_and_removed() -> Result<()> {
    let harness = TestHarness::new()?;
    let root_pane_id = harness.start_session("window-root", "sleep 300")?;
    let mut daemon = harness.start_daemon()?;

    harness.wait_for_daemon_pane(&mut daemon, &root_pane_id, |_| true)?;

    let added_pane_id = harness.new_window("window-root", "sleep 300")?;
    harness.wait_for_daemon_pane(&mut daemon, &added_pane_id, |_| true)?;

    harness.tmux(["kill-window", "-t", "window-root:1"])?;
    harness.wait_for_daemon_snapshot(&mut daemon, |snapshot| {
        pane_from_snapshot(snapshot, &added_pane_id).is_none()
    })?;

    daemon.shutdown()?;
    Ok(())
}

#[test]
fn daemon_updates_socket_snapshot_when_session_is_renamed() -> Result<()> {
    let harness = TestHarness::new()?;
    let pane_id = harness.start_session("rename-session", "sleep 300")?;
    let mut daemon = harness.start_daemon()?;

    harness.wait_for_daemon_pane(&mut daemon, &pane_id, |_| true)?;

    harness.tmux(["rename-session", "-t", "rename-session", "renamed-session"])?;
    harness.wait_for_daemon_pane(&mut daemon, &pane_id, |pane| {
        pane["location"]["session_name"] == "renamed-session"
    })?;

    daemon.shutdown()?;
    Ok(())
}

#[test]
fn daemon_updates_socket_snapshot_when_window_is_renamed() -> Result<()> {
    let harness = TestHarness::new()?;
    let pane_id = harness.start_session("rename-window", "sleep 300")?;
    let mut daemon = harness.start_daemon()?;

    harness.wait_for_daemon_pane(&mut daemon, &pane_id, |_| true)?;

    harness.tmux(["rename-window", "-t", "rename-window:0", "ai"])?;
    harness.wait_for_daemon_pane(&mut daemon, &pane_id, |pane| {
        pane["location"]["window_name"] == "ai"
    })?;

    daemon.shutdown()?;
    Ok(())
}

#[test]
fn daemon_exits_when_tmux_server_disappears() -> Result<()> {
    let harness = TestHarness::new()?;
    let _pane_id = harness.start_session("server-exit", "sleep 300")?;
    let mut daemon = harness.start_daemon()?;

    harness.wait_for_daemon_snapshot(&mut daemon, |_| true)?;
    harness.tmux(["kill-server"])?;
    daemon.wait_for_exit(DAEMON_TIMEOUT)?;

    Ok(())
}

fn connect_agentscan_socket(socket_path: &Path) -> Result<UnixStream> {
    let deadline = Instant::now() + DAEMON_TIMEOUT;
    loop {
        match UnixStream::connect(socket_path) {
            Ok(stream) => return Ok(stream),
            Err(error) if Instant::now() < deadline => {
                if !matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) {
                    return Err(error).with_context(|| {
                        format!("failed to connect to {}", socket_path.display())
                    });
                }
                sleep(POLL_INTERVAL);
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "timed out waiting for daemon socket at {}",
                        socket_path.display()
                    )
                });
            }
        }
    }
}

fn read_daemon_socket_json_line(reader: &mut impl BufRead) -> Result<Value> {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .context("failed to read daemon socket frame")?;
    if line.is_empty() {
        bail!("daemon socket closed before sending expected frame");
    }
    serde_json::from_str(&line).context("daemon socket frame was not valid JSON")
}

fn read_subscribe_stream_until(
    reader: &mut impl BufRead,
    matches: impl Fn(&Value) -> bool,
) -> Result<Value> {
    let deadline = Instant::now() + DAEMON_TIMEOUT;
    let mut last_frame = Value::Null;
    let mut line = String::new();
    while Instant::now() < deadline {
        match reader.read_line(&mut line) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                sleep(POLL_INTERVAL);
                continue;
            }
            Err(error) => return Err(error).context("failed to read subscribe stream frame"),
        }
        if line.is_empty() {
            bail!("subscribe stream closed before expected frame; last frame: {last_frame}");
        }
        let frame: Value = serde_json::from_str(&line)
            .with_context(|| format!("subscribe stream frame was not JSON: {line:?}"))?;
        if matches(&frame) {
            return Ok(frame);
        }
        last_frame = frame;
        line.clear();
    }
    bail!("timed out waiting for subscribe stream frame; last frame: {last_frame}");
}

fn set_fd_nonblocking(fd: std::os::fd::RawFd) -> Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error()).context("failed to get file descriptor flags");
    }
    let result = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if result < 0 {
        return Err(std::io::Error::last_os_error()).context("failed to set nonblocking pipe");
    }
    Ok(())
}

fn wait_for_child_exit(child: &mut Child, timeout: Duration) -> Result<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().context("failed to poll child process")? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("timed out waiting for child process to exit");
        }
        sleep(POLL_INTERVAL);
    }
}

fn serve_fake_snapshot(socket_path: &Path, snapshot: Value) -> FakeSnapshotServer {
    validate_snapshot_json(&snapshot).expect("fake daemon snapshot should validate");
    let listener = UnixListener::bind(socket_path).expect("fake snapshot daemon should bind");
    listener
        .set_nonblocking(true)
        .expect("fake snapshot daemon listener should be nonblocking");
    let protocol_version = daemon_protocol_version();
    let schema_version = snapshot_schema_version();
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let handle = std::thread::spawn(move || {
        let mut stream = accept_fake_snapshot_connection(&listener, &thread_stop);
        stream
            .set_read_timeout(Some(Duration::from_secs(8)))
            .expect("fake snapshot daemon read timeout should set");
        stream
            .set_write_timeout(Some(Duration::from_secs(8)))
            .expect("fake snapshot daemon write timeout should set");
        let mut hello = String::new();
        BufReader::new(
            stream
                .try_clone()
                .expect("fake snapshot stream should clone"),
        )
        .read_line(&mut hello)
        .expect("snapshot hello should read");
        let hello: Value = serde_json::from_str(&hello).expect("snapshot hello should be JSON");
        assert_eq!(hello["type"], "hello");
        assert_eq!(hello["mode"], "snapshot");
        assert_eq!(hello["protocol_version"], protocol_version);
        assert_eq!(hello["snapshot_schema_version"], schema_version);
        let ack = serde_json::json!({
            "type": "hello_ack",
            "protocol_version": protocol_version,
            "snapshot_schema_version": schema_version
        });
        let frame = serde_json::json!({
            "type": "snapshot",
            "snapshot": snapshot
        });
        writeln!(stream, "{ack}").expect("fake snapshot ack should write");
        writeln!(stream, "{frame}").expect("fake snapshot frame should write");
        stream.flush().expect("fake snapshot frame should flush");
        assert_no_unexpected_fake_snapshot_connections(
            &listener,
            &thread_stop,
            protocol_version,
            schema_version,
        );
    });
    FakeSnapshotServer {
        stop,
        handle: Some(handle),
    }
}

fn write_fake_lifecycle_status(
    stream: &mut UnixStream,
    pid: u32,
    socket_path: &str,
    daemon_start_time: &str,
) {
    let ack = serde_json::json!({
        "type": "hello_ack",
        "protocol_version": daemon_protocol_version(),
        "snapshot_schema_version": snapshot_schema_version()
    });
    let status = serde_json::json!({
        "type": "lifecycle_status",
        "status": {
            "state": "ready",
            "identity": {
                "pid": pid,
                "daemon_start_time": daemon_start_time,
                "executable": "/bin/sh",
                "executable_canonical": null,
                "socket_path": socket_path,
                "protocol_version": daemon_protocol_version(),
                "snapshot_schema_version": snapshot_schema_version()
            },
            "subscriber_count": 0,
            "latest_snapshot_generated_at": null,
            "latest_snapshot_pane_count": null,
            "latest_snapshot_update_source": null,
            "latest_snapshot_update_detail": null,
            "latest_snapshot_update_duration_ms": null,
            "unavailable_reason": null,
            "message": null
        }
    });
    writeln!(stream, "{ack}").expect("fake lifecycle ack should write");
    writeln!(stream, "{status}").expect("fake lifecycle status should write");
    stream.flush().expect("fake lifecycle status should flush");
}

struct FakeSnapshotServer {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl FakeSnapshotServer {
    fn join(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.handle
            .take()
            .expect("fake snapshot daemon should have a join handle")
            .join()
            .expect("fake snapshot daemon should join");
    }
}

impl Drop for FakeSnapshotServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn accept_fake_snapshot_connection(listener: &UnixListener, stop: &AtomicBool) -> UnixStream {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        assert!(
            !stop.load(Ordering::SeqCst),
            "fake snapshot daemon stopped before first connection"
        );
        match listener.accept() {
            Ok((stream, _)) => return stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for fake daemon snapshot connection"
                );
                sleep(POLL_INTERVAL);
            }
            Err(error) => panic!("fake daemon snapshot accept failed: {error}"),
        }
    }
}

fn assert_no_unexpected_fake_snapshot_connections(
    listener: &UnixListener,
    stop: &AtomicBool,
    protocol_version: u32,
    schema_version: u32,
) {
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream
                    .set_read_timeout(Some(Duration::from_secs(8)))
                    .expect("fake snapshot daemon event read timeout should set");
                stream
                    .set_write_timeout(Some(Duration::from_secs(8)))
                    .expect("fake snapshot daemon event write timeout should set");
                let mut line = String::new();
                BufReader::new(
                    stream
                        .try_clone()
                        .expect("fake snapshot event stream should clone"),
                )
                .read_line(&mut line)
                .expect("client event frame should read");
                let frame: Value =
                    serde_json::from_str(&line).expect("client event frame should be JSON");
                assert_eq!(frame["type"], "client_event");
                assert_eq!(frame["protocol_version"], protocol_version);
                assert_eq!(frame["snapshot_schema_version"], schema_version);
                assert_eq!(frame["event"]["kind"], "pane_focus");

                let ack = serde_json::json!({
                    "type": "hello_ack",
                    "protocol_version": protocol_version,
                    "snapshot_schema_version": schema_version
                });
                writeln!(stream, "{ack}").expect("fake client event ack should write");
                stream.flush().expect("fake client event ack should flush");
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => sleep(POLL_INTERVAL),
            Err(error) => panic!("fake daemon snapshot accept failed: {error}"),
        }
    }
}

fn accept_fake_daemon_connection(listener: &UnixListener) -> UnixStream {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        match listener.accept() {
            Ok((stream, _)) => return stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for fake daemon lifecycle connection"
                );
                sleep(POLL_INTERVAL);
            }
            Err(error) => panic!("fake daemon accept failed: {error}"),
        }
    }
}

fn serve_incompatible_lifecycle_once(socket_path: &Path) -> std::thread::JoinHandle<()> {
    let listener = UnixListener::bind(socket_path).expect("fake daemon socket should bind");
    listener
        .set_nonblocking(true)
        .expect("fake daemon listener should be nonblocking");
    std::thread::spawn(move || {
        let mut stream = accept_fake_daemon_connection(&listener);
        let mut request = String::new();
        let _ = BufReader::new(stream.try_clone().expect("stream should clone"))
            .read_line(&mut request);
        let frame = serde_json::json!({
            "type": "shutdown",
            "reason": "schema_mismatch",
            "message": format!(
                "unsupported snapshot schema version {} (expected {})",
                snapshot_schema_version(),
                snapshot_schema_version() - 1
            )
        });
        writeln!(stream, "{frame}").expect("schema mismatch frame should write");
        stream.flush().expect("schema mismatch frame should flush");
    })
}

fn start_fake_incompatible_daemon(
    socket_path: &Path,
    daemon_snapshot_schema_version: u32,
) -> Result<u32> {
    start_fake_incompatible_daemon_with_options(
        socket_path,
        daemon_snapshot_schema_version,
        false,
        false,
    )
}

fn start_fake_incompatible_daemon_with_legacy_identity_sidecar(
    socket_path: &Path,
    daemon_snapshot_schema_version: u32,
) -> Result<u32> {
    start_fake_incompatible_daemon_with_options(
        socket_path,
        daemon_snapshot_schema_version,
        false,
        true,
    )
}

fn start_fake_incompatible_daemon_with_options(
    socket_path: &Path,
    daemon_snapshot_schema_version: u32,
    cleanup_on_sigterm: bool,
    legacy_identity_sidecar: bool,
) -> Result<u32> {
    let ready_pid_path = socket_path.with_extension("fake-incompatible-daemon.pid");
    let lock_path = socket_path.with_extension("sock.lock");
    let identity_path = socket_path.with_extension("sock.identity.json");
    let test_binary = std::env::current_exe().context("failed to resolve current test binary")?;
    let launch_status = Command::new("sh")
        .arg("-c")
        .arg(
            "exec \"$FAKE_TEST_BIN\" --ignored --exact fake_incompatible_daemon_process --nocapture >/dev/null 2>&1 </dev/null &",
        )
        .env("FAKE_TEST_BIN", &test_binary)
        .env("FAKE_DAEMON_SOCKET", socket_path)
        .env("FAKE_DAEMON_LOCK", &lock_path)
        .env("FAKE_DAEMON_IDENTITY", &identity_path)
        .env("FAKE_DAEMON_READY", &ready_pid_path)
        .env("FAKE_DAEMON_PROTOCOL", daemon_protocol_version().to_string())
        .env("FAKE_DAEMON_CLIENT_SCHEMA", snapshot_schema_version().to_string())
        .env("FAKE_DAEMON_SCHEMA", daemon_snapshot_schema_version.to_string())
        .env(
            "FAKE_DAEMON_CLEANUP_ON_SIGTERM",
            if cleanup_on_sigterm { "1" } else { "0" },
        )
        .env(
            "FAKE_DAEMON_LEGACY_IDENTITY",
            if legacy_identity_sidecar { "1" } else { "0" },
        )
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("failed to launch fake incompatible daemon")?;
    assert!(
        launch_status.success(),
        "failed to launch fake incompatible daemon: {launch_status}"
    );
    let ready_pid = wait_for_pid_file(&ready_pid_path)?;
    Ok(ready_pid)
}

fn launch_background_sleep(pid_path: &Path) -> Result<u32> {
    let launch_script =
        "sh -c 'echo $$ > \"$PID_PATH\"; exec sleep 300' >/dev/null 2>&1 </dev/null &";
    let launch_status = Command::new("sh")
        .env("PID_PATH", pid_path)
        .args(["-c", launch_script])
        .status()
        .context("failed to launch background sleep")?;
    assert!(
        launch_status.success(),
        "failed to launch background sleep: {launch_status}"
    );
    wait_for_pid_file(pid_path)
}

fn write_daemon_identity_sidecar(
    socket_path: &Path,
    pid: u32,
    snapshot_schema_version: u32,
) -> Result<()> {
    let sleep_path = command_path("sleep")?;
    let executable_canonical = fs::canonicalize(&sleep_path)
        .ok()
        .map(|path| path.display().to_string());
    let identity = serde_json::json!({
        "pid": pid,
        "daemon_start_time": "2026-05-03T00:00:00Z",
        "executable": sleep_path.display().to_string(),
        "executable_canonical": executable_canonical,
        "socket_path": socket_path.display().to_string(),
        "protocol_version": daemon_protocol_version(),
        "snapshot_schema_version": snapshot_schema_version
    });
    let identity_path = socket_path.with_extension("sock.identity.json");
    fs::write(
        &identity_path,
        serde_json::to_vec_pretty(&identity).context("failed to encode daemon identity")?,
    )
    .with_context(|| {
        format!(
            "failed to write daemon identity {}",
            identity_path.display()
        )
    })
}

fn command_path(command: &str) -> Result<PathBuf> {
    let output = Command::new("sh")
        .args(["-c", &format!("command -v {command}")])
        .output()
        .with_context(|| format!("failed to resolve command path for {command}"))?;
    if !output.status.success() {
        bail!("failed to resolve command path for {command}");
    }
    let path = String::from_utf8(output.stdout).context("command path was not valid UTF-8")?;
    let path = path.trim();
    if path.is_empty() {
        bail!("empty command path for {command}");
    }
    Ok(PathBuf::from(path))
}

fn process_is_live_for_test(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

struct KillPidGuard {
    pid: u32,
    active: bool,
}

impl KillPidGuard {
    fn new(pid: u32) -> Self {
        Self { pid, active: true }
    }

    fn disarm(&mut self) {
        self.active = false;
    }
}

impl Drop for KillPidGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = unsafe { libc::kill(self.pid as libc::pid_t, libc::SIGKILL) };
        }
    }
}

fn wait_for_pid_file(path: &Path) -> Result<u32> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match fs::read_to_string(path) {
            Ok(pid) => {
                if let Ok(pid) = pid.trim().parse::<u32>() {
                    return Ok(pid);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to read pid file {}", path.display()));
            }
        }
        if Instant::now() >= deadline {
            bail!("timed out waiting for pid file {}", path.display());
        }
        sleep(POLL_INTERVAL);
    }
}

fn lifecycle_status_value<'a>(status: &'a str, key: &str) -> Option<&'a str> {
    status.lines().find_map(|line| {
        line.strip_prefix(key)
            .and_then(|tail| tail.strip_prefix(": "))
            .map(str::trim)
    })
}

fn daemon_protocol_version() -> u32 {
    agentscan::app::bench_support::daemon_protocol_version_for_tests()
}

fn snapshot_schema_version() -> u32 {
    agentscan::app::bench_support::snapshot_schema_version_for_tests()
}

fn validate_snapshot_json(snapshot: &Value) -> Result<()> {
    let snapshot_json =
        serde_json::to_string(snapshot).context("failed to serialize snapshot JSON")?;
    agentscan::app::bench_support::validate_snapshot_json_for_tests(&snapshot_json)
}

fn doctor_check<'a>(report: &'a Value, id: &str) -> Option<&'a Value> {
    report["checks"]
        .as_array()?
        .iter()
        .find(|check| check["id"] == id)
}

#[test]
fn doctor_json_warns_and_reports_schema_when_daemon_not_running() -> Result<()> {
    let harness = TestHarness::new()?;

    // `agentscan_output` bails on a non-zero exit, so a successful call also
    // asserts the report-only exit-0 contract.
    let stdout = harness.agentscan_output(["doctor", "--format", "json"])?;
    let report: Value = serde_json::from_str(&stdout).context("doctor JSON should parse")?;

    assert_eq!(report["schema_version"], 1);
    assert!(
        report["generated_at"]
            .as_str()
            .is_some_and(|at| !at.is_empty())
    );

    let daemon = doctor_check(&report, "daemon.health").context("daemon.health check missing")?;
    assert_eq!(daemon["status"], "warn");
    assert_eq!(daemon["details"]["daemon_state"], "not_running");

    let tmux = doctor_check(&report, "tmux.reachable").context("tmux.reachable check missing")?;
    assert_eq!(tmux["status"], "ok");

    Ok(())
}

#[test]
fn doctor_json_reports_ready_daemon_and_discovery_when_running() -> Result<()> {
    let harness = TestHarness::new()?;
    let pane_id = harness.start_session("doctor-ready", "sleep 300")?;
    let mut daemon = harness.start_daemon()?;
    harness.wait_for_daemon_snapshot(&mut daemon, |snapshot| {
        pane_from_snapshot(snapshot, &pane_id).is_some()
    })?;
    harness.agentscan([
        "tmux",
        "set-metadata",
        "--pane-id",
        &pane_id,
        "--provider",
        "codex",
        "--label",
        "Doctor",
        "--state",
        "idle",
    ])?;
    harness.wait_for_daemon_pane(&mut daemon, &pane_id, |pane| pane["provider"] == "codex")?;

    let stdout = harness.agentscan_output(["doctor", "--format", "json"])?;
    let report: Value = serde_json::from_str(&stdout).context("doctor JSON should parse")?;

    let health = doctor_check(&report, "daemon.health").context("daemon.health check missing")?;
    assert_eq!(health["status"], "ok");
    assert_eq!(health["details"]["daemon_state"], "ready");

    let summary =
        doctor_check(&report, "discovery.summary").context("discovery.summary check missing")?;
    assert_eq!(summary["status"], "ok");
    assert_eq!(summary["details"]["source"], "daemon");
    assert!(
        summary["details"]["agent_pane_count"]
            .as_u64()
            .is_some_and(|count| count >= 1),
        "expected at least one agent pane, got:\n{stdout}"
    );
    assert_eq!(summary["details"]["provider_counts"]["codex"], 1);

    Ok(())
}

#[test]
fn doctor_refresh_includes_discovery_compare_when_daemon_running() -> Result<()> {
    let harness = TestHarness::new()?;
    let pane_id = harness.start_session("doctor-refresh", "sleep 300")?;
    let mut daemon = harness.start_daemon()?;
    harness.wait_for_daemon_snapshot(&mut daemon, |snapshot| {
        pane_from_snapshot(snapshot, &pane_id).is_some()
    })?;

    let stdout = harness.agentscan_output(["doctor", "--refresh", "--format", "json"])?;
    let report: Value = serde_json::from_str(&stdout).context("doctor JSON should parse")?;

    let summary =
        doctor_check(&report, "discovery.summary").context("discovery.summary check missing")?;
    assert_eq!(summary["details"]["source"], "direct_tmux");

    let compare =
        doctor_check(&report, "discovery.compare").context("discovery.compare check missing")?;
    assert!(
        compare["status"] == "ok" || compare["status"] == "warn",
        "unexpected discovery.compare status: {compare}"
    );
    assert!(compare["details"]["daemon_pane_count"].as_u64().is_some());

    Ok(())
}

#[test]
fn doctor_reports_invalid_config_as_fail_and_still_exits_zero() -> Result<()> {
    let harness = TestHarness::new()?;
    // Point config resolution at an isolated XDG_CONFIG_HOME so the broken file
    // is read deterministically regardless of the ambient environment.
    let xdg_config_home = harness._tempdir.path().join("xdg-config");
    let config_dir = xdg_config_home.join("agentscan");
    fs::create_dir_all(&config_dir)
        .with_context(|| format!("failed to create {}", config_dir.display()))?;
    fs::write(config_dir.join("config.toml"), "this is = = not valid toml")
        .context("failed to write broken config")?;

    let output = harness
        .agentscan_command()?
        .env("XDG_CONFIG_HOME", &xdg_config_home)
        .args(["doctor", "--format", "json"])
        .output()
        .context("failed to execute agentscan doctor")?;
    assert!(
        output.status.success(),
        "doctor must exit 0 even with invalid config; status: {}; stderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).context("doctor stdout was not valid UTF-8")?;
    let report: Value = serde_json::from_str(&stdout).context("doctor JSON should parse")?;

    let config = doctor_check(&report, "config.valid").context("config.valid check missing")?;
    assert_eq!(config["status"], "fail");
    assert!(
        report["summary"]["fail_count"]
            .as_u64()
            .is_some_and(|count| count >= 1)
    );

    Ok(())
}

#[test]
fn doctor_reports_invalid_runtime_option_as_config_fail() -> Result<()> {
    let harness = TestHarness::new()?;
    // Icons and picker keys are valid; only the runtime toggle is broken. This
    // guards against `config.valid` swallowing runtime-option validation errors.
    let xdg_config_home = harness._tempdir.path().join("xdg-runtime-config");
    let config_dir = xdg_config_home.join("agentscan");
    fs::create_dir_all(&config_dir)
        .with_context(|| format!("failed to create {}", config_dir.display()))?;
    fs::write(
        config_dir.join("config.toml"),
        "icons = \"emoji\"\ndisable_reconcile = \"not-a-bool\"\n",
    )
    .context("failed to write config with invalid runtime option")?;

    let output = harness
        .agentscan_command()?
        .env("XDG_CONFIG_HOME", &xdg_config_home)
        .args(["doctor", "--format", "json"])
        .output()
        .context("failed to execute agentscan doctor")?;
    assert!(
        output.status.success(),
        "doctor must exit 0 even with invalid runtime config; status: {}; stderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).context("doctor stdout was not valid UTF-8")?;
    let report: Value = serde_json::from_str(&stdout).context("doctor JSON should parse")?;
    let config = doctor_check(&report, "config.valid").context("config.valid check missing")?;
    assert_eq!(config["status"], "fail");

    Ok(())
}

#[test]
fn doctor_text_lists_every_check_id() -> Result<()> {
    let harness = TestHarness::new()?;

    let stdout = harness.agentscan_output(["doctor"])?;
    assert!(
        stdout.contains("agentscan doctor"),
        "missing header:\n{stdout}"
    );
    for id in [
        "binary.version",
        "binary.macos_trust",
        "config.valid",
        "tmux.reachable",
        "daemon.health",
        "discovery.summary",
        "picker.contract",
    ] {
        assert!(stdout.contains(id), "missing check {id} in:\n{stdout}");
    }

    Ok(())
}

include!("common/tmux_harness.rs");
