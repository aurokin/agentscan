use std::process::Command;

use anyhow::{Context, Result};

mod common;

#[test]
fn cache_show_is_removed() -> Result<()> {
    let tempdir = tempfile::tempdir().context("failed to create tempdir")?;
    let cache_path = tempdir.path().join("cache.json");

    let snapshot = common::cache_snapshot_fixture()?;
    common::write_cache_snapshot(&cache_path, &snapshot)?;

    let output = Command::new(common::agentscan_bin()?)
        .args(["cache", "show"])
        .env("AGENTSCAN_CACHE_PATH", &cache_path)
        .output()
        .context("failed to execute agentscan cache show")?;
    assert!(
        !output.status.success(),
        "cache show should be removed; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8(output.stderr).context("stderr was not valid UTF-8")?;
    assert!(
        stderr.contains("unrecognized subcommand 'show'"),
        "expected cache show removal error, got:\n{stderr}"
    );

    drop(tempdir);
    Ok(())
}
