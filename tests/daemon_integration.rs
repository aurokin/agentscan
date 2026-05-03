use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tempfile::TempDir;

const DAEMON_TIMEOUT: Duration = Duration::from_secs(40);
const POLL_INTERVAL: Duration = Duration::from_millis(50);
const CACHE_SNAPSHOT_FIXTURE: &str = include_str!("fixtures/cache_snapshot_v1.json");

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
fn daemon_serves_snapshot_over_owned_socket_path() -> Result<()> {
    let harness = TestHarness::new()?;
    let pane_id = harness.start_session("socket-snapshot", "sleep 300")?;
    let mut daemon = harness.start_daemon()?;
    let cache = harness.wait_for_cache(&mut daemon, |cache| {
        pane_from_cache(cache, &pane_id).is_some()
    })?;
    let schema_version = cache["schema_version"]
        .as_u64()
        .context("daemon cache was missing schema_version")?;

    let mut stream = connect_agentscan_socket(&harness.agentscan_socket_path)?;
    writeln!(
        stream,
        "{}",
        serde_json::json!({
            "type": "hello",
            "protocol_version": 1,
            "snapshot_schema_version": schema_version,
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
    assert_eq!(ack["protocol_version"], 1);
    assert_eq!(ack["snapshot_schema_version"], schema_version);

    let snapshot_frame = read_daemon_socket_json_line(&mut reader)?;
    assert_eq!(snapshot_frame["type"], "snapshot");
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
            OsString::from("AGENTSCAN_CACHE_PATH"),
            harness.cache_path.as_os_str().to_owned(),
        ),
        (
            OsString::from("AGENTSCAN_SOCKET_PATH"),
            harness.agentscan_socket_path.as_os_str().to_owned(),
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
    let cache = harness.wait_for_cache(&mut daemon, |cache| {
        pane_from_cache(cache, &pane_id).is_some()
    })?;
    let schema_version = cache["schema_version"]
        .as_u64()
        .context("daemon cache was missing schema_version")?;

    let mut stream = connect_agentscan_socket(&harness.agentscan_socket_path)?;
    stream
        .set_read_timeout(Some(DAEMON_TIMEOUT))
        .context("failed to set subscriber socket read timeout")?;
    writeln!(
        stream,
        "{}",
        serde_json::json!({
            "type": "hello",
            "protocol_version": 1,
            "snapshot_schema_version": schema_version,
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
fn daemon_lifecycle_start_status_stop() -> Result<()> {
    let harness = TestHarness::new()?;
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
fn metadata_helpers_refresh_existing_snapshot_cache_without_daemon() -> Result<()> {
    let harness = TestHarness::new()?;
    let pane_id = harness.start_session("metadata-cache", "sh")?;
    harness.send_title_escape(&pane_id, "metadata-cache")?;

    harness.agentscan(["-f", "cache", "show"])?;
    harness.wait_for_cache_file(|cache| pane_from_cache(cache, &pane_id).is_some())?;

    harness.agentscan([
        "tmux",
        "set-metadata",
        "--pane-id",
        &pane_id,
        "--provider",
        "claude",
        "--label",
        "Snapshot Wrapper Task",
        "--state",
        "busy",
    ])?;
    harness.wait_for_cache_file(|cache| {
        let Some(pane) = pane_from_cache(cache, &pane_id) else {
            return false;
        };
        pane["provider"] == "claude"
            && pane["display"]["label"] == "Snapshot Wrapper Task"
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
    harness.wait_for_cache_file(|cache| {
        let Some(pane) = pane_from_cache(cache, &pane_id) else {
            return false;
        };
        pane["provider"].is_null()
            && pane["display"]["label"] == "metadata-cache"
            && pane["status"]["kind"] == "unknown"
    })?;

    Ok(())
}

#[test]
fn forced_refresh_preserves_last_daemon_refresh_semantics() -> Result<()> {
    let harness = TestHarness::new()?;
    let _pane_id = harness.start_session("refresh-daemon", "sleep 300")?;
    let mut daemon = harness.start_daemon()?;

    harness.wait_for_cache(&mut daemon, |_| true)?;
    daemon.shutdown()?;
    let daemon_cache = harness.wait_for_cache_file(|cache| cache["source"]["kind"] == "daemon")?;
    let last_daemon_generated_at = daemon_cache["source"]["daemon_generated_at"]
        .as_str()
        .context("daemon cache was missing daemon_generated_at")?
        .to_string();

    sleep(Duration::from_secs(1));
    harness.agentscan(["-f", "cache", "show"])?;
    harness.agentscan(["daemon", "status"])?;

    let refreshed_cache =
        harness.wait_for_cache_file(|cache| cache["source"]["kind"] == "snapshot")?;
    assert_eq!(
        refreshed_cache["source"]["daemon_generated_at"].as_str(),
        Some(last_daemon_generated_at.as_str())
    );
    Ok(())
}

#[test]
fn scan_refresh_preserves_last_daemon_refresh_semantics() -> Result<()> {
    let harness = TestHarness::new()?;
    let _pane_id = harness.start_session("scan-refresh-daemon", "sleep 300")?;
    let mut daemon = harness.start_daemon()?;

    harness.wait_for_cache(&mut daemon, |_| true)?;
    daemon.shutdown()?;
    let daemon_cache = harness.wait_for_cache_file(|cache| cache["source"]["kind"] == "daemon")?;
    let last_daemon_generated_at = daemon_cache["source"]["daemon_generated_at"]
        .as_str()
        .context("daemon cache was missing daemon_generated_at")?
        .to_string();

    sleep(Duration::from_secs(1));
    harness.agentscan(["scan", "-f", "--format", "text"])?;
    harness.agentscan(["daemon", "status"])?;

    let refreshed_cache =
        harness.wait_for_cache_file(|cache| cache["source"]["kind"] == "snapshot")?;
    assert_eq!(
        refreshed_cache["source"]["daemon_generated_at"].as_str(),
        Some(last_daemon_generated_at.as_str())
    );
    Ok(())
}

#[test]
fn metadata_helpers_survive_unrelated_daemon_updates() -> Result<()> {
    let harness = TestHarness::new()?;
    let metadata_pane_id = harness.start_session("metadata-survives", "sh")?;
    let trigger_pane_id = harness.start_session("metadata-trigger", "sh")?;
    harness.send_title_escape(&metadata_pane_id, "metadata-survives")?;
    let mut daemon = harness.start_daemon()?;

    harness.wait_for_pane(&mut daemon, &metadata_pane_id, |_| true)?;
    harness.wait_for_pane(&mut daemon, &trigger_pane_id, |_| true)?;

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
    harness.wait_for_pane(&mut daemon, &metadata_pane_id, |pane| {
        pane["provider"] == "codex"
            && pane["display"]["label"] == "Persistent Metadata"
            && pane["status"]["kind"] == "busy"
    })?;

    harness.send_title_escape(&trigger_pane_id, "Claude Code | Working")?;
    harness.wait_for_pane(&mut daemon, &trigger_pane_id, |pane| {
        pane["provider"] == "claude"
            && pane["status"]["kind"] == "busy"
            && pane["display"]["label"] == "Working"
    })?;
    harness.wait_for_pane(&mut daemon, &metadata_pane_id, |pane| {
        pane["provider"] == "codex"
            && pane["display"]["label"] == "Persistent Metadata"
            && pane["status"]["kind"] == "busy"
    })?;

    daemon.shutdown()?;
    Ok(())
}

#[test]
fn metadata_helpers_rebuild_cache_when_existing_cache_is_invalid() -> Result<()> {
    let harness = TestHarness::new()?;
    let pane_id = harness.start_session("metadata-invalid-cache", "sh")?;
    fs::write(&harness.cache_path, b"{invalid json").context("failed to seed invalid cache")?;

    harness.agentscan([
        "tmux",
        "set-metadata",
        "--pane-id",
        &pane_id,
        "--provider",
        "claude",
        "--label",
        "Recovered Cache",
        "--state",
        "idle",
    ])?;
    harness.wait_for_cache_file(|cache| {
        let Some(pane) = pane_from_cache(cache, &pane_id) else {
            return false;
        };
        pane["provider"] == "claude"
            && pane["display"]["label"] == "Recovered Cache"
            && pane["status"]["kind"] == "idle"
    })?;

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
fn list_no_auto_start_preserves_cache_backed_behavior_before_socket_migration() -> Result<()> {
    let harness = TestHarness::new()?;
    fs::write(&harness.cache_path, CACHE_SNAPSHOT_FIXTURE)
        .context("failed to seed cache fixture")?;
    fs::write(&harness.agentscan_socket_path, b"not a socket")
        .context("failed to poison daemon socket path")?;

    let stdout = harness.agentscan_output(["list", "--no-auto-start", "--format", "json"])?;
    let snapshot: Value = serde_json::from_str(&stdout).context("list output should be JSON")?;

    assert!(
        snapshot["panes"]
            .as_array()
            .context("snapshot panes should be an array")?
            .iter()
            .any(|pane| pane["pane_id"] == "%67" && pane["provider"] == "codex"),
        "list should read the existing cache fixture, got:\n{stdout}"
    );
    Ok(())
}

#[test]
fn inspect_no_auto_start_preserves_cache_backed_behavior_before_socket_migration() -> Result<()> {
    let harness = TestHarness::new()?;
    fs::write(&harness.cache_path, CACHE_SNAPSHOT_FIXTURE)
        .context("failed to seed cache fixture")?;
    fs::write(&harness.agentscan_socket_path, b"not a socket")
        .context("failed to poison daemon socket path")?;

    let stdout =
        harness.agentscan_output(["inspect", "%67", "--no-auto-start", "--format", "json"])?;
    let pane: Value = serde_json::from_str(&stdout).context("inspect output should be JSON")?;

    assert_eq!(pane["pane_id"], "%67");
    assert_eq!(pane["provider"], "codex");
    Ok(())
}

#[test]
fn focus_no_auto_start_refresh_preserves_direct_tmux_behavior_before_socket_migration() -> Result<()>
{
    let harness = TestHarness::new()?;
    let _root_pane_id = harness.start_session("focus-no-auto-start", "sleep 300")?;
    let split_pane_id = harness.split_window("focus-no-auto-start:0.0", "sleep 300")?;
    let mut client = harness.attach_client("focus-no-auto-start")?;
    fs::write(&harness.agentscan_socket_path, b"not a socket")
        .context("failed to poison daemon socket path")?;

    harness.agentscan([
        "focus",
        "--client-tty",
        &client.tty,
        &split_pane_id,
        "--no-auto-start",
        "--refresh",
    ])?;
    harness.wait_for_client_pane(&mut client, &split_pane_id)?;

    Ok(())
}

#[test]
fn env_no_auto_start_preserves_cache_backed_behavior_before_socket_migration() -> Result<()> {
    let harness = TestHarness::new()?;
    fs::write(&harness.cache_path, CACHE_SNAPSHOT_FIXTURE)
        .context("failed to seed cache fixture")?;
    fs::write(&harness.agentscan_socket_path, b"not a socket")
        .context("failed to poison daemon socket path")?;

    let output = harness
        .agentscan_command()?
        .args(["list", "--format", "json"])
        .env("AGENTSCAN_NO_AUTO_START", "1")
        .output()
        .context("failed to run list with env opt-out")?;
    assert!(
        output.status.success(),
        "list should succeed from cache; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).context("list output should be UTF-8")?;
    let snapshot: Value = serde_json::from_str(&stdout).context("list output should be JSON")?;
    assert!(
        snapshot["panes"]
            .as_array()
            .context("snapshot panes should be an array")?
            .iter()
            .any(|pane| pane["pane_id"] == "%67" && pane["provider"] == "codex"),
        "list should read the existing cache fixture, got:\n{stdout}"
    );
    Ok(())
}

#[test]
fn env_no_auto_start_does_not_disable_daemon_start_command() -> Result<()> {
    let harness = TestHarness::new()?;
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
fn cache_commands_reject_root_no_auto_start_before_socket_migration() -> Result<()> {
    let harness = TestHarness::new()?;
    fs::write(&harness.cache_path, CACHE_SNAPSHOT_FIXTURE)
        .context("failed to seed cache fixture")?;

    for args in [
        ["--no-auto-start", "cache", "show"].as_slice(),
        ["--no-auto-start", "cache", "validate"].as_slice(),
    ] {
        let output = harness
            .agentscan_command()?
            .args(args)
            .output()
            .context("failed to run cache command with root no-auto-start")?;
        assert!(
            !output.status.success(),
            "cache command should reject root no-auto-start; args: {args:?}"
        );
        let stderr = String::from_utf8(output.stderr).context("stderr should be UTF-8")?;
        assert!(
            stderr.contains("`--no-auto-start` is not supported"),
            "expected root no-auto-start rejection, got:\n{stderr}"
        );
    }
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
    harness.agentscan(["-f", "cache", "show"])?;
    let mut client = harness.attach_client("tui-focus")?;

    let tui_pane_id = harness.start_agentscan_tui_pane("tui-focus:0.0", &[])?;
    sleep(Duration::from_millis(200));
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
    harness.seed_tui_two_pane_cache(&root_pane_id, &split_pane_id)?;
    let mut client = harness.attach_client("display-tui-focus")?;

    let mut display_popup = harness.start_agentscan_display_popup(&client.tty, &[])?;
    display_popup.wait_until_ready()?;
    harness.send_keys_to_client(&client.tty, ["2"])?;
    display_popup.wait_for_exit()?;
    harness.wait_for_client_pane(&mut client, &split_pane_id)?;

    Ok(())
}

#[test]
fn tui_displays_message_when_cached_pane_no_longer_exists() -> Result<()> {
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
    harness.agentscan(["-f", "cache", "show"])?;
    harness.tmux(["kill-pane", "-t", &split_pane_id])?;
    let mut client = harness.attach_client("tui-missing")?;

    let tui_pane_id = harness.start_agentscan_tui_pane("tui-missing:0.0", &[])?;
    sleep(Duration::from_millis(200));
    harness.tmux(["send-keys", "-t", &tui_pane_id, "2"])?;
    harness.wait_for_client_pane(&mut client, &root_pane_id)?;

    Ok(())
}

#[test]
fn display_popup_closes_when_cached_pane_no_longer_exists() -> Result<()> {
    let harness = TestHarness::new()?;
    if !harness.supports_display_popup_key_injection()? {
        return Ok(());
    }
    let root_pane_id = harness.start_session("display-tui-missing", "sleep 300")?;
    let split_pane_id = harness.split_window("display-tui-missing:0.0", "sleep 300")?;
    harness.seed_tui_two_pane_cache(&root_pane_id, &split_pane_id)?;
    harness.tmux(["kill-pane", "-t", &split_pane_id])?;
    let mut client = harness.attach_client("display-tui-missing")?;

    let mut display_popup = harness.start_agentscan_display_popup(&client.tty, &[])?;
    display_popup.wait_until_ready()?;
    harness.send_keys_to_client(&client.tty, ["2"])?;
    display_popup.wait_for_exit()?;
    harness.wait_for_client_pane(&mut client, &root_pane_id)?;

    Ok(())
}

#[test]
fn tui_ctrl_b_passthrough_returns_to_tmux_prefix_table() -> Result<()> {
    let harness = TestHarness::new()?;
    let _pane_id = harness.start_session("tui-prefix", "sleep 300")?;
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
fn tui_renders_cache_error_frame_when_cache_is_missing() -> Result<()> {
    let harness = TestHarness::new()?;
    let _pane_id = harness.start_session("tui-cache-missing", "sleep 300")?;

    let tui_pane_id = harness.start_agentscan_tui_pane("tui-cache-missing:0.0", &[])?;
    sleep(Duration::from_millis(200));
    let contents = harness.capture_pane(&tui_pane_id)?;

    assert!(
        contents.contains("agentscan tui unavailable"),
        "expected TUI error frame, got:\n{contents}"
    );
    assert!(
        contents.contains("tui --refresh"),
        "expected refresh guidance in TUI error frame, got:\n{contents}"
    );

    Ok(())
}

#[test]
fn tui_rerenders_when_cache_changes() -> Result<()> {
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
    harness.agentscan(["-f", "cache", "show"])?;

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
    harness.wait_for_pane_contents(&tui_pane_id, |contents| contents.contains("Updated Task"))?;

    harness.tmux(["send-keys", "-t", &tui_pane_id, "Escape"])?;
    harness.wait_for_pane_closed(&tui_pane_id)?;

    Ok(())
}

#[test]
fn tui_ignores_non_selection_keys_until_escape() -> Result<()> {
    let harness = TestHarness::new()?;
    let _pane_id = harness.start_session("tui-ignore", "sleep 300")?;

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
    harness.agentscan(["-f", "cache", "show"])?;

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
    harness.agentscan(["-f", "cache", "show"])?;
    let target_pane_id = pane_ids[16].clone();
    let mut client = harness.attach_client("display-tui-paging")?;

    let mut display_popup = harness.start_agentscan_display_popup(&client.tty, &[])?;
    display_popup.wait_until_ready()?;
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
fn daemon_updates_cache_when_windows_are_added_and_removed() -> Result<()> {
    let harness = TestHarness::new()?;
    let root_pane_id = harness.start_session("window-root", "sleep 300")?;
    let mut daemon = harness.start_daemon()?;

    harness.wait_for_pane(&mut daemon, &root_pane_id, |_| true)?;

    let added_pane_id = harness.new_window("window-root", "sleep 300")?;
    harness.wait_for_pane(&mut daemon, &added_pane_id, |_| true)?;

    harness.tmux(["kill-window", "-t", "window-root:1"])?;
    harness.wait_for_cache(&mut daemon, |cache| {
        pane_from_cache(cache, &added_pane_id).is_none()
    })?;

    daemon.shutdown()?;
    Ok(())
}

#[test]
fn daemon_updates_cache_when_session_is_renamed() -> Result<()> {
    let harness = TestHarness::new()?;
    let pane_id = harness.start_session("rename-session", "sleep 300")?;
    let mut daemon = harness.start_daemon()?;

    harness.wait_for_pane(&mut daemon, &pane_id, |_| true)?;

    harness.tmux(["rename-session", "-t", "rename-session", "renamed-session"])?;
    harness.wait_for_pane(&mut daemon, &pane_id, |pane| {
        pane["location"]["session_name"] == "renamed-session"
    })?;

    daemon.shutdown()?;
    Ok(())
}

#[test]
fn daemon_updates_cache_when_window_is_renamed() -> Result<()> {
    let harness = TestHarness::new()?;
    let pane_id = harness.start_session("rename-window", "sleep 300")?;
    let mut daemon = harness.start_daemon()?;

    harness.wait_for_pane(&mut daemon, &pane_id, |_| true)?;

    harness.tmux(["rename-window", "-t", "rename-window:0", "ai"])?;
    harness.wait_for_pane(&mut daemon, &pane_id, |pane| {
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

    harness.wait_for_cache(&mut daemon, |_| true)?;
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

fn write_fake_lifecycle_status(
    stream: &mut UnixStream,
    pid: u32,
    socket_path: &str,
    daemon_start_time: &str,
) {
    let ack = serde_json::json!({
        "type": "hello_ack",
        "protocol_version": 1,
        "snapshot_schema_version": 4
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
                "protocol_version": 1,
                "snapshot_schema_version": 4
            },
            "subscriber_count": 0,
            "latest_snapshot_generated_at": null,
            "latest_snapshot_pane_count": null,
            "unavailable_reason": null,
            "message": null
        }
    });
    writeln!(stream, "{ack}").expect("fake lifecycle ack should write");
    writeln!(stream, "{status}").expect("fake lifecycle status should write");
    stream.flush().expect("fake lifecycle status should flush");
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

include!("common/tmux_harness.rs");
