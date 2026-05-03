use std::process::Command;

use anyhow::{Context, Result};

#[allow(dead_code)]
mod common;

#[test]
fn cache_command_family_is_removed() -> Result<()> {
    for args in [
        ["cache"].as_slice(),
        ["cache", "path"].as_slice(),
        ["cache", "validate"].as_slice(),
        ["cache", "show"].as_slice(),
    ] {
        let output = Command::new(common::agentscan_bin()?)
            .args(args)
            .output()
            .with_context(|| format!("failed to execute agentscan {args:?}"))?;
        assert!(
            !output.status.success(),
            "cache command should be removed: {args:?}; stdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8(output.stderr).context("stderr was not valid UTF-8")?;
        assert!(
            stderr.contains("unrecognized subcommand 'cache'"),
            "expected removed cache command error for {args:?}, got:\n{stderr}"
        );
    }

    Ok(())
}
