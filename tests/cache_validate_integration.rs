use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

mod common;

#[test]
fn cache_validate_honors_max_age_seconds_for_daemon_diagnostics() -> Result<()> {
    let tempdir = tempfile::tempdir().context("failed to create tempdir")?;
    let cache_path = tempdir.path().join("cache.json");

    let mut snapshot = common::cache_snapshot_fixture()?;
    let now = OffsetDateTime::now_utc();
    snapshot["generated_at"] = Value::String(now.format(&Rfc3339)?);
    snapshot["source"]["kind"] = Value::String("snapshot".to_string());
    snapshot["source"]["daemon_generated_at"] =
        Value::String((now - time::Duration::minutes(5)).format(&Rfc3339)?);

    common::write_cache_snapshot(&cache_path, &snapshot)?;

    let output = Command::new(common::agentscan_bin()?)
        .args(["cache", "validate", "--max-age-seconds", "60"])
        .env("AGENTSCAN_CACHE_PATH", &cache_path)
        .output()
        .context("failed to execute agentscan cache validate")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("agentscan cache validate failed: {}", stderr.trim());
    }

    let stdout = String::from_utf8(output.stdout).context("stdout was not valid UTF-8")?;
    assert!(
        stdout.contains("cache_valid: yes"),
        "expected validation success output, got:\n{stdout}"
    );
    assert!(
        stdout.contains("daemon_cache_status: stale"),
        "expected stale daemon diagnostics, got:\n{stdout}"
    );
    assert!(
        stdout.contains("max_age_seconds: 60"),
        "expected max-age output, got:\n{stdout}"
    );

    drop(tempdir);
    Ok(())
}

#[test]
fn cache_validate_treats_invalid_daemon_timestamp_as_unavailable() -> Result<()> {
    let tempdir = tempfile::tempdir().context("failed to create tempdir")?;
    let cache_path = tempdir.path().join("cache.json");

    let mut snapshot = common::cache_snapshot_fixture()?;
    snapshot["source"]["kind"] = Value::String("snapshot".to_string());
    snapshot["source"]["daemon_generated_at"] = Value::String("not-a-timestamp".to_string());

    common::write_cache_snapshot(&cache_path, &snapshot)?;

    let output = Command::new(common::agentscan_bin()?)
        .args(["cache", "validate"])
        .env("AGENTSCAN_CACHE_PATH", &cache_path)
        .output()
        .context("failed to execute agentscan cache validate")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("agentscan cache validate failed: {}", stderr.trim());
    }

    let stdout = String::from_utf8(output.stdout).context("stdout was not valid UTF-8")?;
    assert!(
        stdout.contains("cache_valid: yes"),
        "expected validation success output, got:\n{stdout}"
    );
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

    drop(tempdir);
    Ok(())
}
