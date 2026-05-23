use std::{os::unix::fs::PermissionsExt, process::Command};

use anyhow::{Context, Result};
use serde_json::Value;

#[allow(dead_code)]
mod common;

#[test]
fn daemon_status_reports_not_running_without_cache_freshness_checks() -> Result<()> {
    let tempdir = tempfile::tempdir().context("failed to create tempdir")?;
    let mut permissions = std::fs::metadata(tempdir.path())
        .context("failed to read tempdir metadata")?
        .permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(tempdir.path(), permissions)
        .context("failed to make tempdir private")?;
    let socket_path = tempdir.path().join("agentscan.sock");

    let output = Command::new(common::agentscan_bin()?)
        .args(["daemon", "status"])
        .env("AGENTSCAN_SOCKET_PATH", &socket_path)
        .output()
        .context("failed to execute agentscan daemon status")?;
    assert!(
        output.status.success(),
        "daemon status should succeed when not running; status: {}; stdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).context("stdout was not valid UTF-8")?;
    let stderr = String::from_utf8(output.stderr).context("stderr was not valid UTF-8")?;
    assert!(
        stdout.contains("daemon_state: not_running"),
        "expected not-running lifecycle status, got:\n{stdout}"
    );
    assert!(stderr.is_empty(), "did not expect stderr, got:\n{stderr}");

    drop(tempdir);
    Ok(())
}

#[test]
fn daemon_status_json_reports_not_running_without_cache_freshness_checks() -> Result<()> {
    let tempdir = tempfile::tempdir().context("failed to create tempdir")?;
    let mut permissions = std::fs::metadata(tempdir.path())
        .context("failed to read tempdir metadata")?
        .permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(tempdir.path(), permissions)
        .context("failed to make tempdir private")?;
    let socket_path = tempdir.path().join("agentscan.sock");

    let output = Command::new(common::agentscan_bin()?)
        .args(["daemon", "status", "--format", "json"])
        .env("AGENTSCAN_SOCKET_PATH", &socket_path)
        .output()
        .context("failed to execute agentscan daemon status --format json")?;
    assert!(
        output.status.success(),
        "daemon status JSON should succeed when not running; status: {}; stdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).context("stdout was not valid UTF-8")?;
    let stderr = String::from_utf8(output.stderr).context("stderr was not valid UTF-8")?;
    assert!(stderr.is_empty(), "did not expect stderr, got:\n{stderr}");

    let status: Value = serde_json::from_str(&stdout).context("status JSON should parse")?;
    assert_eq!(status["daemon_state"], "not_running");
    assert_eq!(status["socket_path"], socket_path.display().to_string());
    assert!(
        status["lock_path"]
            .as_str()
            .is_some_and(|path| !path.is_empty())
    );
    assert!(
        status["start_lock_path"]
            .as_str()
            .is_some_and(|path| !path.is_empty())
    );
    assert!(
        status["log_path"]
            .as_str()
            .is_some_and(|path| !path.is_empty())
    );
    assert!(
        status["reason"]
            .as_str()
            .is_some_and(|reason| !reason.is_empty())
    );
    assert!(status["pid"].is_null());
    for key in [
        "control_event_refresh_count",
        "reconcile_attempt_count",
        "reconcile_noop_count",
        "reconcile_changed_snapshot_count",
        "targeted_refresh_fallback_to_full_count",
        "broker_fallback_count",
    ] {
        assert!(
            status[key].is_null(),
            "expected {key} to be null when daemon is not running, got:\n{stdout}"
        );
    }

    drop(tempdir);
    Ok(())
}
