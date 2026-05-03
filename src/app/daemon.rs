use super::*;
use std::collections::HashMap;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use std::time::Instant;

const RECONCILE_INTERVAL: Duration = Duration::from_secs(1);
const STARTUP_FAILURE_OBSERVABILITY_WINDOW: Duration = Duration::from_millis(200);
const CONTROL_MODE_STARTUP_TIMEOUT: Duration = Duration::from_secs(2);
const CLIENT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);
const CLIENT_WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const SUBSCRIBER_WRITE_TIMEOUT: Duration = Duration::from_millis(250);
const SUBSCRIBER_MONITOR_POLL_INTERVAL: Duration = Duration::from_millis(250);
pub(crate) const MAX_PENDING_HANDSHAKES: usize = 8;
pub(crate) const MAX_SUBSCRIBERS: usize = 64;
const LIFECYCLE_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const DAEMON_START_READINESS_TIMEOUT: Duration = Duration::from_secs(5);
const DAEMON_STOP_TIMEOUT: Duration = Duration::from_secs(3);
const LIFECYCLE_POLL_INTERVAL: Duration = Duration::from_millis(50);
const LOG_TRUNCATE_THRESHOLD_BYTES: u64 = 1024 * 1024;
#[cfg(not(test))]
const TUI_SUBSCRIPTION_INITIAL_BACKOFF: Duration = Duration::from_millis(250);
#[cfg(test)]
const TUI_SUBSCRIPTION_INITIAL_BACKOFF: Duration = Duration::from_millis(10);
const TUI_SUBSCRIPTION_MAX_BACKOFF: Duration = Duration::from_secs(1);
pub(crate) const NO_AUTO_START_ENV_VAR: &str = "AGENTSCAN_NO_AUTO_START";

static DAEMON_SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

type SubscriberId = u64;
pub(crate) type EncodedDaemonFrame = Arc<[u8]>;

#[derive(Clone)]
struct DaemonRuntimeIdentity {
    pid: u32,
    daemon_start_time: String,
    executable: String,
    executable_canonical: Option<String>,
    socket_path: String,
}

impl DaemonRuntimeIdentity {
    fn new(socket_path: &Path) -> Result<Self> {
        let executable = env::current_exe()
            .context("failed to resolve current executable")?
            .display()
            .to_string();
        let executable_canonical = fs::canonicalize(&executable)
            .ok()
            .map(|path| path.display().to_string());
        Ok(Self {
            pid: std::process::id(),
            daemon_start_time: cache::now_rfc3339()?,
            executable,
            executable_canonical,
            socket_path: socket_path.display().to_string(),
        })
    }

    fn frame(&self) -> ipc::DaemonIdentityFrame {
        ipc::DaemonIdentityFrame {
            pid: self.pid,
            daemon_start_time: self.daemon_start_time.clone(),
            executable: self.executable.clone(),
            executable_canonical: self.executable_canonical.clone(),
            socket_path: self.socket_path.clone(),
            protocol_version: ipc::WIRE_PROTOCOL_VERSION,
            snapshot_schema_version: CACHE_SCHEMA_VERSION,
        }
    }

    fn unknown_for_tests() -> Self {
        Self {
            pid: std::process::id(),
            daemon_start_time: "1970-01-01T00:00:00Z".to_string(),
            executable: "unknown".to_string(),
            executable_canonical: None,
            socket_path: "unknown".to_string(),
        }
    }
}

#[derive(Clone)]
struct LifecyclePaths {
    lock_path: PathBuf,
    start_lock_path: PathBuf,
    identity_path: PathBuf,
    log_path: PathBuf,
}

impl LifecyclePaths {
    fn from_socket_path(socket_path: &Path) -> Self {
        Self {
            lock_path: socket_path.with_extension("sock.lock"),
            start_lock_path: socket_path.with_extension("sock.start.lock"),
            identity_path: socket_path.with_extension("sock.identity.json"),
            log_path: socket_path.with_extension("sock.log"),
        }
    }
}

enum LifecycleQuery {
    NotRunning(String),
    Status(ipc::LifecycleStatusFrame),
    Incompatible(String),
    Busy(String),
}

// AUR-175 lands this helper before AUR-176 wires command consumers to it.
#[allow(dead_code)]
enum SnapshotQuery {
    NotRunning(String),
    Snapshot(SnapshotEnvelope),
    NotReady,
    StartupFailed(String),
    ServerClosing(String),
    Incompatible(String),
    Busy(String),
    Unexpected(String),
}

// Quiet mode is consumed by the AUR-175 auto-start helper before command consumers are migrated.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StartOutput {
    Verbose,
    Quiet,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StartConfirmation {
    Started,
    AlreadyRunning,
}

impl StartOutput {
    fn print_ready(
        self,
        confirmation: StartConfirmation,
        paths: &LifecyclePaths,
        status: &ipc::LifecycleStatusFrame,
    ) {
        if self == Self::Verbose {
            match confirmation {
                StartConfirmation::Started => println!("agentscan daemon started"),
                StartConfirmation::AlreadyRunning => println!("agentscan daemon already running"),
            }
            print_lifecycle_status(paths, status);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct AutoStartPolicy {
    disabled: bool,
}

impl AutoStartPolicy {
    pub(crate) fn from_args(args: AutoStartArgs) -> Self {
        Self {
            disabled: args.no_auto_start || env::var(NO_AUTO_START_ENV_VAR).as_deref() == Ok("1"),
        }
    }

    #[cfg(test)]
    pub(crate) fn enabled_for_tests() -> Self {
        Self { disabled: false }
    }

    #[cfg(test)]
    pub(crate) fn disabled_for_tests() -> Self {
        Self { disabled: true }
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DaemonSnapshotError {
    NotRunning { reason: String },
    AutoStartDisabled { reason: String },
    Incompatible { message: String },
    StartupFailed { message: String, log_path: PathBuf },
    ChildExited { status: String, log_path: PathBuf },
    ReadinessTimeout { log_path: PathBuf },
    ServerBusy { message: String },
    ServerClosing { message: String },
    UnexpectedFrame { message: String },
}

impl DaemonSnapshotError {
    pub(crate) fn into_anyhow(self) -> anyhow::Error {
        anyhow::anyhow!("{self}")
    }
}

impl std::fmt::Display for DaemonSnapshotError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRunning { reason } => write!(formatter, "daemon is not running: {reason}"),
            Self::AutoStartDisabled { reason } => {
                write!(formatter, "daemon auto-start is disabled: {reason}")
            }
            Self::Incompatible { message } => {
                write!(formatter, "{}", incompatible_daemon_guidance(message))
            }
            Self::StartupFailed { message, log_path } => {
                write!(
                    formatter,
                    "daemon startup failed: {message}; see log {}",
                    log_path.display()
                )
            }
            Self::ChildExited { status, log_path } => {
                write!(
                    formatter,
                    "daemon exited before readiness with status {status}; see log {}",
                    log_path.display()
                )
            }
            Self::ReadinessTimeout { log_path } => {
                write!(
                    formatter,
                    "timed out waiting for daemon readiness; see log {}",
                    log_path.display()
                )
            }
            Self::ServerBusy { message } => write!(formatter, "{message}"),
            Self::ServerClosing { message } => write!(formatter, "{message}"),
            Self::UnexpectedFrame { message } => write!(formatter, "{message}"),
        }
    }
}

impl std::error::Error for DaemonSnapshotError {}

#[derive(Clone, Debug)]
pub(crate) enum DaemonSubscriptionEvent {
    Connecting { message: String },
    Snapshot { snapshot: SnapshotEnvelope },
    Offline { message: String, retrying: bool },
    Shutdown { message: String },
    Fatal { message: String },
}

enum SubscriptionConnect {
    Subscribed {
        reader: BufReader<std::os::unix::net::UnixStream>,
        bootstrap: SnapshotEnvelope,
    },
    NotRunning(String),
    Retryable(String),
    StartupFailed(String),
    ServerClosing(String),
    Incompatible(String),
    Unexpected(String),
}

struct DaemonStartGuard {
    lock_file: File,
}

impl DaemonStartGuard {
    fn acquire(paths: &LifecyclePaths) -> Result<Self> {
        let lock_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&paths.start_lock_path)
            .with_context(|| {
                format!(
                    "failed to open daemon start lock {}",
                    paths.start_lock_path.display()
                )
            })?;
        let result = unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX) };
        if result != 0 {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("failed to lock {}", paths.start_lock_path.display()));
        }
        Ok(Self { lock_file })
    }
}

impl Drop for DaemonStartGuard {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.lock_file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

struct DaemonLifecycleGuard {
    lock_file: File,
    identity_path: PathBuf,
    identity: ipc::DaemonIdentityFrame,
}

impl DaemonLifecycleGuard {
    fn acquire(paths: &LifecyclePaths, identity: &DaemonRuntimeIdentity) -> Result<Self> {
        let lock_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&paths.lock_path)
            .with_context(|| format!("failed to open daemon lock {}", paths.lock_path.display()))?;
        let result = unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EWOULDBLOCK)
                || error.raw_os_error() == Some(libc::EAGAIN)
            {
                bail!(
                    "another agentscan daemon already owns lock {}",
                    paths.lock_path.display()
                );
            }
            return Err(error)
                .with_context(|| format!("failed to lock {}", paths.lock_path.display()));
        }

        let identity_frame = identity.frame();
        let encoded = serde_json::to_vec_pretty(&identity_frame)
            .context("failed to encode daemon identity")?;
        fs::write(&paths.identity_path, encoded).with_context(|| {
            format!("failed to write identity {}", paths.identity_path.display())
        })?;

        Ok(Self {
            lock_file,
            identity_path: paths.identity_path.clone(),
            identity: identity_frame,
        })
    }
}

