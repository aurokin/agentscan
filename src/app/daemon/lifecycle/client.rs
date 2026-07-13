use super::*;

// AUR-175 lands this helper before AUR-176 wires command consumers to it.
#[allow(dead_code)]
pub(super) enum SnapshotQuery {
    NotRunning(String),
    Snapshot(SnapshotEnvelope),
    NotReady,
    StartupFailed(String),
    ServerClosing(String),
    Incompatible(String),
    Busy(String),
    Unexpected(String),
}

enum ClientEventEmit {
    Accepted,
    NotRunning,
    NotReady,
    StartupFailed,
    ServerClosing,
    Incompatible,
    Busy,
    Unexpected,
}
pub(super) enum DaemonClientOpen {
    NotRunning(String),
    Connected(DaemonConnection),
}

pub(super) enum DaemonHello {
    Acked,
    Busy(String),
    Rejected { message: String, can_signal: bool },
    Incompatible { message: String, can_signal: bool },
    Unexpected(ipc::DaemonFrame),
}

pub(super) struct DaemonConnection {
    pub(super) reader: BufReader<std::os::unix::net::UnixStream>,
    peer_pid: Option<u32>,
}
pub(super) fn lifecycle_status_from_socket(
    socket_path: &Path,
    timeout: Duration,
) -> Result<LifecycleQuery> {
    let deadline = Instant::now() + timeout;
    loop {
        match lifecycle_status_once(socket_path)? {
            LifecycleQuery::Busy(message) if Instant::now() < deadline => {
                std::thread::sleep(LIFECYCLE_POLL_INTERVAL);
                let _ = message;
            }
            result => return Ok(result),
        }
    }
}

// Read-only daemon lifecycle probe for `agentscan doctor` (AUR-519): the same socket
// query `daemon status` uses, returning structured `LifecycleQuery` instead of printing.
// Never auto-starts a daemon.
pub(crate) fn query_lifecycle_status(socket_path: &Path) -> Result<LifecycleQuery> {
    lifecycle_status_from_socket(socket_path, LIFECYCLE_CONNECT_TIMEOUT)
}

pub(super) fn open_daemon_client(
    socket_path: &Path,
    mode: ipc::ClientMode,
    operation: &str,
    close_write: bool,
) -> Result<DaemonClientOpen> {
    let frame = ipc::ClientFrame::Hello {
        protocol_version: ipc::WIRE_PROTOCOL_VERSION,
        snapshot_schema_version: CACHE_SCHEMA_VERSION,
        mode,
    };
    open_daemon_client_with_frame(socket_path, frame, operation, close_write)
}

fn open_daemon_client_with_frame(
    socket_path: &Path,
    frame: ipc::ClientFrame,
    operation: &str,
    close_write: bool,
) -> Result<DaemonClientOpen> {
    let mut stream = match std::os::unix::net::UnixStream::connect(socket_path) {
        Ok(stream) => stream,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DaemonClientOpen::NotRunning(
                "socket is missing".to_string(),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
            return Ok(DaemonClientOpen::NotRunning(
                "socket exists but no daemon accepted the connection".to_string(),
            ));
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to connect to daemon socket {}",
                    socket_path.display()
                )
            });
        }
    };
    stream
        .set_read_timeout(Some(CLIENT_WRITE_TIMEOUT))
        .with_context(|| format!("failed to set daemon {operation} read timeout"))?;
    stream
        .set_write_timeout(Some(CLIENT_WRITE_TIMEOUT))
        .with_context(|| format!("failed to set daemon {operation} write timeout"))?;
    if let Err(error) = stream.write_all(&ipc::encode_frame(&frame)?) {
        if let Some(reason) = daemon_hello_write_not_running_reason(&error, operation) {
            return Ok(DaemonClientOpen::NotRunning(reason));
        }
        return Err(error).with_context(|| format!("failed to write daemon {operation} hello"));
    }
    if close_write {
        stream
            .shutdown(std::net::Shutdown::Write)
            .with_context(|| format!("failed to close daemon {operation} write side"))?;
    }
    let peer_pid = daemon_socket_peer_pid(&stream);
    Ok(DaemonClientOpen::Connected(DaemonConnection {
        reader: BufReader::new(stream),
        peer_pid,
    }))
}

#[cfg(target_os = "linux")]
fn daemon_socket_peer_pid(stream: &std::os::unix::net::UnixStream) -> Option<u32> {
    let mut credentials = std::mem::MaybeUninit::<libc::ucred>::zeroed();
    let mut credentials_len = libc::socklen_t::try_from(std::mem::size_of::<libc::ucred>()).ok()?;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            credentials.as_mut_ptr().cast(),
            &mut credentials_len,
        )
    };
    if result != 0 {
        return None;
    }
    let credentials = unsafe { credentials.assume_init() };
    u32::try_from(credentials.pid).ok()
}

