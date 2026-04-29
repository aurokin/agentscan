use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::Value;

mod common;

#[test]
fn daemon_status_treats_invalid_daemon_timestamp_as_unavailable() -> Result<()> {
    let tempdir = tempfile::tempdir().context("failed to create tempdir")?;
    let cache_path = tempdir.path().join("cache.json");

    let mut snapshot = common::cache_snapshot_fixture()?;
    snapshot["source"]["kind"] = Value::String("snapshot".to_string());
    snapshot["source"]["daemon_generated_at"] = Value::String("not-a-timestamp".to_string());

    common::write_cache_snapshot(&cache_path, &snapshot)?;

    let output = Command::new(common::agentscan_bin()?)
        .args(["daemon", "status"])
        .env("AGENTSCAN_CACHE_PATH", &cache_path)
        .output()
        .context("failed to execute agentscan daemon status")?;
    if output.status.success() {
        bail!("agentscan daemon status unexpectedly succeeded");
    }

    let stdout = String::from_utf8(output.stdout).context("stdout was not valid UTF-8")?;
    let stderr = String::from_utf8(output.stderr).context("stderr was not valid UTF-8")?;
    assert!(
        stdout.contains("daemon_generated_at: not-a-timestamp"),
        "expected raw daemon timestamp in output, got:\n{stdout}"
    );
    assert!(
        stdout.contains("daemon_cache_status: unavailable"),
        "expected unavailable daemon diagnostics, got:\n{stdout}"
    );
    assert!(
        stdout.contains(
            "daemon_cache_reason: cache does not include a usable daemon refresh timestamp"
        ),
        "expected unavailable daemon reason, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("daemon_age_seconds:"),
        "did not expect daemon age output, got:\n{stdout}"
    );
    assert!(
        stderr.contains("daemon cache is unavailable"),
        "expected unavailable status error, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("daemon_generated_at was not valid RFC3339"),
        "did not expect parse error, got:\n{stderr}"
    );

    drop(tempdir);
    Ok(())
}
