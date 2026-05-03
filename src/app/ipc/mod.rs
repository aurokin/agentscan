#![allow(dead_code)]

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
        status: LifecycleStatusFrame,
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
    pub(crate) unavailable_reason: Option<UnavailableReason>,
    pub(crate) message: Option<String>,
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
    match frame {
        ClientFrame::Hello {
            protocol_version,
            snapshot_schema_version,
            mode: _,
        } => {
            if *protocol_version != WIRE_PROTOCOL_VERSION {
                return DaemonFrame::Shutdown {
                    reason: ShutdownReason::ProtocolMismatch,
                    message: format!(
                        "unsupported IPC protocol version {protocol_version} (expected {WIRE_PROTOCOL_VERSION})"
                    ),
                };
            }

            if *snapshot_schema_version != CACHE_SCHEMA_VERSION {
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