impl Drop for DaemonLifecycleGuard {
    fn drop(&mut self) {
        if let Ok(bytes) = fs::read(&self.identity_path)
            && let Ok(current) = serde_json::from_slice::<ipc::DaemonIdentityFrame>(&bytes)
            && current == self.identity
        {
            let _ = fs::remove_file(&self.identity_path);
        }
        unsafe {
            libc::flock(self.lock_file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn lifecycle_status_from_socket(socket_path: &Path, timeout: Duration) -> Result<LifecycleQuery> {
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

fn lifecycle_status_once(socket_path: &Path) -> Result<LifecycleQuery> {
    let mut stream = match std::os::unix::net::UnixStream::connect(socket_path) {
        Ok(stream) => stream,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LifecycleQuery::NotRunning("socket is missing".to_string()));
        }
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
            return Ok(LifecycleQuery::NotRunning(
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
        .context("failed to set daemon lifecycle read timeout")?;
    stream
        .set_write_timeout(Some(CLIENT_WRITE_TIMEOUT))
        .context("failed to set daemon lifecycle write timeout")?;
    let hello = ipc::ClientFrame::Hello {
        protocol_version: ipc::WIRE_PROTOCOL_VERSION,
        snapshot_schema_version: CACHE_SCHEMA_VERSION,
        mode: ipc::ClientMode::LifecycleStatus,
    };
    stream
        .write_all(&ipc::encode_frame(&hello)?)
        .context("failed to write daemon lifecycle hello")?;
    stream
        .shutdown(std::net::Shutdown::Write)
        .context("failed to close daemon lifecycle write side")?;
    let mut reader = BufReader::new(stream);
    let Some(first_frame) = ipc::read_daemon_frame(&mut reader)? else {
        return Ok(LifecycleQuery::Incompatible(
            "daemon closed without lifecycle response".to_string(),
        ));
    };
    match first_frame {
        ipc::DaemonFrame::Shutdown {
            reason: ipc::ShutdownReason::ServerBusy,
            message,
        } => Ok(LifecycleQuery::Busy(message)),
        ipc::DaemonFrame::Shutdown { reason, message } => Ok(LifecycleQuery::Incompatible(
            format!("daemon rejected lifecycle handshake ({reason:?}): {message}"),
        )),
        ipc::DaemonFrame::HelloAck {
            protocol_version,
            snapshot_schema_version,
        } if protocol_version == ipc::WIRE_PROTOCOL_VERSION
            && snapshot_schema_version == CACHE_SCHEMA_VERSION =>
        {
            let Some(second_frame) = ipc::read_daemon_frame(&mut reader)? else {
                return Ok(LifecycleQuery::Incompatible(
                    "daemon acknowledged lifecycle hello but did not send status".to_string(),
                ));
            };
            match second_frame {
                ipc::DaemonFrame::LifecycleStatus { status } => Ok(LifecycleQuery::Status(status)),
                other => Ok(LifecycleQuery::Incompatible(format!(
                    "daemon returned unexpected lifecycle frame {other:?}"
                ))),
            }
        }
        ipc::DaemonFrame::HelloAck {
            protocol_version,
            snapshot_schema_version,
        } => Ok(LifecycleQuery::Incompatible(format!(
            "daemon acknowledged incompatible lifecycle handshake (protocol {protocol_version}, schema {snapshot_schema_version}; expected protocol {}, schema {})",
            ipc::WIRE_PROTOCOL_VERSION,
            CACHE_SCHEMA_VERSION
        ))),
        other => Ok(LifecycleQuery::Incompatible(format!(
            "daemon returned unexpected lifecycle frame {other:?}"
        ))),
    }
}

#[allow(dead_code)]
fn snapshot_once_from_socket(socket_path: &Path) -> Result<SnapshotQuery> {
    let mut stream = match std::os::unix::net::UnixStream::connect(socket_path) {
        Ok(stream) => stream,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SnapshotQuery::NotRunning("socket is missing".to_string()));
        }
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
            return Ok(SnapshotQuery::NotRunning(
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
        .context("failed to set daemon snapshot read timeout")?;
    stream
        .set_write_timeout(Some(CLIENT_WRITE_TIMEOUT))
        .context("failed to set daemon snapshot write timeout")?;
    let hello = ipc::ClientFrame::Hello {
        protocol_version: ipc::WIRE_PROTOCOL_VERSION,
        snapshot_schema_version: CACHE_SCHEMA_VERSION,
        mode: ipc::ClientMode::Snapshot,
    };
    stream
        .write_all(&ipc::encode_frame(&hello)?)
        .context("failed to write daemon snapshot hello")?;
    stream
        .shutdown(std::net::Shutdown::Write)
        .context("failed to close daemon snapshot write side")?;
    let mut reader = BufReader::new(stream);
    let Some(first_frame) = ipc::read_daemon_frame(&mut reader)? else {
        return Ok(SnapshotQuery::Unexpected(
            "daemon closed without snapshot response".to_string(),
        ));
    };
    match first_frame {
        ipc::DaemonFrame::Shutdown {
            reason: ipc::ShutdownReason::ServerBusy,
            message,
        } => Ok(SnapshotQuery::Busy(message)),
        ipc::DaemonFrame::Shutdown { reason, message } => Ok(SnapshotQuery::Incompatible(format!(
            "daemon rejected snapshot handshake ({reason:?}): {message}"
        ))),
        ipc::DaemonFrame::HelloAck {
            protocol_version,
            snapshot_schema_version,
        } if protocol_version == ipc::WIRE_PROTOCOL_VERSION
            && snapshot_schema_version == CACHE_SCHEMA_VERSION =>
        {
            let Some(second_frame) = ipc::read_daemon_frame(&mut reader)? else {
                return Ok(SnapshotQuery::Unexpected(
                    "daemon acknowledged snapshot hello but did not send snapshot".to_string(),
                ));
            };
            match second_frame {
                ipc::DaemonFrame::Snapshot { snapshot } => Ok(SnapshotQuery::Snapshot(snapshot)),
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
        ipc::DaemonFrame::HelloAck {
            protocol_version,
            snapshot_schema_version,
        } => Ok(SnapshotQuery::Incompatible(format!(
            "daemon acknowledged incompatible snapshot handshake (protocol {protocol_version}, schema {snapshot_schema_version}; expected protocol {}, schema {})",
            ipc::WIRE_PROTOCOL_VERSION,
            CACHE_SCHEMA_VERSION
        ))),
        other => Ok(SnapshotQuery::Unexpected(format!(
            "daemon returned unexpected snapshot frame {other:?}"
        ))),
    }
}

pub(crate) fn spawn_subscription_worker(
    policy: AutoStartPolicy,
    events: mpsc::Sender<DaemonSubscriptionEvent>,
    cancel: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let result = subscription_worker_loop(policy, &events, &cancel);
        if let Err(error) = result {
            let _ = events.send(DaemonSubscriptionEvent::Fatal {
                message: error.to_string(),
            });
        }
    })
}

fn subscription_worker_loop(
    policy: AutoStartPolicy,
    events: &mpsc::Sender<DaemonSubscriptionEvent>,
    cancel: &Arc<AtomicBool>,
) -> Result<()> {
    let socket_path = ipc::resolve_socket_path()?;
    let paths = LifecyclePaths::from_socket_path(&socket_path);
    let mut bootstrapped = false;
    let mut attempted_start = false;
    let mut backoff = TUI_SUBSCRIPTION_INITIAL_BACKOFF;

    while !cancel.load(Ordering::Relaxed) {
        send_subscription_event(
            events,
            DaemonSubscriptionEvent::Connecting {
                message: if bootstrapped {
                    format!("reconnecting to daemon at {}", socket_path.display())
                } else {
                    format!("connecting to daemon at {}", socket_path.display())
                },
            },
        )?;

        match subscribe_once_from_socket(&socket_path)? {
            SubscriptionConnect::Subscribed {
                mut reader,
                bootstrap,
            } => {
                attempted_start = false;
                backoff = TUI_SUBSCRIPTION_INITIAL_BACKOFF;
                bootstrapped = true;
                send_subscription_event(
                    events,
                    DaemonSubscriptionEvent::Snapshot {
                        snapshot: bootstrap,
                    },
                )?;
                match read_subscription_frames(&mut reader, events, cancel) {
                    SubscriptionReadResult::Reconnect(message) => {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        send_subscription_event(
                            events,
                            DaemonSubscriptionEvent::Offline {
                                message,
                                retrying: true,
                            },
                        )?;
                    }
                    SubscriptionReadResult::Shutdown(message) => {
                        send_subscription_event(
                            events,
                            DaemonSubscriptionEvent::Shutdown { message },
                        )?;
                        break;
                    }
                    SubscriptionReadResult::Cancelled => break,
                }
            }
            SubscriptionConnect::NotRunning(reason) if policy.disabled => {
                let event = if bootstrapped {
                    DaemonSubscriptionEvent::Offline {
                        message: format!("daemon auto-start is disabled: {reason}"),
                        retrying: false,
                    }
                } else {
                    DaemonSubscriptionEvent::Fatal {
                        message: format!("daemon auto-start is disabled: {reason}"),
                    }
                };
                send_subscription_event(events, event)?;
                break;
            }
            SubscriptionConnect::NotRunning(reason) if !attempted_start => {
                attempted_start = true;
                send_subscription_event(
                    events,
                    DaemonSubscriptionEvent::Connecting {
                        message: format!("starting daemon after {reason}"),
                    },
                )?;
                match daemon_start_with_socket_path_and_output(&socket_path, StartOutput::Quiet) {
                    Ok(()) => {}
                    Err(error) => {
                        send_subscription_event(
                            events,
                            DaemonSubscriptionEvent::Fatal {
                                message: error.to_string(),
                            },
                        )?;
                        break;
                    }
                }
            }
            SubscriptionConnect::NotRunning(reason) => {
                send_subscription_event(
                    events,
                    DaemonSubscriptionEvent::Offline {
                        message: reason,
                        retrying: true,
                    },
                )?;
                sleep_subscription_backoff(cancel, backoff);
                backoff = next_subscription_backoff(backoff);
            }
            SubscriptionConnect::Retryable(message) => {
                send_subscription_event(
                    events,
                    DaemonSubscriptionEvent::Offline {
                        message,
                        retrying: true,
                    },
                )?;
                sleep_subscription_backoff(cancel, backoff);
                backoff = next_subscription_backoff(backoff);
            }
            SubscriptionConnect::StartupFailed(message) => {
                send_subscription_event(
                    events,
                    DaemonSubscriptionEvent::Fatal {
                        message: format!(
                            "daemon startup failed: {message}; see log {}",
                            paths.log_path.display()
                        ),
                    },
                )?;
                break;
            }
            SubscriptionConnect::ServerClosing(message) => {
                send_subscription_event(events, DaemonSubscriptionEvent::Shutdown { message })?;
                break;
            }
            SubscriptionConnect::Incompatible(message) => {
                send_subscription_event(
                    events,
                    DaemonSubscriptionEvent::Fatal {
                        message: incompatible_daemon_guidance(&message),
                    },
                )?;
                break;
            }
            SubscriptionConnect::Unexpected(message) => {
                let event = if bootstrapped {
                    DaemonSubscriptionEvent::Offline {
                        message,
                        retrying: true,
                    }
                } else {
                    DaemonSubscriptionEvent::Fatal { message }
                };
                send_subscription_event(events, event)?;
                if !bootstrapped {
                    break;
                }
                sleep_subscription_backoff(cancel, backoff);
                backoff = next_subscription_backoff(backoff);
            }
        }
    }

    Ok(())
}

fn subscribe_once_from_socket(socket_path: &Path) -> Result<SubscriptionConnect> {
    let mut stream = match std::os::unix::net::UnixStream::connect(socket_path) {
        Ok(stream) => stream,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SubscriptionConnect::NotRunning(
                "socket is missing".to_string(),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
            return Ok(SubscriptionConnect::NotRunning(
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
        .context("failed to set daemon subscription read timeout")?;
    stream
        .set_write_timeout(Some(CLIENT_WRITE_TIMEOUT))
        .context("failed to set daemon subscription write timeout")?;
    let hello = ipc::ClientFrame::Hello {
        protocol_version: ipc::WIRE_PROTOCOL_VERSION,
        snapshot_schema_version: CACHE_SCHEMA_VERSION,
        mode: ipc::ClientMode::Subscribe,
    };
    stream
        .write_all(&ipc::encode_frame(&hello)?)
        .context("failed to write daemon subscription hello")?;
    let mut reader = BufReader::new(stream);
    let Some(first_frame) = (match read_subscription_bootstrap_frame(&mut reader) {
        BootstrapFrameRead::Frame(frame) => frame,
        BootstrapFrameRead::Connect(connect) => return Ok(connect),
    }) else {
        return Ok(SubscriptionConnect::Unexpected(
            "daemon closed without subscription response".to_string(),
        ));
    };
    match first_frame {
        ipc::DaemonFrame::Shutdown {
            reason: ipc::ShutdownReason::ServerBusy,
            message,
        } => Ok(SubscriptionConnect::Retryable(message)),
        ipc::DaemonFrame::Shutdown { reason, message } => Ok(SubscriptionConnect::Incompatible(
            format!("daemon rejected subscription handshake ({reason:?}): {message}"),
        )),
        ipc::DaemonFrame::HelloAck {
            protocol_version,
            snapshot_schema_version,
        } if protocol_version == ipc::WIRE_PROTOCOL_VERSION
            && snapshot_schema_version == CACHE_SCHEMA_VERSION =>
        {
            let Some(second_frame) = (match read_subscription_bootstrap_frame(&mut reader) {
                BootstrapFrameRead::Frame(frame) => frame,
                BootstrapFrameRead::Connect(connect) => return Ok(connect),
            }) else {
                return Ok(SubscriptionConnect::Unexpected(
                    "daemon acknowledged subscription hello but did not send bootstrap snapshot"
                        .to_string(),
                ));
            };
            match second_frame {
                ipc::DaemonFrame::Snapshot { snapshot } => {
                    if let Err(error) = cache::validate_snapshot(&snapshot, None) {
                        return Ok(SubscriptionConnect::Incompatible(format!(
                            "daemon returned invalid bootstrap snapshot: {error:#}"
                        )));
                    }
                    reader
                        .get_ref()
                        .set_read_timeout(None)
                        .context("failed to clear daemon subscription frame read timeout")?;
                    Ok(SubscriptionConnect::Subscribed {
                        reader,
                        bootstrap: snapshot,
                    })
                }
                ipc::DaemonFrame::Unavailable {
                    reason: ipc::UnavailableReason::DaemonNotReady,
                    message,
                }
                | ipc::DaemonFrame::Unavailable {
                    reason: ipc::UnavailableReason::SubscribeUnavailable,
                    message,
                }
                | ipc::DaemonFrame::Unavailable {
                    reason: ipc::UnavailableReason::SubscriberLimitReached,
                    message,
                } => Ok(SubscriptionConnect::Retryable(message)),
                ipc::DaemonFrame::Unavailable {
                    reason: ipc::UnavailableReason::StartupFailed,
                    message,
                } => Ok(SubscriptionConnect::StartupFailed(message)),
                ipc::DaemonFrame::Unavailable {
                    reason: ipc::UnavailableReason::ServerClosing,
                    message,
                } => Ok(SubscriptionConnect::ServerClosing(message)),
                other => Ok(SubscriptionConnect::Unexpected(format!(
                    "daemon returned unexpected subscription frame {other:?}"
                ))),
            }
        }
        ipc::DaemonFrame::HelloAck {
            protocol_version,
            snapshot_schema_version,
        } => Ok(SubscriptionConnect::Incompatible(format!(
            "daemon acknowledged incompatible subscription handshake (protocol {protocol_version}, schema {snapshot_schema_version}; expected protocol {}, schema {})",
            ipc::WIRE_PROTOCOL_VERSION,
            CACHE_SCHEMA_VERSION
        ))),
        other => Ok(SubscriptionConnect::Unexpected(format!(
            "daemon returned unexpected subscription frame {other:?}"
        ))),
    }
}

enum BootstrapFrameRead {
    Frame(Option<ipc::DaemonFrame>),
    Connect(SubscriptionConnect),
}

fn read_subscription_bootstrap_frame(
    reader: &mut BufReader<std::os::unix::net::UnixStream>,
) -> BootstrapFrameRead {
    match ipc::read_daemon_frame(reader) {
        Ok(frame) => BootstrapFrameRead::Frame(frame),
        Err(error) => BootstrapFrameRead::Connect(SubscriptionConnect::Retryable(format!(
            "daemon subscription read failed: {error:#}"
        ))),
    }
}

enum SubscriptionReadResult {
    Reconnect(String),
    Shutdown(String),
    Cancelled,
}

fn read_subscription_frames(
    reader: &mut BufReader<std::os::unix::net::UnixStream>,
    events: &mpsc::Sender<DaemonSubscriptionEvent>,
    cancel: &Arc<AtomicBool>,
) -> SubscriptionReadResult {
    loop {
        if cancel.load(Ordering::Relaxed) {
            return SubscriptionReadResult::Cancelled;
        }

        match ipc::read_daemon_frame(reader) {
            Ok(Some(ipc::DaemonFrame::Snapshot { snapshot })) => {
                if let Err(error) = cache::validate_snapshot(&snapshot, None) {
                    return SubscriptionReadResult::Reconnect(format!(
                        "invalid daemon snapshot: {error:#}"
                    ));
                }
                if send_subscription_event(events, DaemonSubscriptionEvent::Snapshot { snapshot })
                    .is_err()
                {
                    return SubscriptionReadResult::Cancelled;
                }
            }
            Ok(Some(ipc::DaemonFrame::Unavailable {
                reason: ipc::UnavailableReason::ServerClosing,
                message,
            }))
            | Ok(Some(ipc::DaemonFrame::Shutdown { message, .. })) => {
                return SubscriptionReadResult::Shutdown(message);
            }
            Ok(Some(ipc::DaemonFrame::Unavailable { reason, message })) => {
                return SubscriptionReadResult::Reconnect(format!(
                    "daemon subscription unavailable ({reason:?}): {message}"
                ));
            }
            Ok(Some(other)) => {
                return SubscriptionReadResult::Reconnect(format!(
                    "daemon returned unexpected subscription frame {other:?}"
                ));
            }
            Ok(None) => {
                return SubscriptionReadResult::Reconnect("daemon subscription closed".to_string());
            }
            Err(error)
                if error_chain_contains_io_kind(&error, std::io::ErrorKind::TimedOut)
                    || error_chain_contains_io_kind(&error, std::io::ErrorKind::WouldBlock) => {}
            Err(error) => {
                return SubscriptionReadResult::Reconnect(format!(
                    "daemon subscription read failed: {error:#}"
                ));
            }
        }
    }
}

fn send_subscription_event(
    events: &mpsc::Sender<DaemonSubscriptionEvent>,
    event: DaemonSubscriptionEvent,
) -> std::result::Result<(), mpsc::SendError<DaemonSubscriptionEvent>> {
    events.send(event)
}

fn sleep_subscription_backoff(cancel: &AtomicBool, duration: Duration) {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn next_subscription_backoff(duration: Duration) -> Duration {
    duration.saturating_mul(2).min(TUI_SUBSCRIPTION_MAX_BACKOFF)
}

fn error_chain_contains_io_kind(error: &anyhow::Error, kind: std::io::ErrorKind) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io_error| io_error.kind() == kind)
    })
}

// AUR-175 foundation for AUR-176 socket-backed one-shot consumers.
#[allow(dead_code)]
pub(crate) fn snapshot_via_socket(
    policy: AutoStartPolicy,
) -> std::result::Result<SnapshotEnvelope, DaemonSnapshotError> {
    let socket_path =
        ipc::resolve_socket_path().map_err(|error| DaemonSnapshotError::UnexpectedFrame {
            message: error.to_string(),
        })?;
    snapshot_via_socket_path(&socket_path, policy)
}

#[allow(dead_code)]
pub(crate) fn snapshot_via_socket_path(
    socket_path: &Path,
    policy: AutoStartPolicy,
) -> std::result::Result<SnapshotEnvelope, DaemonSnapshotError> {
    snapshot_via_socket_path_with_starter(socket_path, policy, |socket_path| {
        daemon_start_with_socket_path_and_output(socket_path, StartOutput::Quiet)
    })
}

fn snapshot_via_socket_path_with_starter(
    socket_path: &Path,
    policy: AutoStartPolicy,
    mut start_daemon: impl FnMut(&Path) -> std::result::Result<(), DaemonSnapshotError>,
) -> std::result::Result<SnapshotEnvelope, DaemonSnapshotError> {
    let paths = LifecyclePaths::from_socket_path(socket_path);
    let deadline = Instant::now() + DAEMON_START_READINESS_TIMEOUT;
    let mut attempted_start = false;
    loop {
        match snapshot_once_from_socket(socket_path).map_err(|error| {
            DaemonSnapshotError::UnexpectedFrame {
                message: error.to_string(),
            }
        })? {
            SnapshotQuery::Snapshot(snapshot) => {
                cache::validate_snapshot(&snapshot, None).map_err(|error| {
                    DaemonSnapshotError::Incompatible {
                        message: format!("daemon returned invalid snapshot: {error:#}"),
                    }
                })?;
                return Ok(snapshot);
            }
            SnapshotQuery::NotRunning(reason) if policy.disabled => {
                return Err(DaemonSnapshotError::AutoStartDisabled { reason });
            }
            SnapshotQuery::NotRunning(reason) if !attempted_start => {
                attempted_start = true;
                start_daemon(socket_path)?;
                let _ = reason;
            }
            SnapshotQuery::NotRunning(reason) => {
                if Instant::now() >= deadline {
                    return Err(DaemonSnapshotError::NotRunning { reason });
                }
            }
            SnapshotQuery::NotReady => {
                if Instant::now() >= deadline {
                    return Err(DaemonSnapshotError::ReadinessTimeout {
                        log_path: paths.log_path,
                    });
                }
            }
            SnapshotQuery::Busy(message) => {
                if Instant::now() >= deadline {
                    return Err(DaemonSnapshotError::ServerBusy { message });
                }
            }
            SnapshotQuery::StartupFailed(message) => {
                return Err(DaemonSnapshotError::StartupFailed {
                    message,
                    log_path: paths.log_path,
                });
            }
            SnapshotQuery::ServerClosing(message) => {
                return Err(DaemonSnapshotError::ServerClosing { message });
            }
            SnapshotQuery::Incompatible(message) => {
                return Err(DaemonSnapshotError::Incompatible { message });
            }
            SnapshotQuery::Unexpected(message) => {
                return Err(DaemonSnapshotError::UnexpectedFrame { message });
            }
        }
        std::thread::sleep(LIFECYCLE_POLL_INTERVAL);
    }
}

pub(super) fn snapshot_via_socket_path_with_start_command(
    socket_path: &Path,
    executable_path: &Path,
    envs: &[(OsString, OsString)],
    env_removes: &[OsString],
) -> std::result::Result<SnapshotEnvelope, DaemonSnapshotError> {
    snapshot_via_socket_path_with_starter(socket_path, AutoStartPolicy::default(), |socket_path| {
        daemon_start_with_socket_path_output_and_command(
            socket_path,
            StartOutput::Quiet,
            executable_path,
            envs,
            env_removes,
        )
    })
}

fn print_lifecycle_not_running(socket_path: &Path, paths: &LifecyclePaths, reason: &str) {
    println!("daemon_state: not_running");
    println!("socket_path: {}", socket_path.display());
    println!("lock_path: {}", paths.lock_path.display());
    println!("start_lock_path: {}", paths.start_lock_path.display());
    println!("log_path: {}", paths.log_path.display());
    println!("reason: {reason}");
}

fn incompatible_daemon_guidance(message: &str) -> String {
    format!(
        "{message}; stop the incompatible daemon manually, remove the socket only if it is stale, then run `agentscan daemon start`"
    )
}

fn lifecycle_state_label(state: ipc::LifecycleDaemonState) -> &'static str {
    match state {
        ipc::LifecycleDaemonState::Initializing => "initializing",
        ipc::LifecycleDaemonState::Ready => "ready",
        ipc::LifecycleDaemonState::StartupFailed => "startup_failed",
        ipc::LifecycleDaemonState::Closing => "closing",
    }
}

fn unavailable_reason_label(reason: ipc::UnavailableReason) -> &'static str {
    match reason {
        ipc::UnavailableReason::DaemonNotReady => "daemon_not_ready",
        ipc::UnavailableReason::StartupFailed => "startup_failed",
        ipc::UnavailableReason::ServerClosing => "server_closing",
        ipc::UnavailableReason::SubscribeUnavailable => "subscribe_unavailable",
        ipc::UnavailableReason::SubscriberLimitReached => "subscriber_limit_reached",
    }
}

fn print_lifecycle_status(paths: &LifecyclePaths, status: &ipc::LifecycleStatusFrame) {
    println!("daemon_state: {}", lifecycle_state_label(status.state));
    println!("socket_path: {}", status.identity.socket_path);
    println!("lock_path: {}", paths.lock_path.display());
    println!("start_lock_path: {}", paths.start_lock_path.display());
    println!("log_path: {}", paths.log_path.display());
    println!("pid: {}", status.identity.pid);
    println!("daemon_start_time: {}", status.identity.daemon_start_time);
    println!("executable: {}", status.identity.executable);
    if let Some(executable) = &status.identity.executable_canonical {
        println!("executable_canonical: {executable}");
    }
    println!("protocol_version: {}", status.identity.protocol_version);
    println!(
        "snapshot_schema_version: {}",
        status.identity.snapshot_schema_version
    );
    println!("subscriber_count: {}", status.subscriber_count);
    if let Some(generated_at) = &status.latest_snapshot_generated_at {
        println!("latest_snapshot_generated_at: {generated_at}");
    }
    if let Some(pane_count) = status.latest_snapshot_pane_count {
        println!("latest_snapshot_pane_count: {pane_count}");
    }
    if let Some(reason) = status.unavailable_reason {
        println!("unavailable_reason: {}", unavailable_reason_label(reason));
    }
    if let Some(message) = &status.message {
        println!("message: {message}");
    }
}

fn remove_stale_socket_if_present(socket_path: &Path) -> Result<()> {
    let Ok(metadata) = fs::symlink_metadata(socket_path) else {
        return Ok(());
    };
    if !metadata.file_type().is_socket() {
        bail!(
            "refusing to remove non-socket path at daemon socket location {}",
            socket_path.display()
        );
    }
    match std::os::unix::net::UnixStream::connect(socket_path) {
        Ok(_) => bail!(
            "daemon socket {} is still accepting connections",
            socket_path.display()
        ),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
            ) =>
        {
            fs::remove_file(socket_path)
                .with_context(|| format!("failed to remove stale socket {}", socket_path.display()))
        }
        Err(error) => Err(error)
            .with_context(|| format!("failed to probe daemon socket {}", socket_path.display())),
    }
}

fn ensure_socket_path_is_socket_if_present(socket_path: &Path) -> Result<()> {
    let Ok(metadata) = fs::symlink_metadata(socket_path) else {
        return Ok(());
    };
    if metadata.file_type().is_socket() {
        return Ok(());
    }
    bail!(
        "refusing to use non-socket path at daemon socket location {}",
        socket_path.display()
    );
}

fn prepare_log_file(log_path: &Path) -> Result<()> {
    if fs::metadata(log_path).is_ok_and(|metadata| metadata.len() > LOG_TRUNCATE_THRESHOLD_BYTES) {
        File::create(log_path)
            .with_context(|| format!("failed to truncate daemon log {}", log_path.display()))?;
    }
    Ok(())
}

fn wait_for_daemon_start(
    socket_path: &Path,
    paths: &LifecyclePaths,
    child: &mut std::process::Child,
    output: StartOutput,
) -> std::result::Result<(), DaemonSnapshotError> {
    wait_for_daemon_readiness(
        socket_path,
        paths,
        Some(child),
        output,
        StartConfirmation::Started,
    )
}

fn wait_for_existing_daemon_start(
    socket_path: &Path,
    paths: &LifecyclePaths,
    output: StartOutput,
) -> std::result::Result<(), DaemonSnapshotError> {
    wait_for_daemon_readiness(
        socket_path,
        paths,
        None,
        output,
        StartConfirmation::AlreadyRunning,
    )
}

fn wait_for_daemon_readiness(
    socket_path: &Path,
    paths: &LifecyclePaths,
    mut child: Option<&mut std::process::Child>,
    output: StartOutput,
    confirmation: StartConfirmation,
) -> std::result::Result<(), DaemonSnapshotError> {
    let deadline = Instant::now() + DAEMON_START_READINESS_TIMEOUT;
    loop {
        match lifecycle_status_from_socket(socket_path, LIFECYCLE_CONNECT_TIMEOUT).map_err(
            |error| DaemonSnapshotError::UnexpectedFrame {
                message: error.to_string(),
            },
        )? {
            LifecycleQuery::Status(status) if status.state == ipc::LifecycleDaemonState::Ready => {
                output.print_ready(confirmation, paths, &status);
                return Ok(());
            }
            LifecycleQuery::Status(status)
                if status.state == ipc::LifecycleDaemonState::StartupFailed =>
            {
                return Err(DaemonSnapshotError::StartupFailed {
                    message: status
                        .message
                        .unwrap_or_else(|| "startup_failed".to_string()),
                    log_path: paths.log_path.clone(),
                });
            }
            LifecycleQuery::Incompatible(message) => {
                return Err(DaemonSnapshotError::Incompatible { message });
            }
            LifecycleQuery::Busy(message) => {
                if Instant::now() >= deadline {
                    return Err(DaemonSnapshotError::ServerBusy { message });
                }
            }
            LifecycleQuery::Status(_) | LifecycleQuery::NotRunning(_) => {}
        }
        if let Some(child) = child.as_deref_mut()
            && let Some(status) =
                child
                    .try_wait()
                    .map_err(|error| DaemonSnapshotError::UnexpectedFrame {
                        message: format!("failed to poll daemon process: {error}"),
                    })?
        {
            return Err(DaemonSnapshotError::ChildExited {
                status: status.to_string(),
                log_path: paths.log_path.clone(),
            });
        }
        if Instant::now() >= deadline {
            return Err(DaemonSnapshotError::ReadinessTimeout {
                log_path: paths.log_path.clone(),
            });
        }
        std::thread::sleep(LIFECYCLE_POLL_INTERVAL);
    }
}

fn matching_live_status(
    socket_path: &Path,
    expected_identity: &ipc::DaemonIdentityFrame,
) -> Result<ipc::LifecycleStatusFrame> {
    match lifecycle_status_from_socket(socket_path, LIFECYCLE_CONNECT_TIMEOUT)? {
        LifecycleQuery::Status(status) if status.identity == *expected_identity => Ok(status),
        LifecycleQuery::Status(status) => bail!(
            "daemon identity changed from pid {} to pid {}; not sending forced signal",
            expected_identity.pid,
            status.identity.pid
        ),
        LifecycleQuery::NotRunning(reason) => bail!("daemon is no longer running: {reason}"),
        LifecycleQuery::Incompatible(message) => {
            bail!("{message}; not signaling an incompatible daemon")
        }
        LifecycleQuery::Busy(message) => bail!("{message}; not signaling daemon while busy"),
    }
}

fn validate_live_identity_for_signal(identity: &ipc::DaemonIdentityFrame) -> Result<()> {
    if identity.pid == 0 {
        bail!("daemon live identity did not include a valid pid");
    }
    if !process_is_live(identity.pid) {
        bail!("daemon pid {} is not running", identity.pid);
    }
    Ok(())
}

fn process_is_live(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn signal_process(pid: u32, signal: libc::c_int) -> Result<()> {
    let result = unsafe { libc::kill(pid as libc::pid_t, signal) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
            .with_context(|| format!("failed to signal daemon pid {pid}"))
    }
}

fn wait_for_process_exit(pid: u32, timeout: Duration) -> Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        if !process_is_live(pid) {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(LIFECYCLE_POLL_INTERVAL);
    }
}

fn remove_matching_identity(
    identity_path: &Path,
    identity: &ipc::DaemonIdentityFrame,
) -> Result<()> {
    let bytes = match fs::read(identity_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read identity {}", identity_path.display()));
        }
    };
    let current = serde_json::from_slice::<ipc::DaemonIdentityFrame>(&bytes)
        .with_context(|| format!("failed to parse identity {}", identity_path.display()))?;
    if current != *identity {
        return Ok(());
    }
    match fs::remove_file(identity_path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error)
            .with_context(|| format!("failed to remove identity {}", identity_path.display())),
    }
}

fn daemon_start_existing_status(
    socket_path: &Path,
    paths: &LifecyclePaths,
    output: StartOutput,
) -> std::result::Result<bool, DaemonSnapshotError> {
    ensure_socket_path_is_socket_if_present(socket_path).map_err(|error| {
        DaemonSnapshotError::UnexpectedFrame {
            message: error.to_string(),
        }
    })?;
    match lifecycle_status_from_socket(socket_path, LIFECYCLE_CONNECT_TIMEOUT).map_err(|error| {
        DaemonSnapshotError::UnexpectedFrame {
            message: error.to_string(),
        }
    })? {
        LifecycleQuery::Status(status) if status.state == ipc::LifecycleDaemonState::Ready => {
            output.print_ready(StartConfirmation::AlreadyRunning, paths, &status);
            Ok(true)
        }
        LifecycleQuery::Status(status)
            if status.state == ipc::LifecycleDaemonState::Initializing =>
        {
            wait_for_existing_daemon_start(socket_path, paths, output)?;
            Ok(true)
        }
        LifecycleQuery::Status(status)
            if status.state == ipc::LifecycleDaemonState::StartupFailed =>
        {
            Err(DaemonSnapshotError::StartupFailed {
                message: status
                    .message
                    .unwrap_or_else(|| "startup_failed".to_string()),
                log_path: paths.log_path.clone(),
            })
        }
        LifecycleQuery::Status(status) => Err(DaemonSnapshotError::UnexpectedFrame {
            message: format!(
                "daemon socket is reachable but not startable (state {}); use `agentscan daemon status` for details",
                lifecycle_state_label(status.state)
            ),
        }),
        LifecycleQuery::Incompatible(message) => Err(DaemonSnapshotError::Incompatible { message }),
        LifecycleQuery::Busy(message) => Err(DaemonSnapshotError::ServerBusy {
            message: format!("{message}; retry daemon start later"),
        }),
        LifecycleQuery::NotRunning(_) => Ok(false),
    }
}

pub(super) fn daemon_run() -> Result<()> {
    let socket_path =
        ipc::resolve_socket_path().map_err(|error| DaemonSnapshotError::UnexpectedFrame {
            message: error.to_string(),
        })?;
    daemon_run_with_socket_path_and_startup(&socket_path, DaemonStartup)
}

pub(super) fn daemon_status() -> Result<()> {
    let socket_path = ipc::resolve_socket_path()?;
    daemon_status_with_socket_path(&socket_path)
}

pub(crate) fn daemon_status_with_socket_path(socket_path: &Path) -> Result<()> {
    let paths = LifecyclePaths::from_socket_path(socket_path);
    match lifecycle_status_from_socket(socket_path, LIFECYCLE_CONNECT_TIMEOUT)? {
        LifecycleQuery::NotRunning(reason) => {
            print_lifecycle_not_running(socket_path, &paths, &reason);
            Ok(())
        }
        LifecycleQuery::Status(status) => {
            print_lifecycle_status(&paths, &status);
            Ok(())
        }
        LifecycleQuery::Incompatible(message) => {
            bail!("{}", incompatible_daemon_guidance(&message))
        }
        LifecycleQuery::Busy(message) => bail!("{message}"),
    }
}

pub(super) fn daemon_start() -> Result<()> {
    daemon_start_with_output(StartOutput::Verbose).map_err(DaemonSnapshotError::into_anyhow)
}

fn daemon_start_with_output(output: StartOutput) -> std::result::Result<(), DaemonSnapshotError> {
    let socket_path =
        ipc::resolve_socket_path().map_err(|error| DaemonSnapshotError::UnexpectedFrame {
            message: error.to_string(),
        })?;
    daemon_start_with_socket_path_and_output(&socket_path, output)
}

fn daemon_start_with_socket_path_and_output(
    socket_path: &Path,
    output: StartOutput,
) -> std::result::Result<(), DaemonSnapshotError> {
    let executable_path =
        env::current_exe().map_err(|error| DaemonSnapshotError::UnexpectedFrame {
            message: format!("failed to resolve current executable: {error}"),
        })?;
    let envs = daemon_start_tmux_envs();
    let env_removes = daemon_start_env_removes();
    daemon_start_with_socket_path_output_and_command(
        socket_path,
        output,
        &executable_path,
        &envs,
        &env_removes,
    )
}

fn daemon_start_tmux_envs() -> Vec<(OsString, OsString)> {
    daemon_start_tmux_envs_from(|name| env::var_os(name))
}

fn daemon_start_tmux_envs_from(
    read_env: impl Fn(&str) -> Option<OsString>,
) -> Vec<(OsString, OsString)> {
    if read_env(TMUX_SOCKET_ENV_VAR).is_some() {
        return Vec::new();
    }
    let Some(tmux_env) = read_env("TMUX") else {
        return Vec::new();
    };
    let Some(socket_path) = tmux_socket_path_from_tmux_env(tmux_env.as_os_str()) else {
        return Vec::new();
    };
    vec![(OsString::from(TMUX_SOCKET_ENV_VAR), socket_path)]
}

fn daemon_start_env_removes() -> Vec<OsString> {
    daemon_start_env_removes_from(|name| env::var_os(name))
}

fn daemon_start_env_removes_from(read_env: impl Fn(&str) -> Option<OsString>) -> Vec<OsString> {
    if read_env(TMUX_SOCKET_ENV_VAR).is_some() || read_env("TMUX").is_some() {
        vec![OsString::from("TMUX")]
    } else {
        Vec::new()
    }
}

fn tmux_socket_path_from_tmux_env(value: &std::ffi::OsStr) -> Option<OsString> {
    let value = value.to_string_lossy();
    value
        .split(',')
        .next()
        .filter(|socket_path| !socket_path.is_empty())
        .map(OsString::from)
}

#[cfg(test)]
pub(crate) fn test_daemon_start_tmux_envs_from(
    read_env: impl Fn(&str) -> Option<OsString>,
) -> Vec<(OsString, OsString)> {
    daemon_start_tmux_envs_from(read_env)
}

#[cfg(test)]
pub(crate) fn test_daemon_start_env_removes_from(
    read_env: impl Fn(&str) -> Option<OsString>,
) -> Vec<OsString> {
    daemon_start_env_removes_from(read_env)
}

fn daemon_start_with_socket_path_output_and_command(
    socket_path: &Path,
    output: StartOutput,
    executable_path: &Path,
    envs: &[(OsString, OsString)],
    env_removes: &[OsString],
) -> std::result::Result<(), DaemonSnapshotError> {
    let paths = LifecyclePaths::from_socket_path(socket_path);

    if daemon_start_existing_status(socket_path, &paths, output)? {
        return Ok(());
    }
    let _start_guard = DaemonStartGuard::acquire(&paths).map_err(|error| {
        DaemonSnapshotError::UnexpectedFrame {
            message: error.to_string(),
        }
    })?;
    if daemon_start_existing_status(socket_path, &paths, output)? {
        return Ok(());
    }

    remove_stale_socket_if_present(socket_path).map_err(|error| {
        DaemonSnapshotError::UnexpectedFrame {
            message: error.to_string(),
        }
    })?;
    prepare_log_file(&paths.log_path).map_err(|error| DaemonSnapshotError::UnexpectedFrame {
        message: error.to_string(),
    })?;

    let log_stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.log_path)
        .with_context(|| format!("failed to open daemon log {}", paths.log_path.display()))
        .map_err(|error| DaemonSnapshotError::UnexpectedFrame {
            message: error.to_string(),
        })?;
    let log_stderr = log_stdout
        .try_clone()
        .context("failed to clone daemon log handle")
        .map_err(|error| DaemonSnapshotError::UnexpectedFrame {
            message: error.to_string(),
        })?;
    let stdin = File::open("/dev/null")
        .context("failed to open /dev/null for daemon stdin")
        .map_err(|error| DaemonSnapshotError::UnexpectedFrame {
            message: error.to_string(),
        })?;

    let mut command = Command::new(executable_path);
    command
        .args(["daemon", "run"])
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(log_stdout))
        .stderr(Stdio::from(log_stderr));
    command.envs(envs.iter().cloned());
    for key in env_removes {
        command.env_remove(key);
    }
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = command
        .spawn()
        .context("failed to start daemon process")
        .map_err(|error| DaemonSnapshotError::UnexpectedFrame {
            message: error.to_string(),
        })?;
    match wait_for_daemon_start(socket_path, &paths, &mut child, output) {
        Ok(()) => Ok(()),
        Err(error) => {
            cleanup_detached_daemon_child(&mut child);
            Err(error)
        }
    }
}

pub(super) fn daemon_stop() -> Result<()> {
    let socket_path = ipc::resolve_socket_path()?;
    let paths = LifecyclePaths::from_socket_path(&socket_path);
    match lifecycle_status_from_socket(&socket_path, LIFECYCLE_CONNECT_TIMEOUT)? {
        LifecycleQuery::NotRunning(reason) => {
            remove_stale_socket_if_present(&socket_path)?;
            print_lifecycle_not_running(&socket_path, &paths, &reason);
            Ok(())
        }
        LifecycleQuery::Incompatible(message) => {
            bail!("{message}; not signaling an incompatible daemon")
        }
        LifecycleQuery::Busy(message) => bail!("{message}; not signaling daemon while busy"),
        LifecycleQuery::Status(status) => {
            validate_live_identity_for_signal(&status.identity)?;
            signal_process(status.identity.pid, libc::SIGTERM)?;
            if !wait_for_process_exit(status.identity.pid, DAEMON_STOP_TIMEOUT)? {
                let live_status = matching_live_status(&socket_path, &status.identity)?;
                validate_live_identity_for_signal(&live_status.identity)?;
                signal_process(live_status.identity.pid, libc::SIGKILL)?;
                if !wait_for_process_exit(live_status.identity.pid, DAEMON_STOP_TIMEOUT)? {
                    bail!(
                        "timed out waiting for daemon pid {} to exit after SIGKILL",
                        live_status.identity.pid
                    );
                }
            }
            remove_stale_socket_if_present(&socket_path)?;
            remove_matching_identity(&paths.identity_path, &status.identity)?;
            println!("agentscan daemon stopped");
            Ok(())
        }
    }
}

pub(super) fn daemon_restart() -> Result<()> {
    daemon_stop()?;
    daemon_start()
}

#[cfg(test)]
pub(crate) fn test_daemon_run_with_startup(
    socket_path: &Path,
    startup: impl StartupActions,
) -> Result<()> {
    daemon_run_with_socket_path_and_startup(socket_path, startup)
}

fn daemon_run_with_socket_path_and_startup(
    socket_path: &Path,
    startup: impl StartupActions,
) -> Result<()> {
    install_shutdown_signal_handlers();
    DAEMON_SHUTDOWN_REQUESTED.store(false, Ordering::Relaxed);
    let identity = DaemonRuntimeIdentity::new(socket_path)?;
    let lifecycle_paths = LifecyclePaths::from_socket_path(socket_path);
    let _lifecycle_guard = DaemonLifecycleGuard::acquire(&lifecycle_paths, &identity)?;
    let server = DaemonSocketServer::bind(socket_path)?;
    let socket_state = server.state();
    socket_state.set_identity(identity);
    let server_handle = server.spawn();

    let pending_snapshot = match startup.initial_snapshot().and_then(PreparedSnapshot::new) {
        Ok(pending_snapshot) => pending_snapshot,
        Err(error) => {
            let message = startup_failure_message("initial snapshot", &error);
            socket_state.mark_startup_failed(message.clone());
            std::thread::sleep(STARTUP_FAILURE_OBSERVABILITY_WINDOW);
            drop(server_handle);
            return Err(error.context(message));
        }
    };

    let mut tmux_client = match startup.start_tmux_control_mode_client() {
        Ok(client) => client,
        Err(error) => {
            let message = startup_failure_message("tmux control-mode startup", &error);
            socket_state.mark_startup_failed(message.clone());
            std::thread::sleep(STARTUP_FAILURE_OBSERVABILITY_WINDOW);
            drop(server_handle);
            return Err(error.context(message));
        }
    };

    if let Err(error) = startup.publish_initial_cache_snapshot(&pending_snapshot.snapshot) {
        let message = startup_failure_message("initial snapshot publication", &error);
        socket_state.mark_startup_failed(message.clone());
        tmux_client.cleanup();
        std::thread::sleep(STARTUP_FAILURE_OBSERVABILITY_WINDOW);
        drop(server_handle);
        return Err(error.context(message));
    }
    let mut snapshot = pending_snapshot.snapshot.clone();
    socket_state.publish_prepared_snapshot(pending_snapshot);
    let mut closing_guard = DaemonClosingGuard::new(socket_state.clone());
    let stdout_reader = tmux_client
        .stdout_reader
        .take()
        .context("tmux control-mode client did not provide stdout")?;
    let mut child = tmux_client
        .child
        .take()
        .context("tmux control-mode client did not provide child process")?;
    let mut running_tmux_client = RunningTmuxControlModeClient {
        child: &mut child,
        _stdin: tmux_client.stdin.take(),
    };

    let (line_tx, line_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = stdout_reader;
        loop {
            match read_control_mode_line(&mut reader) {
                Ok(Some(line)) => {
                    if line_tx.send(Ok(line)).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = line_tx.send(Err(error));
                    break;
                }
            }
        }
    });

