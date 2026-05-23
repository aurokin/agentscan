use super::*;

#[derive(Clone)]
pub(super) struct LifecyclePaths {
    lock_path: PathBuf,
    start_lock_path: PathBuf,
    identity_path: PathBuf,
    log_path: PathBuf,
}

impl LifecyclePaths {
    pub(super) fn from_socket_path(socket_path: &Path) -> Self {
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
    Status(Box<ipc::LifecycleStatusFrame>),
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DaemonStartIntent {
    ExplicitLifecycleCommand,
    ImplicitConsumerAutoStart,
    TuiSubscriptionAutoStart,
}

impl DaemonStartIntent {
    #[cfg(any(test, target_os = "macos"))]
    fn requires_macos_trust_preflight(self) -> bool {
        matches!(self, Self::ExplicitLifecycleCommand)
    }
}

struct DaemonStartRequest<'a> {
    socket_path: &'a Path,
    output: StartOutput,
    intent: DaemonStartIntent,
    executable_path: &'a Path,
    envs: &'a [(OsString, OsString)],
    env_removes: &'a [OsString],
}

impl DaemonStartRequest<'_> {
    fn paths(&self) -> LifecyclePaths {
        LifecyclePaths::from_socket_path(self.socket_path)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum DaemonStartPolicyDecision {
    Allowed,
    Blocked(DaemonSnapshotError),
}

impl DaemonStartPolicyDecision {
    fn into_result(self) -> std::result::Result<(), DaemonSnapshotError> {
        match self {
            Self::Allowed => Ok(()),
            Self::Blocked(error) => Err(error),
        }
    }
}

#[cfg(any(test, target_os = "macos"))]
#[derive(Clone, Debug, Eq, PartialEq)]
enum MacExecutableAssessment {
    Trusted,
    Untrusted(String),
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

pub(super) struct DaemonLifecycleGuard {
    lock_file: File,
    identity_path: PathBuf,
    identity: ipc::DaemonIdentityFrame,
}

impl DaemonLifecycleGuard {
    pub(super) fn acquire(
        paths: &LifecyclePaths,
        identity: &DaemonRuntimeIdentity,
    ) -> Result<Self> {
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
                ipc::DaemonFrame::LifecycleStatus { status } => {
                    Ok(LifecycleQuery::Status(Box::new(status)))
                }
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
            SubscriptionConnect::NotRunning(reason)
                if macos_implicit_auto_start_is_disabled(
                    DaemonStartIntent::TuiSubscriptionAutoStart,
                ) =>
            {
                if let Err(error) = remove_stale_socket_if_present(&socket_path) {
                    send_subscription_event(
                        events,
                        DaemonSubscriptionEvent::Fatal {
                            message: error.to_string(),
                        },
                    )?;
                    break;
                }
                let message = macos_implicit_auto_start_disabled_reason(
                    DaemonStartIntent::TuiSubscriptionAutoStart,
                    &reason,
                );
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
            SubscriptionConnect::NotRunning(reason) if !attempted_start => {
                attempted_start = true;
                send_subscription_event(
                    events,
                    DaemonSubscriptionEvent::Connecting {
                        message: format!("starting daemon after {reason}"),
                    },
                )?;
                match daemon_start_with_socket_path_and_output(
                    &socket_path,
                    StartOutput::Quiet,
                    DaemonStartIntent::TuiSubscriptionAutoStart,
                ) {
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
                    if let Err(error) = snapshot::validate_snapshot(&snapshot) {
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
                if let Err(error) = snapshot::validate_snapshot(&snapshot) {
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
        daemon_start_with_socket_path_and_output(
            socket_path,
            StartOutput::Quiet,
            DaemonStartIntent::ImplicitConsumerAutoStart,
        )
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
                snapshot::validate_snapshot(&snapshot).map_err(|error| {
                    DaemonSnapshotError::Incompatible {
                        message: format!("daemon returned invalid snapshot: {error:#}"),
                    }
                })?;
                return Ok(snapshot);
            }
            SnapshotQuery::NotRunning(reason) if policy.disabled => {
                return Err(DaemonSnapshotError::AutoStartDisabled { reason });
            }
            SnapshotQuery::NotRunning(reason)
                if macos_implicit_auto_start_is_disabled(
                    DaemonStartIntent::ImplicitConsumerAutoStart,
                ) =>
            {
                remove_stale_socket_if_present(socket_path).map_err(|error| {
                    DaemonSnapshotError::UnexpectedFrame {
                        message: error.to_string(),
                    }
                })?;
                return Err(DaemonSnapshotError::AutoStartDisabled {
                    reason: macos_implicit_auto_start_disabled_reason(
                        DaemonStartIntent::ImplicitConsumerAutoStart,
                        &reason,
                    ),
                });
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

pub(crate) fn snapshot_via_socket_path_with_start_command(
    socket_path: &Path,
    executable_path: &Path,
    envs: &[(OsString, OsString)],
    env_removes: &[OsString],
) -> std::result::Result<SnapshotEnvelope, DaemonSnapshotError> {
    snapshot_via_socket_path_with_starter(socket_path, AutoStartPolicy::default(), |socket_path| {
        start_daemon(DaemonStartRequest {
            socket_path,
            output: StartOutput::Quiet,
            intent: DaemonStartIntent::ImplicitConsumerAutoStart,
            executable_path,
            envs,
            env_removes,
        })
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

#[derive(Serialize)]
struct DaemonStatusJson {
    daemon_state: String,
    socket_path: String,
    lock_path: String,
    start_lock_path: String,
    log_path: String,
    reason: Option<String>,
    pid: Option<u32>,
    daemon_start_time: Option<String>,
    executable: Option<String>,
    executable_canonical: Option<String>,
    protocol_version: Option<u32>,
    snapshot_schema_version: Option<u32>,
    subscriber_count: Option<usize>,
    latest_snapshot_generated_at: Option<String>,
    latest_snapshot_pane_count: Option<usize>,
    latest_snapshot_update_source: Option<String>,
    latest_snapshot_update_detail: Option<String>,
    latest_snapshot_update_duration_ms: Option<u64>,
    control_mode_broker_mode: Option<String>,
    control_mode_broker_disabled_reason: Option<String>,
    control_mode_broker_reconnect_count: Option<u32>,
    unavailable_reason: Option<String>,
    message: Option<String>,
}

fn lifecycle_not_running_json(
    socket_path: &Path,
    paths: &LifecyclePaths,
    reason: &str,
) -> DaemonStatusJson {
    DaemonStatusJson {
        daemon_state: "not_running".to_string(),
        socket_path: socket_path.display().to_string(),
        lock_path: paths.lock_path.display().to_string(),
        start_lock_path: paths.start_lock_path.display().to_string(),
        log_path: paths.log_path.display().to_string(),
        reason: Some(reason.to_string()),
        pid: None,
        daemon_start_time: None,
        executable: None,
        executable_canonical: None,
        protocol_version: None,
        snapshot_schema_version: None,
        subscriber_count: None,
        latest_snapshot_generated_at: None,
        latest_snapshot_pane_count: None,
        latest_snapshot_update_source: None,
        latest_snapshot_update_detail: None,
        latest_snapshot_update_duration_ms: None,
        control_mode_broker_mode: None,
        control_mode_broker_disabled_reason: None,
        control_mode_broker_reconnect_count: None,
        unavailable_reason: None,
        message: None,
    }
}

fn lifecycle_status_json(
    paths: &LifecyclePaths,
    status: &ipc::LifecycleStatusFrame,
) -> DaemonStatusJson {
    DaemonStatusJson {
        daemon_state: lifecycle_state_label(status.state).to_string(),
        socket_path: status.identity.socket_path.clone(),
        lock_path: paths.lock_path.display().to_string(),
        start_lock_path: paths.start_lock_path.display().to_string(),
        log_path: paths.log_path.display().to_string(),
        reason: None,
        pid: Some(status.identity.pid),
        daemon_start_time: Some(status.identity.daemon_start_time.clone()),
        executable: Some(status.identity.executable.clone()),
        executable_canonical: status.identity.executable_canonical.clone(),
        protocol_version: Some(status.identity.protocol_version),
        snapshot_schema_version: Some(status.identity.snapshot_schema_version),
        subscriber_count: Some(status.subscriber_count),
        latest_snapshot_generated_at: status.latest_snapshot_generated_at.clone(),
        latest_snapshot_pane_count: status.latest_snapshot_pane_count,
        latest_snapshot_update_source: status.latest_snapshot_update_source.clone(),
        latest_snapshot_update_detail: status.latest_snapshot_update_detail.clone(),
        latest_snapshot_update_duration_ms: status.latest_snapshot_update_duration_ms,
        control_mode_broker_mode: status
            .control_mode_broker
            .as_ref()
            .map(|broker| control_mode_broker_mode_label(broker.mode).to_string()),
        control_mode_broker_disabled_reason: status
            .control_mode_broker
            .as_ref()
            .and_then(|broker| broker.disabled_reason.clone()),
        control_mode_broker_reconnect_count: status
            .control_mode_broker
            .as_ref()
            .map(|broker| broker.reconnect_count),
        unavailable_reason: status
            .unavailable_reason
            .map(unavailable_reason_label)
            .map(str::to_string),
        message: status.message.clone(),
    }
}

fn emit_lifecycle_not_running(
    socket_path: &Path,
    paths: &LifecyclePaths,
    reason: &str,
    format: OutputFormat,
) -> Result<()> {
    match format {
        OutputFormat::Text => {
            print_lifecycle_not_running(socket_path, paths, reason);
            Ok(())
        }
        OutputFormat::Json => {
            output::print_json(&lifecycle_not_running_json(socket_path, paths, reason))
        }
    }
}

fn emit_lifecycle_status(
    paths: &LifecyclePaths,
    status: &ipc::LifecycleStatusFrame,
    format: OutputFormat,
) -> Result<()> {
    match format {
        OutputFormat::Text => {
            print_lifecycle_status(paths, status);
            Ok(())
        }
        OutputFormat::Json => output::print_json(&lifecycle_status_json(paths, status)),
    }
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

fn control_mode_broker_mode_label(mode: ipc::ControlModeBrokerMode) -> &'static str {
    match mode {
        ipc::ControlModeBrokerMode::Active => "active",
        ipc::ControlModeBrokerMode::Fallback => "fallback",
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
    if let Some(source) = &status.latest_snapshot_update_source {
        println!("latest_snapshot_update_source: {source}");
    }
    if let Some(detail) = &status.latest_snapshot_update_detail {
        println!("latest_snapshot_update_detail: {detail}");
    }
    if let Some(duration_ms) = status.latest_snapshot_update_duration_ms {
        println!("latest_snapshot_update_duration_ms: {duration_ms}");
    }
    if let Some(broker) = &status.control_mode_broker {
        println!(
            "control_mode_broker_mode: {}",
            control_mode_broker_mode_label(broker.mode)
        );
        println!(
            "control_mode_broker_reconnect_count: {}",
            broker.reconnect_count
        );
        if let Some(reason) = &broker.disabled_reason {
            println!("control_mode_broker_disabled_reason: {reason}");
        }
    }
    if let Some(reason) = status.unavailable_reason {
        println!("unavailable_reason: {}", unavailable_reason_label(reason));
    }
    if let Some(message) = &status.message {
        println!("message: {message}");
    }
}

pub(super) fn remove_stale_socket_if_present(socket_path: &Path) -> Result<()> {
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
        LifecycleQuery::Status(status) if status.identity == *expected_identity => Ok(*status),
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

pub(crate) fn daemon_run() -> Result<()> {
    let socket_path =
        ipc::resolve_socket_path().map_err(|error| DaemonSnapshotError::UnexpectedFrame {
            message: error.to_string(),
        })?;
    daemon_run_with_socket_path_and_startup(&socket_path, DaemonStartup)
}

pub(crate) fn daemon_status(format: OutputFormat) -> Result<()> {
    let socket_path = ipc::resolve_socket_path()?;
    daemon_status_with_socket_path(&socket_path, format)
}

pub(crate) fn daemon_status_with_socket_path(
    socket_path: &Path,
    format: OutputFormat,
) -> Result<()> {
    let paths = LifecyclePaths::from_socket_path(socket_path);
    match lifecycle_status_from_socket(socket_path, LIFECYCLE_CONNECT_TIMEOUT)? {
        LifecycleQuery::NotRunning(reason) => {
            emit_lifecycle_not_running(socket_path, &paths, &reason, format)
        }
        LifecycleQuery::Status(status) => emit_lifecycle_status(&paths, &status, format),
        LifecycleQuery::Incompatible(message) => {
            bail!("{}", incompatible_daemon_guidance(&message))
        }
        LifecycleQuery::Busy(message) => bail!("{message}"),
    }
}

pub(crate) fn daemon_start() -> Result<()> {
    daemon_start_with_output(StartOutput::Verbose).map_err(DaemonSnapshotError::into_anyhow)
}

fn daemon_start_with_output(output: StartOutput) -> std::result::Result<(), DaemonSnapshotError> {
    let socket_path =
        ipc::resolve_socket_path().map_err(|error| DaemonSnapshotError::UnexpectedFrame {
            message: error.to_string(),
        })?;
    daemon_start_with_socket_path_and_output(
        &socket_path,
        output,
        DaemonStartIntent::ExplicitLifecycleCommand,
    )
}

fn daemon_start_with_socket_path_and_output(
    socket_path: &Path,
    output: StartOutput,
    intent: DaemonStartIntent,
) -> std::result::Result<(), DaemonSnapshotError> {
    let executable_path =
        env::current_exe().map_err(|error| DaemonSnapshotError::UnexpectedFrame {
            message: format!("failed to resolve current executable: {error}"),
        })?;
    let envs = daemon_start_tmux_envs();
    let env_removes = daemon_start_env_removes();
    start_daemon(DaemonStartRequest {
        socket_path,
        output,
        intent,
        executable_path: &executable_path,
        envs: &envs,
        env_removes: &env_removes,
    })
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

#[cfg(test)]
pub(crate) fn test_implicit_consumer_macos_auto_start_preflight(
    rejection_reason: Option<&str>,
) -> std::result::Result<(), DaemonSnapshotError> {
    test_macos_auto_start_preflight(
        DaemonStartIntent::ImplicitConsumerAutoStart,
        rejection_reason,
    )
}

#[cfg(test)]
pub(crate) fn test_tui_macos_auto_start_preflight(
    rejection_reason: Option<&str>,
) -> std::result::Result<(), DaemonSnapshotError> {
    test_macos_auto_start_preflight(
        DaemonStartIntent::TuiSubscriptionAutoStart,
        rejection_reason,
    )
}

#[cfg(test)]
pub(crate) fn test_explicit_macos_daemon_start_preflight(
    rejection_reason: Option<&str>,
) -> std::result::Result<(), DaemonSnapshotError> {
    test_macos_auto_start_preflight(
        DaemonStartIntent::ExplicitLifecycleCommand,
        rejection_reason,
    )
}

#[cfg(test)]
fn test_macos_auto_start_preflight(
    intent: DaemonStartIntent,
    rejection_reason: Option<&str>,
) -> std::result::Result<(), DaemonSnapshotError> {
    let assessment = match rejection_reason {
        Some(reason) => MacExecutableAssessment::Untrusted(reason.to_string()),
        None => MacExecutableAssessment::Trusted,
    };
    daemon_start_policy_decision_from_macos_assessment(
        intent,
        Path::new("/tmp/agentscan"),
        assessment,
    )
    .into_result()
}

#[cfg(target_os = "macos")]
fn daemon_start_policy_decision(
    intent: DaemonStartIntent,
    executable_path: &Path,
    envs: &[(OsString, OsString)],
) -> DaemonStartPolicyDecision {
    let _ = envs;
    if macos_implicit_auto_start_is_disabled(intent) {
        return DaemonStartPolicyDecision::Blocked(DaemonSnapshotError::AutoStartDisabled {
            reason: macos_implicit_auto_start_disabled_reason(intent, "daemon is not running"),
        });
    }
    if !intent.requires_macos_trust_preflight() {
        return DaemonStartPolicyDecision::Allowed;
    }
    let assessment = assess_macos_executable_for_daemon_autostart(executable_path);
    daemon_start_policy_decision_from_macos_assessment(intent, executable_path, assessment)
}

#[cfg(test)]
pub(crate) fn test_macos_preflight_skips_assessment(explicit_start: bool, tui_start: bool) -> bool {
    let intent = match (explicit_start, tui_start) {
        (true, _) => DaemonStartIntent::ExplicitLifecycleCommand,
        (false, true) => DaemonStartIntent::TuiSubscriptionAutoStart,
        (false, false) => DaemonStartIntent::ImplicitConsumerAutoStart,
    };
    !intent.requires_macos_trust_preflight()
}

#[cfg(not(target_os = "macos"))]
fn daemon_start_policy_decision(
    intent: DaemonStartIntent,
    executable_path: &Path,
    envs: &[(OsString, OsString)],
) -> DaemonStartPolicyDecision {
    let _ = (intent, executable_path, envs);
    DaemonStartPolicyDecision::Allowed
}

fn daemon_start_preflight(
    intent: DaemonStartIntent,
    executable_path: &Path,
    envs: &[(OsString, OsString)],
) -> std::result::Result<(), DaemonSnapshotError> {
    daemon_start_policy_decision(intent, executable_path, envs).into_result()
}

#[cfg(any(test, target_os = "macos"))]
fn daemon_start_policy_decision_from_macos_assessment(
    intent: DaemonStartIntent,
    executable_path: &Path,
    assessment: MacExecutableAssessment,
) -> DaemonStartPolicyDecision {
    if macos_implicit_auto_start_is_disabled_for_policy_tests(intent) {
        return DaemonStartPolicyDecision::Blocked(DaemonSnapshotError::AutoStartDisabled {
            reason: macos_implicit_auto_start_disabled_reason(intent, "daemon is not running"),
        });
    }
    if !intent.requires_macos_trust_preflight() {
        return DaemonStartPolicyDecision::Allowed;
    }

    match assessment {
        MacExecutableAssessment::Trusted => DaemonStartPolicyDecision::Allowed,
        MacExecutableAssessment::Untrusted(reason) => {
            DaemonStartPolicyDecision::Blocked(DaemonSnapshotError::AutoStartDisabled {
                reason: format!(
                    "macOS executable trust preflight rejected detached daemon start for {}; {reason}. {}",
                    executable_path.display(),
                    macos_auto_start_recovery_guidance(intent)
                ),
            })
        }
    }
}

fn macos_daemon_run_guidance() -> &'static str {
    "Start the daemon in a long-lived tmux pane with `agentscan daemon run`"
}

fn macos_implicit_auto_start_disabled_reason(intent: DaemonStartIntent, reason: &str) -> String {
    let recovery = match intent {
        DaemonStartIntent::ImplicitConsumerAutoStart => {
            "Use `agentscan scan` or a refresh-capable command for one-shot direct tmux reads."
        }
        DaemonStartIntent::TuiSubscriptionAutoStart => {
            "The TUI requires a running daemon on macOS."
        }
        DaemonStartIntent::ExplicitLifecycleCommand => {
            "Use `agentscan daemon start` with a signed binary or `agentscan daemon run` in the foreground."
        }
    };
    format!(
        "{reason}; macOS does not implicitly auto-start the agentscan daemon. {} {recovery}",
        macos_daemon_run_guidance()
    )
}

fn macos_implicit_auto_start_is_disabled(intent: DaemonStartIntent) -> bool {
    cfg!(target_os = "macos")
        && matches!(
            intent,
            DaemonStartIntent::ImplicitConsumerAutoStart
                | DaemonStartIntent::TuiSubscriptionAutoStart
        )
}

#[cfg(any(test, target_os = "macos"))]
fn macos_implicit_auto_start_is_disabled_for_policy_tests(intent: DaemonStartIntent) -> bool {
    matches!(
        intent,
        DaemonStartIntent::ImplicitConsumerAutoStart | DaemonStartIntent::TuiSubscriptionAutoStart
    )
}

#[cfg(any(test, target_os = "macos"))]
fn macos_auto_start_recovery_guidance(intent: DaemonStartIntent) -> &'static str {
    match intent {
        DaemonStartIntent::ImplicitConsumerAutoStart => {
            "Run `agentscan scan`, pass `--refresh`, run `agentscan daemon run` in the foreground, or install a signed release binary for detached daemon operation"
        }
        DaemonStartIntent::TuiSubscriptionAutoStart => {
            "Run `agentscan scan`, run `agentscan daemon run` in the foreground, or install a signed release binary for detached daemon operation"
        }
        DaemonStartIntent::ExplicitLifecycleCommand => {
            "Run `agentscan daemon run` in the foreground, or install a signed release binary for detached daemon operation"
        }
    }
}

#[cfg(target_os = "macos")]
fn assess_macos_executable_for_daemon_autostart(path: &Path) -> MacExecutableAssessment {
    let display_output = match Command::new("/usr/bin/codesign")
        .args(["-dv", "--verbose=4"])
        .arg(path)
        .output()
    {
        Ok(output) => output,
        Err(error) => {
            return MacExecutableAssessment::Untrusted(format!(
                "codesign inspection could not run: {error}"
            ));
        }
    };
    let display_text = command_output_text(&display_output);
    if let Some(assessment) =
        macos_codesign_display_rejection(display_output.status.success(), &display_text)
    {
        return assessment;
    }

    let verify_output = match Command::new("/usr/bin/codesign")
        .args(["--verify", "--verbose=4"])
        .arg(path)
        .output()
    {
        Ok(output) => output,
        Err(error) => {
            return MacExecutableAssessment::Untrusted(format!(
                "codesign verification could not run: {error}"
            ));
        }
    };
    if let MacExecutableAssessment::Untrusted(reason) = assess_macos_codesign_verification(
        verify_output.status.success(),
        &command_output_text(&verify_output),
    ) {
        return MacExecutableAssessment::Untrusted(reason);
    }

    MacExecutableAssessment::Trusted
}

#[cfg(any(test, target_os = "macos"))]
fn macos_codesign_display_rejection(
    status_success: bool,
    text: &str,
) -> Option<MacExecutableAssessment> {
    if !status_success {
        return Some(MacExecutableAssessment::Untrusted(format!(
            "codesign inspection failed: {}",
            compact_command_output(text)
        )));
    }

    let lower = text.to_ascii_lowercase();
    if lower.contains("signature=adhoc") || lower.contains("(adhoc") || lower.contains("adhoc,") {
        return Some(MacExecutableAssessment::Untrusted(format!(
            "codesign reports an ad-hoc executable: {}",
            compact_command_output(text)
        )));
    }
    None
}

#[cfg(any(test, target_os = "macos"))]
fn assess_macos_codesign_verification(status_success: bool, text: &str) -> MacExecutableAssessment {
    if status_success {
        MacExecutableAssessment::Trusted
    } else {
        MacExecutableAssessment::Untrusted(format!(
            "codesign verification failed: {}",
            compact_command_output(text)
        ))
    }
}

#[cfg(test)]
pub(crate) fn test_macos_executable_assessment_for_outputs(
    display_status_success: bool,
    display_text: &str,
    verify_status_success: bool,
    verify_text: &str,
) -> std::result::Result<(), String> {
    if let Some(MacExecutableAssessment::Untrusted(reason)) =
        macos_codesign_display_rejection(display_status_success, display_text)
    {
        return Err(reason);
    }

    if let MacExecutableAssessment::Untrusted(reason) =
        assess_macos_codesign_verification(verify_status_success, verify_text)
    {
        return Err(reason);
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn command_output_text(output: &std::process::Output) -> String {
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    if !output.stdout.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&String::from_utf8_lossy(&output.stdout));
    }
    text
}

#[cfg(any(test, target_os = "macos"))]
fn compact_command_output(text: &str) -> String {
    let compact = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("; ");
    if compact.chars().count() > 500 {
        let prefix = compact.chars().take(500).collect::<String>();
        format!("{prefix}...")
    } else {
        compact
    }
}

fn log_daemon_start_policy_decision(
    paths: &LifecyclePaths,
    request: &DaemonStartRequest<'_>,
    decision: &DaemonStartPolicyDecision,
) {
    let Ok(mut log) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.log_path)
    else {
        return;
    };
    let timestamp = snapshot::now_rfc3339().unwrap_or_else(|_| "unknown".to_string());
    let executable_canonical = fs::canonicalize(request.executable_path)
        .ok()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "<unavailable>".to_string());
    let env_keys = request
        .envs
        .iter()
        .map(|(key, _)| key.to_string_lossy())
        .collect::<Vec<_>>()
        .join(",");
    let env_remove_keys = request
        .env_removes
        .iter()
        .map(|key| key.to_string_lossy())
        .collect::<Vec<_>>()
        .join(",");
    let result = match decision {
        DaemonStartPolicyDecision::Allowed => "allowed".to_string(),
        DaemonStartPolicyDecision::Blocked(error) => format!("blocked: {error}"),
    };
    let _ = writeln!(
        log,
        "daemon_start_preflight timestamp={timestamp} parent_pid={} intent={:?} target_os={} socket_path={} executable_path={} executable_canonical={} envs={} env_removes={} result={result}",
        std::process::id(),
        request.intent,
        std::env::consts::OS,
        request.socket_path.display(),
        request.executable_path.display(),
        executable_canonical,
        env_keys,
        env_remove_keys,
    );
}

fn start_daemon(request: DaemonStartRequest<'_>) -> std::result::Result<(), DaemonSnapshotError> {
    let paths = request.paths();

    if daemon_start_existing_status(request.socket_path, &paths, request.output)? {
        return Ok(());
    }
    let _start_guard = DaemonStartGuard::acquire(&paths).map_err(|error| {
        DaemonSnapshotError::UnexpectedFrame {
            message: error.to_string(),
        }
    })?;
    if daemon_start_existing_status(request.socket_path, &paths, request.output)? {
        return Ok(());
    }

    remove_stale_socket_if_present(request.socket_path).map_err(|error| {
        DaemonSnapshotError::UnexpectedFrame {
            message: error.to_string(),
        }
    })?;
    prepare_log_file(&paths.log_path).map_err(|error| DaemonSnapshotError::UnexpectedFrame {
        message: error.to_string(),
    })?;
    let decision =
        daemon_start_policy_decision(request.intent, request.executable_path, request.envs);
    log_daemon_start_policy_decision(&paths, &request, &decision);
    decision.into_result()?;

    let mut child = spawn_detached_daemon_process(&paths, &request)?;
    match wait_for_daemon_start(request.socket_path, &paths, &mut child, request.output) {
        Ok(()) => Ok(()),
        Err(error) => {
            cleanup_detached_daemon_child(&mut child);
            Err(error)
        }
    }
}

fn spawn_detached_daemon_process(
    paths: &LifecyclePaths,
    request: &DaemonStartRequest<'_>,
) -> std::result::Result<std::process::Child, DaemonSnapshotError> {
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

    let mut command = Command::new(request.executable_path);
    command
        .args(["daemon", "run"])
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(log_stdout))
        .stderr(Stdio::from(log_stderr));
    command.envs(request.envs.iter().cloned());
    for key in request.env_removes {
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

    command
        .spawn()
        .context("failed to start daemon process")
        .map_err(|error| DaemonSnapshotError::UnexpectedFrame {
            message: error.to_string(),
        })
}

pub(crate) fn daemon_stop() -> Result<()> {
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

pub(crate) fn daemon_restart() -> Result<()> {
    daemon_restart_with_steps(
        || daemon_start_preflight_for_current_command(DaemonStartIntent::ExplicitLifecycleCommand),
        daemon_stop,
        daemon_start,
    )
}

fn daemon_restart_with_steps(
    preflight: impl FnOnce() -> std::result::Result<(), DaemonSnapshotError>,
    stop: impl FnOnce() -> Result<()>,
    start: impl FnOnce() -> Result<()>,
) -> Result<()> {
    preflight().map_err(DaemonSnapshotError::into_anyhow)?;
    stop()?;
    start()
}

fn daemon_start_preflight_for_current_command(
    intent: DaemonStartIntent,
) -> std::result::Result<(), DaemonSnapshotError> {
    let executable_path =
        env::current_exe().map_err(|error| DaemonSnapshotError::UnexpectedFrame {
            message: format!("failed to resolve current executable: {error}"),
        })?;
    let envs = daemon_start_tmux_envs();
    daemon_start_preflight(intent, &executable_path, &envs)
}

#[cfg(test)]
pub(crate) fn test_daemon_restart_with_steps(
    preflight: impl FnOnce() -> std::result::Result<(), DaemonSnapshotError>,
    stop: impl FnOnce() -> Result<()>,
    start: impl FnOnce() -> Result<()>,
) -> Result<()> {
    daemon_restart_with_steps(preflight, stop, start)
}
