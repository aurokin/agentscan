use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::Value;

const CACHE_SNAPSHOT_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/cache_snapshot_v1.json"
));

pub fn cache_snapshot_fixture() -> Result<Value> {
    serde_json::from_str(CACHE_SNAPSHOT_FIXTURE).context("cache fixture should parse")
}

pub fn write_cache_snapshot(path: &Path, snapshot: &Value) -> Result<()> {
    fs::write(
        path,
        serde_json::to_vec_pretty(snapshot).context("failed to serialize cache fixture")?,
    )
    .with_context(|| format!("failed to write {}", path.display()))
}

pub fn agentscan_bin() -> Result<PathBuf> {
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