    let mut next_reconcile_at = Instant::now() + RECONCILE_INTERVAL;

    loop {
        if DAEMON_SHUTDOWN_REQUESTED.load(Ordering::Relaxed) {
            break;
        }
        let now = Instant::now();
        if now >= next_reconcile_at {
            reconcile_full_snapshot(&mut snapshot)?;
            socket_state.publish_later_snapshot(snapshot.clone());
            cache::write_snapshot_to_cache(&snapshot)?;
            next_reconcile_at = Instant::now() + RECONCILE_INTERVAL;
        }

        let timeout = next_reconcile_at.saturating_duration_since(Instant::now());
        match line_rx.recv_timeout(timeout) {
            Ok(line) => {
                let line = line?;
                let event = control_event_from_line(&line);
                let should_exit = event == ControlEvent::Exit;
                if apply_control_event(&mut snapshot, &line, &event)? {
                    socket_state.publish_later_snapshot(snapshot.clone());
                    cache::write_snapshot_to_cache(&snapshot)?;
                }

                if should_exit {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                reconcile_full_snapshot(&mut snapshot)?;
                socket_state.publish_later_snapshot(snapshot.clone());
                cache::write_snapshot_to_cache(&snapshot)?;
                next_reconcile_at = Instant::now() + RECONCILE_INTERVAL;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    closing_guard.mark_closing();

    if DAEMON_SHUTDOWN_REQUESTED.load(Ordering::Relaxed) {
        running_tmux_client.terminate();
    } else {
        running_tmux_client.wait_for_exit()?;
    }

    Ok(())
}

pub(crate) trait StartupActions {
    fn initial_snapshot(&self) -> Result<SnapshotEnvelope>;
    fn start_tmux_control_mode_client(&self) -> Result<StartedTmuxControlModeClient>;
    fn publish_initial_cache_snapshot(&self, snapshot: &SnapshotEnvelope) -> Result<()>;
}

#[derive(Default)]
struct DaemonStartup;

impl StartupActions for DaemonStartup {
    fn initial_snapshot(&self) -> Result<SnapshotEnvelope> {
        cache::daemon_snapshot_from_tmux()
    }

    fn start_tmux_control_mode_client(&self) -> Result<StartedTmuxControlModeClient> {
        start_tmux_control_mode_client().map(StartedTmuxControlModeClient::from_real)
    }

    fn publish_initial_cache_snapshot(&self, snapshot: &SnapshotEnvelope) -> Result<()> {
        cache::write_snapshot_to_cache(snapshot)
    }
}

pub(crate) struct StartedTmuxControlModeClient {
    child: Option<std::process::Child>,
    stdout_reader: Option<BufReader<std::process::ChildStdout>>,
    stdin: Option<std::process::ChildStdin>,
}

impl StartedTmuxControlModeClient {
    fn from_real(
        (child, stdout_reader, stdin): (
            std::process::Child,
            BufReader<std::process::ChildStdout>,
            std::process::ChildStdin,
        ),
    ) -> Self {
        Self {
            child: Some(child),
            stdout_reader: Some(stdout_reader),
            stdin: Some(stdin),
        }
    }

    #[cfg(test)]
    pub(crate) fn test_started_without_process() -> Self {
        Self {
            child: None,
            stdout_reader: None,
            stdin: None,
        }
    }

    fn cleanup(&mut self) {
        if let Some(child) = &mut self.child {
            cleanup_startup_child(child);
        }
    }
}

struct RunningTmuxControlModeClient<'a> {
    child: &'a mut std::process::Child,
    _stdin: Option<std::process::ChildStdin>,
}

impl RunningTmuxControlModeClient<'_> {
    fn wait_for_exit(&mut self) -> Result<()> {
        let status = self
            .child
            .wait()
            .context("failed while waiting for tmux control-mode client to exit")?;
        if !status.success() {
            bail!("tmux control-mode client exited with status {status}");
        }
        Ok(())
    }

    fn terminate(&mut self) {
        cleanup_startup_child(self.child);
    }
}

impl Drop for RunningTmuxControlModeClient<'_> {
    fn drop(&mut self) {
        cleanup_startup_child(self.child);
    }
}

extern "C" fn daemon_shutdown_signal_handler(_signal: libc::c_int) {
    DAEMON_SHUTDOWN_REQUESTED.store(true, Ordering::Relaxed);
}

fn install_shutdown_signal_handlers() {
    unsafe {
        libc::signal(
            libc::SIGTERM,
            daemon_shutdown_signal_handler as *const () as usize,
        );
        libc::signal(
            libc::SIGINT,
            daemon_shutdown_signal_handler as *const () as usize,
        );
    }
}

struct DaemonClosingGuard {
    state: DaemonSocketState,
    marked: bool,
}

impl DaemonClosingGuard {
    fn new(state: DaemonSocketState) -> Self {
        Self {
            state,
            marked: false,
        }
    }

