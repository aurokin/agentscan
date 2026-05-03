use std::{os::unix::fs::PermissionsExt, process::Command};

use anyhow::{Context, Result};

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