#[cfg(target_os = "macos")]
fn daemon_socket_peer_pid(stream: &std::os::unix::net::UnixStream) -> Option<u32> {
    let mut peer_pid = 0 as libc::pid_t;
    let mut peer_pid_len = libc::socklen_t::try_from(std::mem::size_of::<libc::pid_t>()).ok()?;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            std::ptr::addr_of_mut!(peer_pid).cast(),
            &mut peer_pid_len,
        )
    };
    if result != 0 {
        return None;
    }
    u32::try_from(peer_pid).ok()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn daemon_socket_peer_pid(_stream: &std::os::unix::net::UnixStream) -> Option<u32> {
    None
}

pub(super) fn daemon_hello_write_not_running_reason(
    error: &std::io::Error,
    operation: &str,
) -> Option<String> {
    matches!(
        error.kind(),
        std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::NotConnected
    )
    .then(|| format!("socket closed before accepting daemon {operation} hello"))
}

pub(super) fn classify_daemon_hello_frame(frame: ipc::DaemonFrame, operation: &str) -> DaemonHello {
    match frame {
        ipc::DaemonFrame::Shutdown {
            reason: ipc::ShutdownReason::ServerBusy,
            message,
        } => DaemonHello::Busy(message),
        ipc::DaemonFrame::Shutdown { reason, message } => DaemonHello::Rejected {
            message: format!("daemon rejected {operation} handshake ({reason:?}): {message}"),
            can_signal: matches!(
                reason,
                ipc::ShutdownReason::ProtocolMismatch | ipc::ShutdownReason::SchemaMismatch
            ),
        },
        ipc::DaemonFrame::HelloAck {
            protocol_version,
            snapshot_schema_version,
        } if protocol_version == ipc::WIRE_PROTOCOL_VERSION
            && snapshot_schema_version == CACHE_SCHEMA_VERSION =>
        {
            DaemonHello::Acked
        }
        ipc::DaemonFrame::HelloAck {
            protocol_version,
            snapshot_schema_version,
        } => DaemonHello::Incompatible {
            message: format!(
                "daemon acknowledged incompatible {operation} handshake (protocol {protocol_version}, schema {snapshot_schema_version}; expected protocol {}, schema {})",
                ipc::WIRE_PROTOCOL_VERSION,
                CACHE_SCHEMA_VERSION
            ),
            can_signal: true,
        },
        other => DaemonHello::Unexpected(other),
    }
}

fn lifecycle_status_once(socket_path: &Path) -> Result<LifecycleQuery> {
    let connection = match open_daemon_client(
        socket_path,
        ipc::ClientMode::LifecycleStatus,
        "lifecycle",
        true,
    )? {
        DaemonClientOpen::NotRunning(reason) => return Ok(LifecycleQuery::NotRunning(reason)),
        DaemonClientOpen::Connected(connection) => connection,
    };
    let peer_pid = connection.peer_pid;
    let mut reader = connection.reader;
    let Some(first_frame) = ipc::read_daemon_frame(&mut reader)? else {
        return Ok(LifecycleQuery::Incompatible {
            message: "daemon closed without lifecycle response".to_string(),
            peer_pid,
            can_signal: false,
        });
    };
    match classify_daemon_hello_frame(first_frame, "lifecycle") {
        DaemonHello::Busy(message) => Ok(LifecycleQuery::Busy(message)),
        DaemonHello::Rejected {
            message,
            can_signal,
        }
        | DaemonHello::Incompatible {
            message,
            can_signal,
        } => Ok(LifecycleQuery::Incompatible {
            message,
            peer_pid,
            can_signal,
        }),
        DaemonHello::Acked => {
            let Some(second_frame) = ipc::read_daemon_frame(&mut reader)? else {
                return Ok(LifecycleQuery::Incompatible {
                    message: "daemon acknowledged lifecycle hello but did not send status"
                        .to_string(),
                    peer_pid,
                    can_signal: false,
                });
            };
            match second_frame {
                ipc::DaemonFrame::LifecycleStatus { status } => Ok(LifecycleQuery::Status(status)),
                other => Ok(LifecycleQuery::Incompatible {
                    message: format!("daemon returned unexpected lifecycle frame {other:?}"),
                    peer_pid,
                    can_signal: false,
                }),
            }
        }
        DaemonHello::Unexpected(other) => Ok(LifecycleQuery::Incompatible {
            message: format!("daemon returned unexpected lifecycle frame {other:?}"),
            peer_pid,
            can_signal: false,
        }),
    }
}