    fn mark_closing(&mut self) {
        if !self.marked {
            self.state.mark_closing();
            self.marked = true;
        }
    }
}

impl Drop for DaemonClosingGuard {
    fn drop(&mut self) {
        self.mark_closing();
    }
}

fn startup_failure_message(context: &str, error: &anyhow::Error) -> String {
    format!(
        "{context} failed before daemon socket readiness; no usable socket snapshot was published: {error:#}"
    )
}

fn start_tmux_control_mode_client() -> Result<(
    std::process::Child,
    BufReader<std::process::ChildStdout>,
    std::process::ChildStdin,
)> {
    let session_target = tmux::default_session_target()?;
    let mut child = tmux::tmux_command()
        .args(["-C", "attach-session", "-t", &session_target])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("failed to start tmux control-mode client")?;

    match configure_started_tmux_control_mode_client(&mut child) {
        Ok((stdout_reader, stdin)) => Ok((child, stdout_reader, stdin)),
        Err(error) => {
            cleanup_startup_child(&mut child);
            Err(error)
        }
    }
}

fn configure_started_tmux_control_mode_client(
    child: &mut std::process::Child,
) -> Result<(
    BufReader<std::process::ChildStdout>,
    std::process::ChildStdin,
)> {
    let stdout = child
        .stdout
        .take()
        .context("tmux control-mode client did not provide stdout")?;
    let mut stdout_reader = BufReader::new(stdout);

    let mut stdin = child
        .stdin
        .take()
        .context("tmux control-mode client did not provide stdin")?;
    wait_for_control_mode_startup_response(&mut stdout_reader, "tmux control-mode attach")?;
    writeln!(stdin, "refresh-client -B '{DAEMON_SUBSCRIPTION_FORMAT}'")
        .context("failed to subscribe to pane and metadata updates")?;
    stdin
        .flush()
        .context("failed to flush tmux control commands")?;
    wait_for_control_mode_startup_response(&mut stdout_reader, "daemon subscription setup")?;

    Ok((stdout_reader, stdin))
}

fn wait_for_control_mode_startup_response(
    reader: &mut BufReader<std::process::ChildStdout>,
    context: &str,
) -> Result<()> {
    let deadline = Instant::now() + CONTROL_MODE_STARTUP_TIMEOUT;
    loop {
        let line =
            read_control_mode_line_before_deadline(reader, deadline)?.with_context(|| {
                format!("tmux control-mode client exited before confirming {context}")
            })?;
        if control_mode_startup_response_from_line(&line, context)? {
            return Ok(());
        }
    }
}

fn control_mode_startup_response_from_line(line: &str, context: &str) -> Result<bool> {
    if line.starts_with("%error") {
        bail!("tmux rejected {context}: {line}");
    }
    Ok(line.starts_with("%end"))
}

#[cfg(test)]
pub(crate) fn test_wait_for_attach_then_subscription_transcript(lines: &[&str]) -> Result<()> {
    let mut waiting_for_attach = true;
    for line in lines {
        let context = if waiting_for_attach {
            "tmux control-mode attach"
        } else {
            "daemon subscription setup"
        };
        if control_mode_startup_response_from_line(line, context)? {
            if waiting_for_attach {
                waiting_for_attach = false;
            } else {
                return Ok(());
            }
        }
    }

    bail!("transcript ended before confirming daemon subscription setup")
}

fn read_control_mode_line_before_deadline(
    reader: &mut BufReader<std::process::ChildStdout>,
    deadline: Instant,
) -> Result<Option<String>> {
    wait_for_control_mode_readable(reader, deadline)?;
    read_control_mode_line(reader)
}

fn wait_for_control_mode_readable(
    reader: &BufReader<std::process::ChildStdout>,
    deadline: Instant,
) -> Result<()> {
    if !reader.buffer().is_empty() {
        return Ok(());
    }

    loop {
        let now = Instant::now();
        if now >= deadline {
            bail!("timed out waiting for tmux control-mode subscription setup");
        }
        let timeout = deadline.saturating_duration_since(now);
        let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
        let mut pollfd = libc::pollfd {
            fd: reader.get_ref().as_raw_fd(),
            events: libc::POLLIN | libc::POLLHUP | libc::POLLERR,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
        if result > 0 {
            return Ok(());
        }
        if result == 0 {
            bail!("timed out waiting for tmux control-mode subscription setup");
        }

        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error).context("failed to wait for tmux control-mode output");
        }
    }
}

