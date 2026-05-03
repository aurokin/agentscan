use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
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

include!("common/tmux_harness.rs");
