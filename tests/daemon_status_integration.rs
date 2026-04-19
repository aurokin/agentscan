use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::Value;

const CACHE_SNAPSHOT_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/cache_snapshot_v1.json"
));

#[test]
fn daemon_status_treats_invalid_daemon_timestamp_as_unavailable() -> Result<()> {
    let tempdir = tempfile::tempdir().context("failed to create tempdir")?;
    let cache_path = tempdir.path().join("cache.json");

    let mut snapshot: Value =
        serde_json::from_str(CACHE_SNAPSHOT_FIXTURE).context("cache fixture should parse")?;
    snapshot["source"]["kind"] = Value::String("snapshot".to_string());
    snapshot["source"]["daemon_generated_at"] = Value::String("not-a-timestamp".to_string());

    fs::write(
        &cache_path,
        serde_json::to_vec_pretty(&snapshot).context("failed to serialize cache fixture")?,
    )
    .with_context(|| format!("failed to write {}", cache_path.display()))?;

    let output = Command::new(agentscan_bin()?)
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