fn cleanup_startup_child(child: &mut std::process::Child) {
    if let Ok(None) = child.try_wait() {
        let _ = child.kill();
    }
    let _ = child.wait();
}

fn cleanup_detached_daemon_child(child: &mut std::process::Child) {
    if let Ok(None) = child.try_wait() {
        let _ = unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
        let deadline = Instant::now() + STARTUP_FAILURE_OBSERVABILITY_WINDOW * 5;
        while Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(_)) => {
                    let _ = child.wait();
                    return;
                }
                Ok(None) => std::thread::sleep(LIFECYCLE_POLL_INTERVAL),
                Err(_) => break,
            }
        }
        let _ = child.kill();
    }
    let _ = child.wait();
}

struct DaemonSocketServer {
    listener: std::os::unix::net::UnixListener,
    socket_path: PathBuf,
    socket_identity: Option<SocketFileIdentity>,
    state: DaemonSocketState,
    stop: Arc<AtomicBool>,
}

#[derive(Clone, Copy)]
struct SocketFileIdentity {
    dev: u64,
    ino: u64,
}

impl SocketFileIdentity {
    fn from_path(path: &Path) -> Result<Option<Self>> {
        let metadata = fs::symlink_metadata(path)
            .with_context(|| format!("failed to stat socket path {}", path.display()))?;
        if !metadata.file_type().is_socket() {
            return Ok(None);
        }
        Ok(Some(Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
        }))
    }

    fn still_matches(self, path: &Path) -> bool {
        Self::from_path(path)
            .ok()
            .flatten()
            .is_some_and(|current| current.dev == self.dev && current.ino == self.ino)
    }
}

