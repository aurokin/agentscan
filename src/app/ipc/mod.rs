#![allow(dead_code)]

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::*;

pub(crate) const SOCKET_ENV_VAR: &str = "AGENTSCAN_SOCKET_PATH";
pub(crate) const WIRE_PROTOCOL_VERSION: u32 = 1;
pub(crate) const CLIENT_HELLO_MAX_BYTES: usize = 4 * 1024;
pub(crate) const DAEMON_FRAME_MAX_BYTES: usize = 4 * 1024 * 1024;

const SOCKET_FILE_NAME: &str = "agentscan.sock";
const SOCKET_DIR_NAME: &str = "agentscan";

#[cfg(any(target_os = "macos", target_os = "ios"))]
const UNIX_SOCKET_PATH_MAX_BYTES: usize = 103;
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
const UNIX_SOCKET_PATH_MAX_BYTES: usize = 107;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SocketPathPlatform {
    Macos,
    Unix,
}

#[derive(Clone, Debug)]
pub(crate) struct SocketPathConfig {
    pub(crate) explicit_path: Option<PathBuf>,
    pub(crate) xdg_runtime_dir: Option<PathBuf>,
    pub(crate) tmpdir: Option<PathBuf>,
    pub(crate) home: Option<PathBuf>,
    pub(crate) xdg_state_home: Option<PathBuf>,
    pub(crate) platform: SocketPathPlatform,
    pub(crate) uid: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SocketPathSource {
    Explicit,
    RuntimeDir,
    MacosTemp,
    MacosCache,
    UnixState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SocketPathCandidate {
    path: PathBuf,
    source: SocketPathSource,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClientMode {
    Snapshot,
    Subscribe,
    LifecycleStatus,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ClientFrame {
    Hello {
        protocol_version: u32,
        snapshot_schema_version: u32,
        mode: ClientMode,
    },
    ClientEvent {
        protocol_version: u32,
        snapshot_schema_version: u32,
        event: ClientEventFrame,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ClientEventFrame {
    PaneFocus { pane_id: String },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ShutdownReason {
    ProtocolMismatch,
    SchemaMismatch,
    ServerBusy,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum UnavailableReason {
    DaemonNotReady,
    StartupFailed,
    ServerClosing,
    SubscribeUnavailable,
    SubscriberLimitReached,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum DaemonFrame {
    HelloAck {
        protocol_version: u32,
        snapshot_schema_version: u32,
    },
    Snapshot {
        snapshot: SnapshotEnvelope,
    },
    Unavailable {
        reason: UnavailableReason,
        message: String,
    },
    LifecycleStatus {
        status: Box<LifecycleStatusFrame>,
    },
    Shutdown {
        reason: ShutdownReason,
        message: String,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LifecycleDaemonState {
    Initializing,
    Ready,
    StartupFailed,
    Closing,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct DaemonIdentityFrame {
    pub(crate) pid: u32,
    pub(crate) daemon_start_time: String,
    pub(crate) executable: String,
    pub(crate) executable_canonical: Option<String>,
    pub(crate) socket_path: String,
    pub(crate) protocol_version: u32,
    pub(crate) snapshot_schema_version: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct LifecycleStatusFrame {
    pub(crate) state: LifecycleDaemonState,
    pub(crate) identity: DaemonIdentityFrame,
    pub(crate) subscriber_count: usize,
    pub(crate) latest_snapshot_generated_at: Option<String>,
    pub(crate) latest_snapshot_pane_count: Option<usize>,
    #[serde(default)]
    pub(crate) latest_snapshot_update_source: Option<String>,
    #[serde(default)]
    pub(crate) latest_snapshot_update_detail: Option<String>,
    #[serde(default)]
    pub(crate) latest_snapshot_update_duration_ms: Option<u64>,
    #[serde(default)]
    pub(crate) control_mode_broker: Option<ControlModeBrokerStatusFrame>,
    #[serde(default)]
    pub(crate) runtime_telemetry: Option<RuntimeTelemetryFrame>,
    #[serde(default)]
    pub(crate) latest_snapshot_observability: Option<SnapshotObservabilityFrame>,
    #[serde(default)]
    pub(crate) recent_events: Vec<DaemonObservabilityEventFrame>,
    pub(crate) unavailable_reason: Option<UnavailableReason>,
    pub(crate) message: Option<String>,
}

// Most telemetry counters are plain `u64` with `#[serde(default)]`, not
// `Option`, by design: adding a counter is a backward/forward-compatible schema
// change that does not bump the protocol version, so an older daemon that
// predates a given counter simply omits it and a newer CLI deserializes it as
// `0`. The availability boundary is the whole frame — `daemon status` reports
// those counters as `null` only when `runtime_telemetry` itself is absent
// (daemon not running / telemetry not initialized). Subscriber health counters
// are intentionally `Option<u64>` because they are paired with per-subscriber
// health lists where "unavailable from an older daemon" must remain distinct
// from real zero/empty coverage diagnostics.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct RuntimeTelemetryFrame {
    pub(crate) control_event_refresh_count: u64,
    #[serde(default)]
    pub(crate) control_event_batch_count: u64,
    #[serde(default)]
    pub(crate) control_event_line_count: u64,
    #[serde(default)]
    pub(crate) control_event_output_line_count: u64,
    #[serde(default)]
    pub(crate) control_event_output_byte_count: u64,
    #[serde(default)]
    pub(crate) control_event_pane_count: u64,
    #[serde(default)]
    pub(crate) control_event_title_count: u64,
    #[serde(default)]
    pub(crate) control_event_window_count: u64,
    #[serde(default)]
    pub(crate) control_event_session_count: u64,
    #[serde(default)]
    pub(crate) control_event_resnapshot_count: u64,
    #[serde(default)]
    pub(crate) control_event_ignored_count: u64,
    pub(crate) reconcile_attempt_count: u64,
    pub(crate) reconcile_noop_count: u64,
    pub(crate) reconcile_changed_snapshot_count: u64,
    #[serde(default)]
    pub(crate) targeted_title_update_count: u64,
    #[serde(default)]
    pub(crate) targeted_pane_refresh_count: u64,
    #[serde(default)]
    pub(crate) targeted_scope_refresh_count: u64,
    #[serde(default)]
    pub(crate) full_snapshot_refresh_count: u64,
    pub(crate) targeted_refresh_fallback_to_full_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) subscriber_monitor_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) subscriber_start_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) subscriber_reattach_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) subscriber_attach_failure_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) subscriber_exit_count: Option<u64>,
    pub(crate) broker_fallback_count: u64,
    #[serde(default)]
    pub(crate) pane_output_capture_attempt_count: u64,
    #[serde(default)]
    pub(crate) pane_output_capture_hit_count: u64,
    #[serde(default)]
    pub(crate) pane_output_capture_error_count: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SnapshotObservabilityFrame {
    pub(crate) provider_known_count: usize,
    pub(crate) provider_unknown_count: usize,
    pub(crate) status_source_pane_metadata_count: usize,
    pub(crate) status_source_tmux_title_count: usize,
    pub(crate) status_source_pane_output_count: usize,
    pub(crate) status_source_not_checked_count: usize,
    pub(crate) proc_fallback_not_run_count: usize,
    pub(crate) proc_fallback_skipped_count: usize,
    pub(crate) proc_fallback_no_match_count: usize,
    pub(crate) proc_fallback_error_count: usize,
    pub(crate) proc_fallback_resolved_count: usize,
    /// Per-provider breakdown of identity match-kind and status-source reliance.
    /// Keyed by canonical provider name; unclassified panes bucket under `unknown`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) per_provider: BTreeMap<String, ProviderPathStats>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ProviderPathStats {
    pub(crate) pane_count: usize,
    pub(crate) matched_pane_metadata_count: usize,
    pub(crate) matched_pane_current_command_count: usize,
    pub(crate) matched_pane_title_count: usize,
    pub(crate) matched_proc_process_tree_count: usize,
    pub(crate) status_source_pane_metadata_count: usize,
    pub(crate) status_source_tmux_title_count: usize,
    pub(crate) status_source_pane_output_count: usize,
    pub(crate) status_source_not_checked_count: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct DaemonObservabilityEventFrame {
    pub(crate) at: String,
    pub(crate) source: String,
    pub(crate) detail: Option<String>,
    pub(crate) refresh: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) control_sources: Vec<ControlModeSourceFrame>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) control_lines: Vec<String>,
    pub(crate) changed: bool,
    pub(crate) published: bool,
    pub(crate) duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) diff: Option<SnapshotDiffFrame>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ControlModeSourceFrame {
    pub(crate) source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) session_id: Option<String>,
    pub(crate) line_count: u64,
    pub(crate) event_count: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SnapshotDiffFrame {
    pub(crate) added_pane_ids: Vec<String>,
    pub(crate) removed_pane_ids: Vec<String>,
    pub(crate) changed_panes: Vec<SnapshotPaneDiffFrame>,
    pub(crate) truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SnapshotPaneDiffFrame {
    pub(crate) pane_id: String,
    pub(crate) fields: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ControlModeBrokerMode {
    Active,
    Fallback,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ControlModeBrokerStatusFrame {
    pub(crate) mode: ControlModeBrokerMode,
    pub(crate) disabled_reason: Option<String>,
    pub(crate) reconnect_count: u32,
    #[serde(default)]
    pub(crate) fallback_count: Option<u64>,
    // Count of per-session event-only subscriber clients (one per non-primary
    // session). `None` when published by an older daemon that predates the
    // per-session-client architecture.
    #[serde(default)]
    pub(crate) subscriber_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) primary_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) subscriber_coverage_complete: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) desired_subscriber_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) active_subscriber_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) missing_subscriber_session_ids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) dead_subscriber_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) subscribers: Option<Vec<ControlModeSubscriberStatusFrame>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_subscriber_reconcile_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) next_subscriber_monitor_in_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) next_reconcile_in_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ControlModeSubscriberStatusFrame {
    pub(crate) session_id: String,
    pub(crate) pid: u32,
    pub(crate) started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_line_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_event_at: Option<String>,
    pub(crate) restart_count: u64,
    pub(crate) dead: bool,
}

impl SocketPathConfig {
    pub(crate) fn from_env() -> Self {
        Self {
            explicit_path: env::var_os(SOCKET_ENV_VAR).map(PathBuf::from),
            xdg_runtime_dir: env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
            tmpdir: env::var_os("TMPDIR").map(PathBuf::from),
            home: env::var_os("HOME").map(PathBuf::from),
            xdg_state_home: env::var_os("XDG_STATE_HOME").map(PathBuf::from),
            platform: current_socket_path_platform(),
            uid: current_uid(),
        }
    }
}

pub(crate) fn resolve_socket_path() -> Result<PathBuf> {
    resolve_socket_path_with_config(&SocketPathConfig::from_env())
}

pub(crate) fn resolve_socket_path_with_config(config: &SocketPathConfig) -> Result<PathBuf> {
    let candidates = socket_path_candidates(config);
    if candidates.is_empty() {
        bail!(
            "could not resolve agentscan socket path because no runtime, temp, state, or home directory is available"
        );
    }

    let mut skipped = Vec::new();
    for candidate in candidates {
        match prepare_socket_path_candidate(&candidate, config.uid) {
            Ok(path) => return Ok(path),
            Err(error) if candidate.source != SocketPathSource::Explicit => {
                skipped.push(format!("{}: {error:#}", candidate.path.display()));
            }
            Err(error) => return Err(error),
        }
    }

    bail!(
        "could not resolve agentscan socket path from fallback candidates: {}",
        skipped.join("; ")
    )
}

pub(crate) fn validate_client_hello(frame: &ClientFrame) -> DaemonFrame {
    let (protocol_version, snapshot_schema_version) = match frame {
        ClientFrame::Hello {
            protocol_version,
            snapshot_schema_version,
            mode: _,
        }
        | ClientFrame::ClientEvent {
            protocol_version,
            snapshot_schema_version,
            event: _,
        } => (*protocol_version, *snapshot_schema_version),
    };

    if protocol_version != WIRE_PROTOCOL_VERSION {
        return DaemonFrame::Shutdown {
            reason: ShutdownReason::ProtocolMismatch,
            message: format!(
                "unsupported IPC protocol version {protocol_version} (expected {WIRE_PROTOCOL_VERSION})"
            ),
        };
    }

    if snapshot_schema_version != CACHE_SCHEMA_VERSION {
        return DaemonFrame::Shutdown {
            reason: ShutdownReason::SchemaMismatch,
            message: format!(
                "unsupported snapshot schema version {snapshot_schema_version} (expected {CACHE_SCHEMA_VERSION})"
            ),
        };
    }

    DaemonFrame::HelloAck {
        protocol_version: WIRE_PROTOCOL_VERSION,
        snapshot_schema_version: CACHE_SCHEMA_VERSION,
    }
}

pub(crate) fn encode_frame(frame: &impl Serialize) -> Result<Vec<u8>> {
    let mut encoded = serde_json::to_vec(frame).context("failed to encode IPC frame")?;
    encoded.push(b'\n');
    Ok(encoded)
}

pub(crate) fn decode_client_frame(bytes: &[u8]) -> Result<ClientFrame> {
    serde_json::from_slice(bytes).context("failed to decode client IPC frame")
}

pub(crate) fn decode_daemon_frame(bytes: &[u8]) -> Result<DaemonFrame> {
    serde_json::from_slice(bytes).context("failed to decode daemon IPC frame")
}

pub(crate) fn read_client_frame(reader: &mut impl BufRead) -> Result<Option<ClientFrame>> {
    read_bounded_json_line(reader, CLIENT_HELLO_MAX_BYTES)?
        .map(|bytes| decode_client_frame(&bytes))
        .transpose()
}

pub(crate) fn read_daemon_frame(reader: &mut impl BufRead) -> Result<Option<DaemonFrame>> {
    read_bounded_json_line(reader, DAEMON_FRAME_MAX_BYTES)?
        .map(|bytes| decode_daemon_frame(&bytes))
        .transpose()
}

pub(crate) fn read_bounded_json_line(
    reader: &mut impl BufRead,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>> {
    let mut output = Vec::new();
    loop {
        let available = reader
            .fill_buf()
            .context("failed to read IPC frame bytes")?;
        if available.is_empty() {
            if output.is_empty() {
                return Ok(None);
            }
            bail!("IPC frame ended before newline");
        }

        let chunk_len = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);

        if output.len().saturating_add(chunk_len) > max_bytes {
            bail!("IPC frame exceeds {max_bytes} byte limit");
        }

        output.extend_from_slice(&available[..chunk_len]);
        reader.consume(chunk_len);

        if output.ends_with(b"\n") {
            output.pop();
            if output.ends_with(b"\r") {
                output.pop();
            }
            return Ok(Some(output));
        }
    }
}

fn socket_path_candidates(config: &SocketPathConfig) -> Vec<SocketPathCandidate> {
    let mut candidates = Vec::new();

    if let Some(path) = config.explicit_path.clone() {
        candidates.push(SocketPathCandidate {
            path,
            source: SocketPathSource::Explicit,
        });
        return candidates;
    }

    if let Some(runtime_dir) = config.xdg_runtime_dir.clone() {
        candidates.push(SocketPathCandidate {
            path: runtime_dir.join(SOCKET_DIR_NAME).join(SOCKET_FILE_NAME),
            source: SocketPathSource::RuntimeDir,
        });
    }

    match config.platform {
        SocketPathPlatform::Macos => {
            if let Some(tmpdir) = config.tmpdir.clone() {
                candidates.push(SocketPathCandidate {
                    path: tmpdir
                        .join(format!("{SOCKET_DIR_NAME}-{}", config.uid))
                        .join(SOCKET_FILE_NAME),
                    source: SocketPathSource::MacosTemp,
                });
            }
            if let Some(home) = config.home.clone() {
                candidates.push(SocketPathCandidate {
                    path: home
                        .join("Library")
                        .join("Caches")
                        .join(SOCKET_DIR_NAME)
                        .join(SOCKET_FILE_NAME),
                    source: SocketPathSource::MacosCache,
                });
            }
        }
        SocketPathPlatform::Unix => {
            if let Some(state_home) = unix_state_home(config) {
                candidates.push(SocketPathCandidate {
                    path: state_home.join(SOCKET_DIR_NAME).join(SOCKET_FILE_NAME),
                    source: SocketPathSource::UnixState,
                });
            }
        }
    }

    candidates
}

fn unix_state_home(config: &SocketPathConfig) -> Option<PathBuf> {
    config.xdg_state_home.clone().or_else(|| {
        config
            .home
            .as_ref()
            .map(|home| home.join(".local").join("state"))
    })
}

fn prepare_socket_path_candidate(candidate: &SocketPathCandidate, uid: u32) -> Result<PathBuf> {
    check_socket_path_length(&candidate.path)?;

    let parent = candidate
        .path
        .parent()
        .with_context(|| format!("socket path {} has no parent", candidate.path.display()))?;
    match candidate.source {
        SocketPathSource::Explicit => validate_socket_parent(parent, uid, false)?,
        SocketPathSource::RuntimeDir
        | SocketPathSource::MacosTemp
        | SocketPathSource::MacosCache
        | SocketPathSource::UnixState => {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create socket directory {}", parent.display())
            })?;
            validate_socket_parent(parent, uid, true)?;
        }
    }