#[allow(dead_code)]
pub(super) fn snapshot_once_from_socket(socket_path: &Path) -> Result<SnapshotQuery> {
    let mut reader =
        match open_daemon_client(socket_path, ipc::ClientMode::Snapshot, "snapshot", true)? {
            DaemonClientOpen::NotRunning(reason) => return Ok(SnapshotQuery::NotRunning(reason)),
            DaemonClientOpen::Connected(connection) => connection.reader,
        };
    let Some(first_frame) = ipc::read_daemon_frame(&mut reader)? else {
        return Ok(SnapshotQuery::Unexpected(
            "daemon closed without snapshot response".to_string(),
        ));
    };
    match classify_daemon_hello_frame(first_frame, "snapshot") {
        DaemonHello::Busy(message) => Ok(SnapshotQuery::Busy(message)),
        DaemonHello::Rejected { message, .. } | DaemonHello::Incompatible { message, .. } => {
            Ok(SnapshotQuery::Incompatible(message))
        }
        DaemonHello::Acked => {
            let Some(second_frame) = ipc::read_daemon_frame(&mut reader)? else {
                return Ok(SnapshotQuery::Unexpected(
                    "daemon acknowledged snapshot hello but did not send snapshot".to_string(),
                ));
            };
            match second_frame {
                ipc::DaemonFrame::Snapshot { snapshot, .. } => {
                    Ok(SnapshotQuery::Snapshot(snapshot))
                }
                ipc::DaemonFrame::Unavailable {
                    reason: ipc::UnavailableReason::DaemonNotReady,
                    message: _,
                } => Ok(SnapshotQuery::NotReady),
                ipc::DaemonFrame::Unavailable {
                    reason: ipc::UnavailableReason::StartupFailed,
                    message,
                } => Ok(SnapshotQuery::StartupFailed(message)),
                ipc::DaemonFrame::Unavailable {
                    reason: ipc::UnavailableReason::ServerClosing,
                    message,
                } => Ok(SnapshotQuery::ServerClosing(message)),
                ipc::DaemonFrame::Unavailable { reason, message } => Ok(SnapshotQuery::Unexpected(
                    format!("daemon returned unexpected unavailable reason {reason:?}: {message}"),
                )),
                other => Ok(SnapshotQuery::Unexpected(format!(
                    "daemon returned unexpected snapshot frame {other:?}"
                ))),
            }
        }
        DaemonHello::Unexpected(other) => Ok(SnapshotQuery::Unexpected(format!(
            "daemon returned unexpected snapshot frame {other:?}"
        ))),
    }
}

fn emit_client_event_once(
    socket_path: &Path,
    event: ipc::ClientEventFrame,
) -> Result<ClientEventEmit> {
    let frame = ipc::ClientFrame::ClientEvent {
        protocol_version: ipc::WIRE_PROTOCOL_VERSION,
        snapshot_schema_version: CACHE_SCHEMA_VERSION,
        event,
    };
    let mut reader = match open_daemon_client_with_frame(socket_path, frame, "client event", true)?
    {
        DaemonClientOpen::NotRunning(_) => {
            return Ok(ClientEventEmit::NotRunning);
        }
        DaemonClientOpen::Connected(connection) => connection.reader,
    };
    let Some(first_frame) = ipc::read_daemon_frame(&mut reader)? else {
        return Ok(ClientEventEmit::Unexpected);
    };
    match classify_daemon_hello_frame(first_frame, "client event") {
        DaemonHello::Busy(_) => Ok(ClientEventEmit::Busy),
        DaemonHello::Rejected { .. } | DaemonHello::Incompatible { .. } => {
            Ok(ClientEventEmit::Incompatible)
        }
        DaemonHello::Acked => match ipc::read_daemon_frame(&mut reader)? {
            None => Ok(ClientEventEmit::Accepted),
            Some(ipc::DaemonFrame::Unavailable {
                reason: ipc::UnavailableReason::DaemonNotReady,
                ..
            }) => Ok(ClientEventEmit::NotReady),
            Some(ipc::DaemonFrame::Unavailable {
                reason: ipc::UnavailableReason::StartupFailed,
                ..
            }) => Ok(ClientEventEmit::StartupFailed),
            Some(ipc::DaemonFrame::Unavailable {
                reason: ipc::UnavailableReason::ServerClosing,
                ..
            }) => Ok(ClientEventEmit::ServerClosing),
            Some(_) => Ok(ClientEventEmit::Unexpected),
        },
        DaemonHello::Unexpected(_) => Ok(ClientEventEmit::Unexpected),
    }
}

pub(crate) fn emit_pane_focus_event_best_effort(pane_id: &str) {
    let Ok(socket_path) = ipc::resolve_socket_path() else {
        return;
    };
    let event = ipc::ClientEventFrame::PaneFocus {
        pane_id: pane_id.to_string(),
    };
    let _ = emit_client_event_once(&socket_path, event);
}