impl DaemonSocketServer {
    fn bind(socket_path: &Path) -> Result<Self> {
        let listener = std::os::unix::net::UnixListener::bind(socket_path)
            .with_context(|| format!("failed to bind daemon socket {}", socket_path.display()))?;
        listener
            .set_nonblocking(true)
            .context("failed to configure daemon socket listener")?;
        Ok(Self {
            listener,
            socket_path: socket_path.to_path_buf(),
            socket_identity: SocketFileIdentity::from_path(socket_path)?,
            state: DaemonSocketState::new(),
            stop: Arc::new(AtomicBool::new(false)),
        })
    }

    fn state(&self) -> DaemonSocketState {
        self.state.clone()
    }

    fn spawn(self) -> DaemonSocketServerHandle {
        let stop = self.stop.clone();
        let handle_stop = self.stop.clone();
        let join = std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match self.listener.accept() {
                    Ok((stream, _)) => {
                        let state = self.state.clone();
                        if let Some(pending_handshake) = state.try_acquire_pending_handshake() {
                            std::thread::spawn(move || {
                                if let Err(error) = handle_daemon_socket_client_with_pending(
                                    stream,
                                    &state,
                                    pending_handshake,
                                ) {
                                    eprintln!("agentscan: daemon socket client failed: {error:#}");
                                }
                            });
                        } else {
                            refuse_server_busy(stream);
                        }
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                        ) =>
                    {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => {
                        eprintln!("agentscan: daemon socket accept failed: {error}");
                        break;
                    }
                }
            }
        });
        DaemonSocketServerHandle {
            stop: handle_stop,
            join: Some(join),
            socket_path: self.socket_path,
            socket_identity: self.socket_identity,
        }
    }
}

struct DaemonSocketServerHandle {
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
    socket_path: PathBuf,
    socket_identity: Option<SocketFileIdentity>,
}

impl Drop for DaemonSocketServerHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        if self
            .socket_identity
            .is_some_and(|identity| identity.still_matches(&self.socket_path))
            && let Err(error) = fs::remove_file(&self.socket_path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!(
                "agentscan: failed to remove daemon socket {}: {error}",
                self.socket_path.display()
            );
        }
    }
}

#[derive(Clone)]
pub(crate) struct DaemonSocketState {
    inner: Arc<Mutex<DaemonSocketStateInner>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum DaemonStartupState {
    Initializing,
    Ready,
    StartupFailed(String),
    Closing,
}

struct DaemonSocketStateInner {
    startup_state: DaemonStartupState,
    identity: Option<DaemonRuntimeIdentity>,
    latest_snapshot: Option<SnapshotEnvelope>,
    latest_snapshot_frame: Option<EncodedDaemonFrame>,
    pending_handshakes: usize,
    subscribers: HashMap<SubscriberId, SubscriberMailbox>,
    next_subscriber_id: SubscriberId,
}

struct PreparedSnapshot {
    snapshot: SnapshotEnvelope,
    frame: EncodedDaemonFrame,
}

impl PreparedSnapshot {
    fn new(snapshot: SnapshotEnvelope) -> Result<Self> {
        let frame = encode_snapshot_frame(&snapshot)?;
        Ok(Self { snapshot, frame })
    }
}

impl DaemonSocketState {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(DaemonSocketStateInner {
                startup_state: DaemonStartupState::Initializing,
                identity: None,
                latest_snapshot: None,
                latest_snapshot_frame: None,
                pending_handshakes: 0,
                subscribers: HashMap::new(),
                next_subscriber_id: 1,
            })),
        }
    }

    fn set_identity(&self, identity: DaemonRuntimeIdentity) {
        self.lock().identity = Some(identity);
    }

    #[cfg(test)]
    pub(crate) fn publish_initial_snapshot(&self, snapshot: SnapshotEnvelope) -> Result<()> {
        let prepared = PreparedSnapshot::new(snapshot)
            .context("initial daemon snapshot exceeded socket frame limit")?;
        self.publish_prepared_snapshot(prepared);
        Ok(())
    }

    fn publish_prepared_snapshot(&self, prepared: PreparedSnapshot) {
        let subscribers = {
            let mut inner = self.lock();
            inner.latest_snapshot = Some(prepared.snapshot);
            inner.latest_snapshot_frame = Some(prepared.frame.clone());
            inner.startup_state = DaemonStartupState::Ready;
            subscriber_mailboxes(&inner)
        };
        fan_out_snapshot(prepared.frame, subscribers);
    }

    pub(crate) fn publish_later_snapshot(&self, snapshot: SnapshotEnvelope) {
        match encode_snapshot_frame(&snapshot) {
            Ok(frame) => {
                let subscribers = {
                    let mut inner = self.lock();
                    inner.latest_snapshot = Some(snapshot);
                    inner.latest_snapshot_frame = Some(frame.clone());
                    inner.startup_state = DaemonStartupState::Ready;
                    subscriber_mailboxes(&inner)
                };
                fan_out_snapshot(frame, subscribers);
            }
            Err(error) => {
                eprintln!(
                    "agentscan: skipped daemon socket snapshot update because encoded frame exceeded {} bytes; previous good snapshot remains active: {error:#}",
                    ipc::DAEMON_FRAME_MAX_BYTES
                );
            }
        }
    }

    pub(crate) fn mark_startup_failed(&self, message: String) {
        let mut inner = self.lock();
        inner.startup_state = DaemonStartupState::StartupFailed(message);
    }

    pub(crate) fn mark_closing(&self) {
        let subscribers = {
            let mut inner = self.lock();
            inner.startup_state = DaemonStartupState::Closing;
            std::mem::take(&mut inner.subscribers)
        };
        close_subscribers(subscribers);
    }

    fn snapshot_response(&self) -> DaemonSocketResponse {
        let inner = self.lock();
        match &inner.startup_state {
            DaemonStartupState::Closing => DaemonSocketResponse::Unavailable {
                reason: ipc::UnavailableReason::ServerClosing,
                message: "daemon socket server is closing".to_string(),
            },
            DaemonStartupState::StartupFailed(message) => DaemonSocketResponse::Unavailable {
                reason: ipc::UnavailableReason::StartupFailed,
                message: message.clone(),
            },
            DaemonStartupState::Ready => {
                if let Some(frame) = &inner.latest_snapshot_frame {
                    DaemonSocketResponse::Snapshot(frame.clone())
                } else {
                    DaemonSocketResponse::Unavailable {
                        reason: ipc::UnavailableReason::StartupFailed,
                        message: "daemon reported ready without a snapshot".to_string(),
                    }
                }
            }
            DaemonStartupState::Initializing => DaemonSocketResponse::Unavailable {
                reason: ipc::UnavailableReason::DaemonNotReady,
                message: "daemon has not published its initial snapshot yet".to_string(),
            },
        }
    }

    fn subscribe_response(&self) -> SubscribeResponse {
        let mut inner = self.lock();
        match &inner.startup_state {
            DaemonStartupState::Closing => SubscribeResponse::Unavailable {
                reason: ipc::UnavailableReason::ServerClosing,
                message: "daemon socket server is closing".to_string(),
            },
            DaemonStartupState::StartupFailed(message) => SubscribeResponse::Unavailable {
                reason: ipc::UnavailableReason::StartupFailed,
                message: message.clone(),
            },
            DaemonStartupState::Initializing => SubscribeResponse::Unavailable {
                reason: ipc::UnavailableReason::DaemonNotReady,
                message: "daemon has not published its initial snapshot yet".to_string(),
            },
            DaemonStartupState::Ready => {
                let Some(bootstrap_frame) = inner.latest_snapshot_frame.clone() else {
                    return SubscribeResponse::Unavailable {
                        reason: ipc::UnavailableReason::StartupFailed,
                        message: "daemon reported ready without a snapshot".to_string(),
                    };
                };
                if inner.subscribers.len() >= MAX_SUBSCRIBERS {
                    return SubscribeResponse::Unavailable {
                        reason: ipc::UnavailableReason::SubscriberLimitReached,
                        message: format!(
                            "daemon subscriber limit reached ({MAX_SUBSCRIBERS} subscribers)"
                        ),
                    };
                }

                let id = inner.next_subscriber_id;
                inner.next_subscriber_id = inner.next_subscriber_id.saturating_add(1);
                let mailbox = SubscriberMailbox::new();
                inner.subscribers.insert(id, mailbox.clone());
                SubscribeResponse::Registered(SubscriberRegistration {
                    id,
                    bootstrap_frame,
                    mailbox,
                })
            }
        }
    }

    fn lifecycle_status(&self) -> ipc::LifecycleStatusFrame {
        let inner = self.lock();
        let identity = inner
            .identity
            .as_ref()
            .cloned()
            .unwrap_or_else(DaemonRuntimeIdentity::unknown_for_tests);
        let (state, unavailable_reason, message) = match &inner.startup_state {
            DaemonStartupState::Initializing => (
                ipc::LifecycleDaemonState::Initializing,
                Some(ipc::UnavailableReason::DaemonNotReady),
                Some("daemon has not published its initial snapshot yet".to_string()),
            ),
            DaemonStartupState::Ready => (ipc::LifecycleDaemonState::Ready, None, None),
            DaemonStartupState::StartupFailed(message) => (
                ipc::LifecycleDaemonState::StartupFailed,
                Some(ipc::UnavailableReason::StartupFailed),
                Some(message.clone()),
            ),
            DaemonStartupState::Closing => (
                ipc::LifecycleDaemonState::Closing,
                Some(ipc::UnavailableReason::ServerClosing),
                Some("daemon socket server is closing".to_string()),
            ),
        };
        ipc::LifecycleStatusFrame {
            state,
            identity: identity.frame(),
            subscriber_count: inner.subscribers.len(),
            latest_snapshot_generated_at: inner
                .latest_snapshot
                .as_ref()
                .map(|snapshot| snapshot.generated_at.clone()),
            latest_snapshot_pane_count: inner
                .latest_snapshot
                .as_ref()
                .map(|snapshot| snapshot.panes.len()),
            unavailable_reason,
            message,
        }
    }

    pub(crate) fn try_acquire_pending_handshake(&self) -> Option<PendingHandshake> {
        let mut inner = self.lock();
        if inner.pending_handshakes >= MAX_PENDING_HANDSHAKES {
            return None;
        }
        inner.pending_handshakes += 1;
        Some(PendingHandshake {
            state: self.clone(),
            active: true,
        })
    }

    fn release_pending_handshake(&self) {
        let mut inner = self.lock();
        inner.pending_handshakes = inner.pending_handshakes.saturating_sub(1);
    }

    fn retire_subscriber(&self, id: SubscriberId) -> bool {
        let subscriber = {
            let mut inner = self.lock();
            inner.subscribers.remove(&id)
        };
        if let Some(mailbox) = subscriber {
            mailbox.close();
            true
        } else {
            false
        }
    }

    fn has_subscriber(&self, id: SubscriberId) -> bool {
        self.lock().subscribers.contains_key(&id)
    }

    #[cfg(test)]
    pub(crate) fn subscriber_count(&self) -> usize {
        self.lock().subscribers.len()
    }

    #[cfg(test)]
    pub(crate) fn pending_handshake_count(&self) -> usize {
        self.lock().pending_handshakes
    }

    #[cfg(test)]
    pub(crate) fn test_register_subscriber_for_capacity(&self) -> Option<SubscriberId> {
        match self.subscribe_response() {
            SubscribeResponse::Registered(registration) => Some(registration.id),
            SubscribeResponse::Unavailable { .. } => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn test_retire_subscriber(&self, id: SubscriberId) -> bool {
        self.retire_subscriber(id)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, DaemonSocketStateInner> {
        self.inner
            .lock()
            .expect("daemon socket state lock poisoned")
    }
}

enum DaemonSocketResponse {
    Snapshot(EncodedDaemonFrame),
    Unavailable {
        reason: ipc::UnavailableReason,
        message: String,
    },
}

enum SubscribeResponse {
    Registered(SubscriberRegistration),
    Unavailable {
        reason: ipc::UnavailableReason,
        message: String,
    },
}

struct SubscriberRegistration {
    id: SubscriberId,
    bootstrap_frame: EncodedDaemonFrame,
    mailbox: SubscriberMailbox,
}

pub(crate) struct PendingHandshake {
    state: DaemonSocketState,
    active: bool,
}

impl PendingHandshake {
    fn release(mut self) {
        if self.active {
            self.active = false;
            self.state.release_pending_handshake();
        }
    }
}

impl Drop for PendingHandshake {
    fn drop(&mut self) {
        if self.active {
            self.active = false;
            self.state.release_pending_handshake();
        }
    }
}

fn subscriber_mailboxes(inner: &DaemonSocketStateInner) -> Vec<SubscriberMailbox> {
    inner.subscribers.values().cloned().collect()
}

fn fan_out_snapshot(frame: EncodedDaemonFrame, subscribers: Vec<SubscriberMailbox>) {
    for subscriber in subscribers {
        subscriber.enqueue(frame.clone());
    }
}

fn close_subscribers(subscribers: HashMap<SubscriberId, SubscriberMailbox>) {
    let closing_frame = ipc::encode_frame(&ipc::DaemonFrame::Unavailable {
        reason: ipc::UnavailableReason::ServerClosing,
        message: "daemon socket server is closing".to_string(),
    })
    .map(Arc::<[u8]>::from)
    .ok();
    for subscriber in subscribers.into_values() {
        if let Some(frame) = &closing_frame {
            subscriber.close_with_frame(frame.clone());
        } else {
            subscriber.close();
        }
    }
}

fn encode_snapshot_frame(snapshot: &SnapshotEnvelope) -> Result<EncodedDaemonFrame> {
    let frame = ipc::DaemonFrame::Snapshot {
        snapshot: snapshot.clone(),
    };
    let encoded = ipc::encode_frame(&frame)?;
    if encoded.len() > ipc::DAEMON_FRAME_MAX_BYTES {
        bail!(
            "encoded snapshot frame was {} bytes, exceeding daemon frame limit of {} bytes",
            encoded.len(),
            ipc::DAEMON_FRAME_MAX_BYTES
        );
    }
    Ok(Arc::<[u8]>::from(encoded))
}

#[derive(Clone)]
pub(crate) struct SubscriberMailbox {
    inner: Arc<(Mutex<SubscriberMailboxState>, Condvar)>,
}

struct SubscriberMailboxState {
    pending_frame: Option<EncodedDaemonFrame>,
    closed: bool,
}

impl SubscriberMailbox {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new((
                Mutex::new(SubscriberMailboxState {
                    pending_frame: None,
                    closed: false,
                }),
                Condvar::new(),
            )),
        }
    }

    pub(crate) fn enqueue(&self, frame: EncodedDaemonFrame) {
        let (lock, condvar) = &*self.inner;
        let mut state = lock.lock().expect("subscriber mailbox lock poisoned");
        if state.closed {
            return;
        }
        state.pending_frame = Some(frame);
        condvar.notify_one();
    }

    pub(crate) fn close(&self) {
        let (lock, condvar) = &*self.inner;
        let mut state = lock.lock().expect("subscriber mailbox lock poisoned");
        state.closed = true;
        state.pending_frame = None;
        condvar.notify_all();
    }

    pub(crate) fn close_with_frame(&self, frame: EncodedDaemonFrame) {
        let (lock, condvar) = &*self.inner;
        let mut state = lock.lock().expect("subscriber mailbox lock poisoned");
        state.pending_frame = Some(frame);
        state.closed = true;
        condvar.notify_all();
    }

    pub(crate) fn recv(&self) -> Option<EncodedDaemonFrame> {
        let (lock, condvar) = &*self.inner;
        let mut state = lock.lock().expect("subscriber mailbox lock poisoned");
        loop {
            if let Some(frame) = state.pending_frame.take() {
                return Some(frame);
            }
            if state.closed {
                return None;
            }
            state = condvar
                .wait(state)
                .expect("subscriber mailbox lock poisoned while waiting");
        }
    }

    #[cfg(test)]
    pub(crate) fn try_take_pending(&self) -> Option<EncodedDaemonFrame> {
        let (lock, _) = &*self.inner;
        lock.lock()
            .expect("subscriber mailbox lock poisoned")
            .pending_frame
            .take()
    }

    #[cfg(test)]
    pub(crate) fn is_closed(&self) -> bool {
        let (lock, _) = &*self.inner;
        lock.lock()
            .expect("subscriber mailbox lock poisoned")
            .closed
    }
}