    Ok(candidate.path.clone())
}

fn check_socket_path_length(path: &Path) -> Result<()> {
    let path_bytes = path.as_os_str().as_encoded_bytes().len();
    if path_bytes > UNIX_SOCKET_PATH_MAX_BYTES {
        bail!(
            "socket path {} is too long: {path_bytes} bytes exceeds Unix socket limit of {UNIX_SOCKET_PATH_MAX_BYTES} bytes",
            path.display()
        );
    }
    Ok(())
}

fn validate_socket_parent(parent: &Path, uid: u32, allow_tighten: bool) -> Result<()> {
    let metadata = fs::metadata(parent)
        .with_context(|| format!("failed to inspect socket directory {}", parent.display()))?;
    if !metadata.is_dir() {
        bail!("socket parent {} is not a directory", parent.display());
    }

    validate_socket_parent_owner(parent, &metadata, uid)?;
    validate_socket_parent_permissions(parent, &metadata, allow_tighten)
}

#[cfg(unix)]
fn validate_socket_parent_owner(parent: &Path, metadata: &fs::Metadata, uid: u32) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    if metadata.uid() != uid {
        bail!(
            "socket directory {} is owned by uid {} but current uid is {uid}",
            parent.display(),
            metadata.uid()
        );
    }
    Ok(())
}

#[cfg(unix)]
fn validate_socket_parent_permissions(
    parent: &Path,
    metadata: &fs::Metadata,
    allow_tighten: bool,
) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 == 0 {
        return Ok(());
    }

    if allow_tighten {
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("failed to tighten socket directory {}", parent.display()))?;
        return Ok(());
    }

    bail!(
        "socket directory {} permissions {:03o} are not private",
        parent.display(),
        mode
    )
}

#[cfg(not(unix))]
fn validate_socket_parent_owner(_parent: &Path, _metadata: &fs::Metadata, _uid: u32) -> Result<()> {
    Ok(())
}

#[cfg(not(unix))]
fn validate_socket_parent_permissions(
    _parent: &Path,
    _metadata: &fs::Metadata,
    _allow_tighten: bool,
) -> Result<()> {
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn current_socket_path_platform() -> SocketPathPlatform {
    SocketPathPlatform::Macos
}

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
fn current_socket_path_platform() -> SocketPathPlatform {
    SocketPathPlatform::Unix
}

#[cfg(unix)]
fn current_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and does not mutate Rust-managed memory.
    unsafe { libc::geteuid() }
}

#[cfg(not(unix))]
fn current_uid() -> u32 {
    0
}