#[cfg(test)]
pub(crate) fn handle_daemon_socket_client(
    stream: std::os::unix::net::UnixStream,
    state: &DaemonSocketState,
) -> Result<()> {
    if let Some(pending_handshake) = state.try_acquire_pending_handshake() {
        handle_daemon_socket_client_with_pending(stream, state, pending_handshake)
    } else {
        refuse_server_busy(stream);
        Ok(())
    }
}

fn handle_daemon_socket_client_with_pending(
    mut stream: std::os::unix::net::UnixStream,
    state: &DaemonSocketState,
    pending_handshake: PendingHandshake,
) -> Result<()> {
    stream
        .set_write_timeout(Some(CLIENT_WRITE_TIMEOUT))
        .context("failed to set daemon socket write timeout")?;
    let mut writer = stream
        .try_clone()
        .context("failed to clone daemon socket stream")?;
    writer
        .set_write_timeout(Some(CLIENT_WRITE_TIMEOUT))
        .context("failed to set daemon socket writer timeout")?;
    let Some(frame) = read_client_frame_with_deadline(&mut stream)? else {
        return Ok(());
    };

    let ack = match ipc::validate_client_hello(&frame) {
        ack @ ipc::DaemonFrame::HelloAck { .. } => ack,
        shutdown => {
            pending_handshake.release();
            write_daemon_frame(&mut writer, &shutdown)?;
            return Ok(());
        }
    };
    pending_handshake.release();

    match frame {
        ipc::ClientFrame::Hello {
            mode: ipc::ClientMode::Snapshot,
            ..
        } => {
            write_daemon_frame(&mut writer, &ack)?;
            match state.snapshot_response() {
                DaemonSocketResponse::Snapshot(bytes) => {
                    write_all_with_deadline(&mut writer, &bytes, CLIENT_WRITE_TIMEOUT)
                        .context("failed to write daemon snapshot frame")?
                }
                DaemonSocketResponse::Unavailable { reason, message } => write_daemon_frame(
                    &mut writer,
                    &ipc::DaemonFrame::Unavailable { reason, message },
                )?,
            }
            writer
                .flush()
                .context("failed to flush daemon socket frame")
        }
        ipc::ClientFrame::Hello {
            mode: ipc::ClientMode::Subscribe,
            ..
        } => match state.subscribe_response() {
            SubscribeResponse::Registered(registration) => {
                write_daemon_frame(&mut writer, &ack)
                    .and_then(|()| {
                        write_all_with_deadline(
                            &mut writer,
                            &registration.bootstrap_frame,
                            CLIENT_WRITE_TIMEOUT,
                        )
                        .context("failed to write daemon subscriber bootstrap snapshot")
                    })
                    .and_then(|()| {
                        writer
                            .flush()
                            .context("failed to flush daemon socket frame")
                    })
                    .inspect_err(|_| {
                        state.retire_subscriber(registration.id);
                    })?;
                serve_subscriber(stream, writer, state.clone(), registration);
                Ok(())
            }
            SubscribeResponse::Unavailable { reason, message } => {
                write_daemon_frame(&mut writer, &ack)?;
                write_daemon_frame(
                    &mut writer,
                    &ipc::DaemonFrame::Unavailable { reason, message },
                )?;
                writer
                    .flush()
                    .context("failed to flush daemon socket frame")
            }
        },
        ipc::ClientFrame::Hello {
            mode: ipc::ClientMode::LifecycleStatus,
            ..
        } => {
            write_daemon_frame(&mut writer, &ack)?;
            write_daemon_frame(
                &mut writer,
                &ipc::DaemonFrame::LifecycleStatus {
                    status: state.lifecycle_status(),
                },
            )?;
            writer
                .flush()
                .context("failed to flush daemon socket frame")
        }
    }
}

pub(crate) fn refuse_server_busy(mut stream: std::os::unix::net::UnixStream) {
    let _ = stream.set_write_timeout(Some(SUBSCRIBER_WRITE_TIMEOUT));
    let _ = write_daemon_frame(
        &mut stream,
        &ipc::DaemonFrame::Shutdown {
            reason: ipc::ShutdownReason::ServerBusy,
            message: format!("daemon is busy handling {MAX_PENDING_HANDSHAKES} pending clients"),
        },
    );
    let _ = stream.flush();
}

fn serve_subscriber(
    mut stream: std::os::unix::net::UnixStream,
    mut writer: std::os::unix::net::UnixStream,
    state: DaemonSocketState,
    registration: SubscriberRegistration,
) {
    let SubscriberRegistration { id, mailbox, .. } = registration;
    let writer_state = state.clone();
    let writer_mailbox = mailbox.clone();
    std::thread::spawn(move || {
        writer
            .set_write_timeout(Some(SUBSCRIBER_WRITE_TIMEOUT))
            .ok();
        while let Some(frame) = writer_mailbox.recv() {
            let result = write_all_with_deadline(&mut writer, &frame, SUBSCRIBER_WRITE_TIMEOUT)
                .and_then(|()| writer.flush().context("failed to flush subscriber frame"))
                .with_context(|| format!("failed to write subscriber frame for {id}"));
            if result.is_err() {
                writer_state.retire_subscriber(id);
                break;
            }
        }
    });

    stream
        .set_read_timeout(Some(SUBSCRIBER_MONITOR_POLL_INTERVAL))
        .ok();
    let mut byte = [0; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => {
                state.retire_subscriber(id);
                break;
            }
            Ok(_) => {
                state.retire_subscriber(id);
                break;
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if !state.has_subscriber(id) {
                    break;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => {
                state.retire_subscriber(id);
                break;
            }
        }
    }
}

fn read_client_frame_with_deadline(
    stream: &mut std::os::unix::net::UnixStream,
) -> Result<Option<ipc::ClientFrame>> {
    let deadline = Instant::now() + CLIENT_HANDSHAKE_TIMEOUT;
    let mut output = Vec::new();
    let mut byte = [0; 1];

    loop {
        let now = Instant::now();
        if now >= deadline {
            bail!("timed out waiting for daemon client hello");
        }
        stream
            .set_read_timeout(Some(deadline.saturating_duration_since(now)))
            .context("failed to set daemon socket read timeout")?;

        match stream.read(&mut byte) {
            Ok(0) if output.is_empty() => return Ok(None),
            Ok(0) => bail!("IPC frame ended before newline"),
            Ok(_) => {
                if output.len() >= ipc::CLIENT_HELLO_MAX_BYTES {
                    bail!(
                        "IPC frame exceeds {} byte limit",
                        ipc::CLIENT_HELLO_MAX_BYTES
                    );
                }
                if byte[0] == b'\n' {
                    if output.ends_with(b"\r") {
                        output.pop();
                    }
                    return ipc::decode_client_frame(&output).map(Some);
                }
                output.push(byte[0]);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                bail!("timed out waiting for daemon client hello");
            }
            Err(error) => return Err(error).context("failed to read daemon client hello"),
        }
    }
}

fn write_daemon_frame(writer: &mut impl Write, frame: &ipc::DaemonFrame) -> Result<()> {
    let encoded = ipc::encode_frame(frame)?;
    if encoded.len() > ipc::DAEMON_FRAME_MAX_BYTES {
        bail!(
            "daemon frame was {} bytes, exceeding daemon frame limit of {} bytes",
            encoded.len(),
            ipc::DAEMON_FRAME_MAX_BYTES
        );
    }
    writer
        .write_all(&encoded)
        .context("failed to write daemon socket frame")
}

fn write_all_with_deadline(
    writer: &mut impl Write,
    mut bytes: &[u8],
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while !bytes.is_empty() {
        match writer.write(bytes) {
            Ok(0) => bail!("daemon socket write returned zero bytes"),
            Ok(written) => bytes = &bytes[written..],
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(error).context("failed to write daemon socket frame"),
        }
    }
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
enum ControlEvent {
    PaneChanged(String),
    TitleChanged { pane_id: String, title: String },
    WindowChanged(String),
    SessionChanged(String),
    Resnapshot,
    Exit,
    Ignored,
}

fn control_event_from_line(line: &str) -> ControlEvent {
    if line.starts_with("%exit") {
        return ControlEvent::Exit;
    }

    if let Some(pane_id) = subscription_changed_pane_id(line) {
        return ControlEvent::PaneChanged(pane_id.to_string());
    }

    if let Some(change) = output_title_change(line) {
        return ControlEvent::TitleChanged {
            pane_id: change.pane_id.to_string(),
            title: change.title,
        };
    }

    if let Some(window_id) = window_notification_target(line) {
        return ControlEvent::WindowChanged(window_id.to_string());
    }

    if let Some(session_id) = session_notification_target(line) {
        return ControlEvent::SessionChanged(session_id.to_string());
    }

    if should_resnapshot_from_notification(line) {
        return ControlEvent::Resnapshot;
    }

    ControlEvent::Ignored
}

fn apply_control_event(
    snapshot: &mut SnapshotEnvelope,
    line: &str,
    event: &ControlEvent,
) -> Result<bool> {
    match event {
        ControlEvent::PaneChanged(pane_id) => {
            refresh_snapshot_pane(snapshot, pane_id)?;
            merge_cached_panes(snapshot, Some(pane_id));
            Ok(true)
        }
        ControlEvent::TitleChanged { pane_id, title } => {
            refresh_snapshot_pane_with_title(snapshot, pane_id, Some(title.as_str()))?;
            merge_cached_panes(snapshot, Some(pane_id));
            Ok(true)
        }
        ControlEvent::WindowChanged(window_id) => {
            refresh_snapshot_window(snapshot, window_id)
                .or_else(|error| fallback_to_full_resnapshot(snapshot, line, error))?;
            Ok(true)
        }
        ControlEvent::SessionChanged(session_id) => {
            refresh_snapshot_session(snapshot, session_id)
                .or_else(|error| fallback_to_full_resnapshot(snapshot, line, error))?;
            Ok(true)
        }
        ControlEvent::Resnapshot => {
            reconcile_full_snapshot(snapshot)?;
            Ok(true)
        }
        ControlEvent::Exit | ControlEvent::Ignored => Ok(false),
    }
}

pub(crate) fn read_control_mode_line(reader: &mut impl BufRead) -> Result<Option<String>> {
    let mut bytes = Vec::new();
    let bytes_read = reader
        .read_until(b'\n', &mut bytes)
        .context("failed to read tmux control-mode output")?;
    if bytes_read == 0 {
        return Ok(None);
    }

    if bytes.ends_with(b"\n") {
        bytes.pop();
    }
    if bytes.ends_with(b"\r") {
        bytes.pop();
    }

    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

pub(crate) fn should_resnapshot_from_notification(line: &str) -> bool {
    matches!(
        notification_name(line),
        Some(
            "%sessions-changed"
                | "%session-changed"
                | "%session-renamed"
                | "%session-window-changed"
                | "%layout-change"
                | "%window-add"
                | "%window-close"
                | "%unlinked-window-close"
                | "%window-pane-changed"
                | "%window-renamed"
        )
    )
}

pub(crate) fn subscription_changed_pane_id(line: &str) -> Option<&str> {
    let mut fields = line.split_whitespace();
    if fields.next()? != "%subscription-changed" {
        return None;
    }
    let _subscription_name = fields.next()?;
    let _session = fields.next()?;
    let _window = fields.next()?;
    let _flags = fields.next()?;
    let pane_id = fields.next()?;
    pane_id.starts_with('%').then_some(pane_id)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn output_title_change_pane_id(line: &str) -> Option<&str> {
    output_title_change(line).map(|change| change.pane_id)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn output_title_change_title(line: &str) -> Option<String> {
    output_title_change(line).map(|change| change.title)
}

struct OutputTitleChange<'a> {
    pane_id: &'a str,
    title: String,
}

fn output_title_change(line: &str) -> Option<OutputTitleChange<'_>> {
    let mut fields = line.splitn(3, ' ');
    if fields.next()? != "%output" {
        return None;
    }

    let pane_id = fields.next()?;
    let payload = fields.next()?;
    let title = terminal_title_from_control_payload(payload)?;
    if !pane_id.starts_with('%') {
        return None;
    }

    Some(OutputTitleChange { pane_id, title })
}

fn terminal_title_from_control_payload(payload: &str) -> Option<String> {
    let decoded = decode_tmux_control_payload(payload);
    terminal_title_from_decoded_output(&decoded)
}

fn decode_tmux_control_payload(payload: &str) -> String {
    let bytes = payload.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'\\'
            && index + 3 < bytes.len()
            && is_octal_digit(bytes[index + 1])
            && is_octal_digit(bytes[index + 2])
            && is_octal_digit(bytes[index + 3])
        {
            let value = ((bytes[index + 1] - b'0') << 6)
                | ((bytes[index + 2] - b'0') << 3)
                | (bytes[index + 3] - b'0');
            decoded.push(value);
            index += 4;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }

    String::from_utf8_lossy(&decoded).into_owned()
}

const fn is_octal_digit(byte: u8) -> bool {
    byte >= b'0' && byte <= b'7'
}

fn terminal_title_from_decoded_output(output: &str) -> Option<String> {
    let bytes = output.as_bytes();
    let mut index = 0;
    let mut title = None;

    while index + 4 <= bytes.len() {
        if bytes[index] == 0x1b
            && bytes[index + 1] == b']'
            && matches!(bytes[index + 2], b'0' | b'2')
            && bytes[index + 3] == b';'
        {
            let title_start = index + 4;
            let mut title_end = title_start;
            while title_end < bytes.len() {
                if bytes[title_end] == 0x07 {
                    title =
                        Some(String::from_utf8_lossy(&bytes[title_start..title_end]).into_owned());
                    index = title_end + 1;
                    break;
                }
                if title_end + 1 < bytes.len()
                    && bytes[title_end] == 0x1b
                    && bytes[title_end + 1] == b'\\'
                {
                    title =
                        Some(String::from_utf8_lossy(&bytes[title_start..title_end]).into_owned());
                    index = title_end + 2;
                    break;
                }
                title_end += 1;
            }

            if title_end == bytes.len() {
                break;
            }
        } else {
            index += 1;
        }
    }

    title
}

fn refresh_snapshot_pane(snapshot: &mut SnapshotEnvelope, pane_id: &str) -> Result<()> {
    refresh_snapshot_pane_with_title(snapshot, pane_id, None)
}

fn refresh_snapshot_pane_with_title(
    snapshot: &mut SnapshotEnvelope,
    pane_id: &str,
    title_override: Option<&str>,
) -> Result<()> {
    let pane = tmux::tmux_list_pane(pane_id)?.map(|mut row| {
        if let Some(title) = title_override {
            row.pane_title_raw = title.to_string();
        }
        let mut pane = classify::pane_from_row(row);
        let proc_inspector = proc::ProcProcessInspector;
        classify::apply_proc_fallback(&mut pane, &proc_inspector);
        scanner::apply_pane_output_status_fallbacks(std::slice::from_mut(&mut pane));
        pane.diagnostics.cache_origin = "daemon_update".to_string();
        pane
    });

    if let Some(index) = snapshot
        .panes
        .iter()
        .position(|existing| existing.pane_id == pane_id)
    {
        if let Some(pane) = pane {
            snapshot.panes[index] = pane;
        } else {
            snapshot.panes.remove(index);
        }
    } else if let Some(pane) = pane {
        snapshot.panes.push(pane);
    }

    cache::sort_snapshot_panes(snapshot);
    cache::mark_snapshot_as_daemon(snapshot)
}

fn refresh_snapshot_window(snapshot: &mut SnapshotEnvelope, window_id: &str) -> Result<()> {
    refresh_snapshot_scope(snapshot, TargetScope::Window, window_id)
}

fn refresh_snapshot_session(snapshot: &mut SnapshotEnvelope, session_id: &str) -> Result<()> {
    refresh_snapshot_scope(snapshot, TargetScope::Session, session_id)
}

fn refresh_snapshot_scope(
    snapshot: &mut SnapshotEnvelope,
    scope: TargetScope,
    target_id: &str,
) -> Result<()> {
    let rows = tmux::tmux_list_panes_target(target_id)?;

    snapshot
        .panes
        .retain(|pane| !scope.matches(pane, target_id));

    if let Some(rows) = rows {
        let proc_inspector = proc::ProcProcessInspector;
        let mut panes = classify::panes_from_rows_with_proc_fallback(rows, &proc_inspector);
        scanner::apply_pane_output_status_fallbacks(&mut panes);
        snapshot.panes.extend(panes.into_iter().map(|mut pane| {
            pane.diagnostics.cache_origin = "daemon_update".to_string();
            pane
        }));
    }

    merge_cached_panes(snapshot, None);
    cache::sort_snapshot_panes(snapshot);
    cache::mark_snapshot_as_daemon(snapshot)
}

fn fallback_to_full_resnapshot(
    snapshot: &mut SnapshotEnvelope,
    line: &str,
    error: anyhow::Error,
) -> Result<()> {
    eprintln!(
        "agentscan: targeted refresh failed for control-mode line {:?}: {error:#}",
        line
    );
    reconcile_full_snapshot(snapshot)
}

fn reconcile_full_snapshot(snapshot: &mut SnapshotEnvelope) -> Result<()> {
    *snapshot = cache::daemon_snapshot_from_tmux()?;
    merge_cached_panes(snapshot, None);
    Ok(())
}

fn merge_cached_panes(snapshot: &mut SnapshotEnvelope, excluded_pane_id: Option<&str>) {
    let Some(existing) = cache::read_existing_snapshot_if_valid() else {
        return;
    };

    for pane in &mut snapshot.panes {
        if excluded_pane_id.is_some_and(|pane_id| pane.pane_id == pane_id) {
            continue;
        }

        if let Some(existing_pane) = existing
            .panes
            .iter()
            .find(|cached| cached.pane_id == pane.pane_id)
            && has_more_recent_helper_state(existing_pane, pane)
        {
            *pane = existing_pane.clone();
        }
    }
}

pub(crate) fn window_notification_target(line: &str) -> Option<&str> {
    match notification_name(line) {
        Some(
            "%layout-change"
            | "%window-add"
            | "%window-close"
            | "%unlinked-window-close"
            | "%unlinked-window-renamed"
            | "%window-pane-changed"
            | "%window-renamed",
        ) => line
            .split_whitespace()
            .nth(1)
            .filter(|value| value.starts_with('@')),
        _ => None,
    }
}

pub(crate) fn session_notification_target(line: &str) -> Option<&str> {
    match notification_name(line) {
        Some("%session-renamed") => line
            .split_whitespace()
            .nth(1)
            .filter(|value| value.starts_with('$')),
        _ => None,
    }
}

fn has_more_recent_helper_state(existing: &PaneRecord, current: &PaneRecord) -> bool {
    existing.agent_metadata.provider != current.agent_metadata.provider
        || existing.agent_metadata.label != current.agent_metadata.label
        || existing.agent_metadata.cwd != current.agent_metadata.cwd
        || existing.agent_metadata.state != current.agent_metadata.state
        || existing.agent_metadata.session_id != current.agent_metadata.session_id
}

pub(crate) fn notification_name(line: &str) -> Option<&str> {
    line.split_whitespace()
        .next()
        .filter(|token| token.starts_with('%'))
}

#[derive(Clone, Copy)]
enum TargetScope {
    Window,
    Session,
}

impl TargetScope {
    fn matches(self, pane: &PaneRecord, target_id: &str) -> bool {
        match self {
            Self::Window => pane.tmux.window_id.as_deref() == Some(target_id),
            Self::Session => pane.tmux.session_id.as_deref() == Some(target_id),
        }
    }
}
