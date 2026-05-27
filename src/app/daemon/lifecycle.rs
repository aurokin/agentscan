use super::*;

#[derive(Clone)]
pub(super) struct LifecyclePaths {
    lock_path: PathBuf,
    start_lock_path: PathBuf,
    identity_path: PathBuf,
    log_path: PathBuf,
    event_log_path: PathBuf,
}

impl LifecyclePaths {
    pub(super) fn from_socket_path(socket_path: &Path) -> Self {
        Self {
            lock_path: socket_path.with_extension("sock.lock"),
            start_lock_path: socket_path.with_extension("sock.start.lock"),
            identity_path: socket_path.with_extension("sock.identity.json"),
            log_path: socket_path.with_extension("sock.log"),
            event_log_path: socket_path.with_extension("sock.events.jsonl"),
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
        let _ = self;
        true
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

struct DaemonStartCoordinator {
    executable_path: PathBuf,
    envs: Vec<(OsString, OsString)>,
    env_removes: Vec<OsString>,
}

impl DaemonStartCoordinator {
    fn from_current_process() -> std::result::Result<Self, DaemonSnapshotError> {
        let executable_path =
            env::current_exe().map_err(|error| DaemonSnapshotError::UnexpectedFrame {
                message: format!("failed to resolve current executable: {error}"),
            })?;
        Ok(Self {
            executable_path,
            envs: daemon_start_tmux_envs(),
            env_removes: daemon_start_env_removes(),
        })
    }

    fn from_command(
        executable_path: &Path,
        envs: &[(OsString, OsString)],
        env_removes: &[OsString],
    ) -> Self {
        Self {
            executable_path: executable_path.to_path_buf(),
            envs: envs.to_vec(),
            env_removes: env_removes.to_vec(),
        }
    }

    fn start(
        &self,
        socket_path: &Path,
        output: StartOutput,
        intent: DaemonStartIntent,
    ) -> std::result::Result<(), DaemonSnapshotError> {
        start_daemon_from_request(self.request(socket_path, output, intent))
    }

    fn request<'a>(
        &'a self,
        socket_path: &'a Path,
        output: StartOutput,
        intent: DaemonStartIntent,
    ) -> DaemonStartRequest<'a> {
        DaemonStartRequest {
            socket_path,
            output,
            intent,
            executable_path: &self.executable_path,
            envs: &self.envs,
            env_removes: &self.env_removes,
        }
    }
}

enum DaemonStartCoordinatorSource<'a> {
    CurrentProcess,
    Command(&'a DaemonStartCoordinator),
}

impl DaemonStartCoordinatorSource<'_> {
    fn start(
        &self,
        socket_path: &Path,
        output: StartOutput,
        intent: DaemonStartIntent,
    ) -> std::result::Result<(), DaemonSnapshotError> {
        match self {
            Self::CurrentProcess => {
                DaemonStartCoordinator::from_current_process()?.start(socket_path, output, intent)
            }
            Self::Command(coordinator) => coordinator.start(socket_path, output, intent),
        }
    }
}

#[cfg(test)]
mod start_coordinator_tests {
    use super::*;

    #[test]
    fn coordinator_request_preserves_spawn_context_for_all_intents() {
        let coordinator = DaemonStartCoordinator::from_command(
            Path::new("/tmp/agentscan"),
            &[(
                OsString::from(TMUX_SOCKET_ENV_VAR),
                OsString::from("/tmp/tmux.sock"),
            )],
            &[OsString::from("TMUX")],
        );
        let socket_path = Path::new("/tmp/agentscan.sock");

        for intent in [
            DaemonStartIntent::ExplicitLifecycleCommand,
            DaemonStartIntent::ImplicitConsumerAutoStart,
            DaemonStartIntent::TuiSubscriptionAutoStart,
        ] {
            let request = coordinator.request(socket_path, StartOutput::Quiet, intent);

            assert_eq!(request.socket_path, socket_path);
            assert_eq!(request.output, StartOutput::Quiet);
            assert_eq!(request.intent, intent);
            assert_eq!(request.executable_path, Path::new("/tmp/agentscan"));
            assert_eq!(
                request.envs,
                &[(
                    OsString::from(TMUX_SOCKET_ENV_VAR),
                    OsString::from("/tmp/tmux.sock")
                )]
            );
            assert_eq!(request.env_removes, &[OsString::from("TMUX")]);
        }
    }

    #[test]
    fn coordinator_from_command_owns_spawn_inputs() {
        let mut envs = vec![(OsString::from("A"), OsString::from("1"))];
        let mut env_removes = vec![OsString::from("TMUX")];
        let coordinator =
            DaemonStartCoordinator::from_command(Path::new("/tmp/agentscan"), &envs, &env_removes);

        envs.push((OsString::from("B"), OsString::from("2")));
        env_removes.push(OsString::from("AGENTSCAN_TMUX_SOCKET"));

        let request = coordinator.request(
            Path::new("/tmp/agentscan.sock"),
            StartOutput::Verbose,
            DaemonStartIntent::ExplicitLifecycleCommand,
        );

        assert_eq!(request.envs, &[(OsString::from("A"), OsString::from("1"))]);
        assert_eq!(request.env_removes, &[OsString::from("TMUX")]);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum DaemonStartPolicyDecision {
    Allowed,
    #[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
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

struct SubscriptionState {
    bootstrapped: bool,
    attempted_start: bool,
    backoff: Duration,
}

impl SubscriptionState {
    fn new() -> Self {
        Self {
            bootstrapped: false,
            attempted_start: false,
            backoff: TUI_SUBSCRIPTION_INITIAL_BACKOFF,
        }
    }

    fn connecting_event(&self, socket_path: &Path) -> LiveClientEvent {
        LiveClientEvent::Connecting {
            message: if self.bootstrapped {
                format!("reconnecting to daemon at {}", socket_path.display())
            } else {
                format!("connecting to daemon at {}", socket_path.display())
            },
        }
    }

    fn mark_subscribed(&mut self) {
        self.attempted_start = false;
        self.backoff = TUI_SUBSCRIPTION_INITIAL_BACKOFF;
        self.bootstrapped = true;
    }

    fn can_attempt_start(&self) -> bool {
        !self.attempted_start
    }

    fn is_bootstrapped(&self) -> bool {
        self.bootstrapped
    }

    fn mark_start_attempted(&mut self) {
        self.attempted_start = true;
    }

    fn reset_start_attempt_after_retry(&mut self) {
        self.attempted_start = false;
    }

    fn auto_start_disabled_event(&self, reason: String) -> LiveClientEvent {
        let message = format!("daemon auto-start is disabled: {reason}");
        if self.bootstrapped {
            LiveClientEvent::Offline {
                message,
                retrying: false,
            }
        } else {
            LiveClientEvent::Fatal { message }
        }
    }

    fn post_bootstrap_auto_start_refusal_event(&self, reason: String) -> LiveClientEvent {
        let message = format!("daemon auto-start is disabled: {reason}");
        if self.bootstrapped {
            LiveClientEvent::Offline {
                message,
                retrying: true,
            }
        } else {
            LiveClientEvent::Fatal { message }
        }
    }

    fn offline_retrying_event(message: String) -> LiveClientEvent {
        LiveClientEvent::Offline {
            message,
            retrying: true,
        }
    }

    fn unexpected_event(&self, message: String) -> LiveClientEvent {
        if self.bootstrapped {
            Self::offline_retrying_event(message)
        } else {
            LiveClientEvent::Fatal { message }
        }
    }

    fn stops_after_unexpected(&self) -> bool {
        !self.bootstrapped
    }

    fn sleep_and_advance_backoff(&mut self, cancel: &AtomicBool) {
        sleep_subscription_backoff(cancel, self.backoff);
        self.advance_backoff();
    }

    fn advance_backoff(&mut self) {
        self.backoff = next_subscription_backoff(self.backoff);
    }
}

#[cfg(test)]
mod subscription_state_tests {
    use super::*;

    fn assert_fatal(event: LiveClientEvent, expected_message: &str) {
        match event {
            LiveClientEvent::Fatal { message } => {
                assert!(message.contains(expected_message), "{message}");
            }
            other => panic!("expected fatal event, got {other:?}"),
        }
    }

    fn assert_offline(event: LiveClientEvent, expected_message: &str, expected_retrying: bool) {
        match event {
            LiveClientEvent::Offline { message, retrying } => {
                assert!(message.contains(expected_message), "{message}");
                assert_eq!(retrying, expected_retrying);
            }
            other => panic!("expected offline event, got {other:?}"),
        }
    }

    #[test]
    fn subscription_auto_start_disabled_is_fatal_before_bootstrap() {
        let state = SubscriptionState::new();

        assert_fatal(
            state.auto_start_disabled_event("socket is missing".to_string()),
            "daemon auto-start is disabled: socket is missing",
        );
    }

    #[test]
    fn subscription_auto_start_disabled_is_terminal_offline_after_bootstrap() {
        let mut state = SubscriptionState::new();
        state.mark_subscribed();

        assert_offline(
            state.auto_start_disabled_event("socket is missing".to_string()),
            "daemon auto-start is disabled: socket is missing",
            false,
        );
    }

    #[test]
    fn subscription_policy_refusal_after_bootstrap_retries_and_can_start_again() {
        let mut state = SubscriptionState::new();
        state.mark_subscribed();
        state.mark_start_attempted();

        assert_offline(
            state.post_bootstrap_auto_start_refusal_event("codesign failed".to_string()),
            "daemon auto-start is disabled: codesign failed",
            true,
        );

        assert!(!state.can_attempt_start());
        state.reset_start_attempt_after_retry();
        assert!(state.can_attempt_start());
    }

    #[test]
    fn subscription_mark_subscribed_resets_start_attempt_and_backoff() {
        let mut state = SubscriptionState::new();
        state.mark_start_attempted();
        state.advance_backoff();
        assert_ne!(state.backoff, TUI_SUBSCRIPTION_INITIAL_BACKOFF);

        state.mark_subscribed();

        assert!(state.can_attempt_start());
        assert_eq!(state.backoff, TUI_SUBSCRIPTION_INITIAL_BACKOFF);
        assert!(state.is_bootstrapped());
    }

    #[test]
    fn subscription_retry_backoff_caps() {
        let mut state = SubscriptionState::new();

        for _ in 0..20 {
            state.advance_backoff();
        }

        assert_eq!(state.backoff, TUI_SUBSCRIPTION_MAX_BACKOFF);
    }
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
    events: mpsc::Sender<LiveClientEvent>,
    cancel: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let result = subscription_worker_loop(policy, &events, &cancel);
        if let Err(error) = result {
            let _ = events.send(LiveClientEvent::Fatal {
                message: error.to_string(),
            });
        }
    })
}

pub(crate) fn stream_subscription_events_json(
    policy: AutoStartPolicy,
) -> std::result::Result<(), DaemonSnapshotError> {
    let (events_tx, events_rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker = spawn_subscription_worker(policy, events_tx, Arc::clone(&cancel));
    let result = write_subscription_events_json(events_rx, &cancel);
    cancel.store(true, Ordering::Relaxed);
    match result {
        Ok(SubscriptionStreamCompletion::StdoutClosed) => Ok(()),
        Ok(SubscriptionStreamCompletion::WorkerFinished) => {
            let _ = worker.join();
            Ok(())
        }
        Err(error) => Err(error),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SubscriptionStreamCompletion {
    StdoutClosed,
    WorkerFinished,
}

fn write_subscription_events_json(
    events: mpsc::Receiver<LiveClientEvent>,
    cancel: &AtomicBool,
) -> std::result::Result<SubscriptionStreamCompletion, DaemonSnapshotError> {
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();

    while let Ok(event) = events.recv() {
        if !write_subscription_event_json_line(&mut stdout, &event, cancel)? {
            return Ok(SubscriptionStreamCompletion::StdoutClosed);
        }

        match event {
            LiveClientEvent::Fatal { message } => {
                return Err(DaemonSnapshotError::UnexpectedFrame { message });
            }
            LiveClientEvent::Shutdown { .. } => {
                return Ok(SubscriptionStreamCompletion::WorkerFinished);
            }
            LiveClientEvent::Connecting { .. }
            | LiveClientEvent::Snapshot { .. }
            | LiveClientEvent::Offline { .. } => {}
        }
    }

    Ok(SubscriptionStreamCompletion::WorkerFinished)
}

fn write_subscription_event_json_line(
    writer: &mut impl Write,
    event: &LiveClientEvent,
    cancel: &AtomicBool,
) -> std::result::Result<bool, DaemonSnapshotError> {
    if let Err(error) = serde_json::to_writer(&mut *writer, event) {
        if error
            .io_error_kind()
            .is_some_and(|kind| kind == std::io::ErrorKind::BrokenPipe)
        {
            cancel.store(true, Ordering::Relaxed);
            return Ok(false);
        }
        return Err(DaemonSnapshotError::UnexpectedFrame {
            message: format!("failed to encode subscription event: {error}"),
        });
    }
    if let Err(error) = writer.write_all(b"\n").and_then(|()| writer.flush()) {
        if error.kind() == std::io::ErrorKind::BrokenPipe {
            cancel.store(true, Ordering::Relaxed);
            return Ok(false);
        }
        return Err(DaemonSnapshotError::UnexpectedFrame {
            message: format!("failed to write subscription event: {error}"),
        });
    }
    Ok(true)
}

fn subscription_worker_loop(
    policy: AutoStartPolicy,
    events: &mpsc::Sender<LiveClientEvent>,
    cancel: &Arc<AtomicBool>,
) -> Result<()> {
    let socket_path = ipc::resolve_socket_path()?;
    let paths = LifecyclePaths::from_socket_path(&socket_path);
    let mut state = SubscriptionState::new();

    while !cancel.load(Ordering::Relaxed) {
        send_subscription_event(events, state.connecting_event(&socket_path))?;

        match subscribe_once_from_socket(&socket_path)? {
            SubscriptionConnect::Subscribed {
                mut reader,
                bootstrap,
            } => {
                state.mark_subscribed();
                send_subscription_event(
                    events,
                    LiveClientEvent::Snapshot {
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
                            LiveClientEvent::Offline {
                                message,
                                retrying: true,
                            },
                        )?;
                    }
                    SubscriptionReadResult::Shutdown(message) => {
                        send_subscription_event(events, LiveClientEvent::Shutdown { message })?;
                        break;
                    }
                    SubscriptionReadResult::Cancelled => break,
                }
            }
            SubscriptionConnect::NotRunning(reason) if policy.disabled => {
                send_subscription_event(events, state.auto_start_disabled_event(reason))?;
                break;
            }
            SubscriptionConnect::NotRunning(reason) if state.can_attempt_start() => {
                state.mark_start_attempted();
                send_subscription_event(
                    events,
                    LiveClientEvent::Connecting {
                        message: format!("starting daemon after {reason}"),
                    },
                )?;
                let coordinator = DaemonStartCoordinator::from_current_process()?;
                match coordinator.start(
                    &socket_path,
                    StartOutput::Quiet,
                    DaemonStartIntent::TuiSubscriptionAutoStart,
                ) {
                    Ok(()) => {}
                    Err(DaemonSnapshotError::AutoStartDisabled { reason })
                        if state.is_bootstrapped() =>
                    {
                        send_subscription_event(
                            events,
                            state.post_bootstrap_auto_start_refusal_event(reason),
                        )?;
                        state.sleep_and_advance_backoff(cancel);
                        state.reset_start_attempt_after_retry();
                    }
                    Err(error) => {
                        send_subscription_event(
                            events,
                            LiveClientEvent::Fatal {
                                message: error.to_string(),
                            },
                        )?;
                        break;
                    }
                }
            }
            SubscriptionConnect::NotRunning(reason) => {
                send_subscription_event(events, SubscriptionState::offline_retrying_event(reason))?;
                state.sleep_and_advance_backoff(cancel);
            }
            SubscriptionConnect::Retryable(message) => {
                send_subscription_event(
                    events,
                    SubscriptionState::offline_retrying_event(message),
                )?;
                state.sleep_and_advance_backoff(cancel);
            }
            SubscriptionConnect::StartupFailed(message) => {
                send_subscription_event(
                    events,
                    LiveClientEvent::Fatal {
                        message: format!(
                            "daemon startup failed: {message}; see log {}",
                            paths.log_path.display()
                        ),
                    },
                )?;
                break;
            }
            SubscriptionConnect::ServerClosing(message) => {
                send_subscription_event(events, LiveClientEvent::Shutdown { message })?;
                break;
            }
            SubscriptionConnect::Incompatible(message) => {
                send_subscription_event(
                    events,
                    LiveClientEvent::Fatal {
                        message: incompatible_daemon_guidance(&message),
                    },
                )?;
                break;
            }
            SubscriptionConnect::Unexpected(message) => {
                send_subscription_event(events, state.unexpected_event(message))?;
                if state.stops_after_unexpected() {
                    break;
                }
                state.sleep_and_advance_backoff(cancel);
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
    events: &mpsc::Sender<LiveClientEvent>,
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
                if send_subscription_event(events, LiveClientEvent::Snapshot { snapshot }).is_err()
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
                    || error_chain_contains_io_kind(&error, std::io::ErrorKind::WouldBlock) =>
            {
                sleep_subscription_backoff(cancel, Duration::from_millis(10));
            }
            Err(error) => {
                return SubscriptionReadResult::Reconnect(format!(
                    "daemon subscription read failed: {error:#}"
                ));
            }
        }
    }
}

fn send_subscription_event(
    events: &mpsc::Sender<LiveClientEvent>,
    event: LiveClientEvent,
) -> std::result::Result<(), mpsc::SendError<LiveClientEvent>> {
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
    snapshot_via_socket_path_with_coordinator(
        socket_path,
        policy,
        DaemonStartCoordinatorSource::CurrentProcess,
    )
}

fn snapshot_via_socket_path_with_coordinator(
    socket_path: &Path,
    policy: AutoStartPolicy,
    coordinator: DaemonStartCoordinatorSource<'_>,
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
            SnapshotQuery::NotRunning(reason) if !attempted_start => {
                attempted_start = true;
                coordinator.start(
                    socket_path,
                    StartOutput::Quiet,
                    DaemonStartIntent::ImplicitConsumerAutoStart,
                )?;
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
    let coordinator = DaemonStartCoordinator::from_command(executable_path, envs, env_removes);
    snapshot_via_socket_path_with_coordinator(
        socket_path,
        AutoStartPolicy::default(),
        DaemonStartCoordinatorSource::Command(&coordinator),
    )
}

fn print_lifecycle_not_running(socket_path: &Path, paths: &LifecyclePaths, reason: &str) {
    println!("daemon_state: not_running");
    println!("socket_path: {}", socket_path.display());
    println!("lock_path: {}", paths.lock_path.display());
    println!("start_lock_path: {}", paths.start_lock_path.display());
    println!("log_path: {}", paths.log_path.display());
    println!("event_log_path: {}", paths.event_log_path.display());
    println!("reason: {reason}");
}

#[derive(Serialize)]
struct DaemonStatusJson {
    daemon_state: String,
    socket_path: String,
    lock_path: String,
    start_lock_path: String,
    log_path: String,
    event_log_path: String,
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
    control_mode_broker_fallback_count: Option<u64>,
    control_mode_broker_subscriber_count: Option<usize>,
    control_event_refresh_count: Option<u64>,
    control_event_batch_count: Option<u64>,
    control_event_line_count: Option<u64>,
    control_event_output_line_count: Option<u64>,
    control_event_output_byte_count: Option<u64>,
    control_event_pane_count: Option<u64>,
    control_event_title_count: Option<u64>,
    control_event_window_count: Option<u64>,
    control_event_session_count: Option<u64>,
    control_event_resnapshot_count: Option<u64>,
    control_event_ignored_count: Option<u64>,
    reconcile_attempt_count: Option<u64>,
    reconcile_noop_count: Option<u64>,
    reconcile_changed_snapshot_count: Option<u64>,
    targeted_title_update_count: Option<u64>,
    targeted_pane_refresh_count: Option<u64>,
    targeted_scope_refresh_count: Option<u64>,
    full_snapshot_refresh_count: Option<u64>,
    targeted_refresh_fallback_to_full_count: Option<u64>,
    broker_fallback_count: Option<u64>,
    pane_output_capture_attempt_count: Option<u64>,
    pane_output_capture_hit_count: Option<u64>,
    pane_output_capture_error_count: Option<u64>,
    latest_snapshot_observability: Option<ipc::SnapshotObservabilityFrame>,
    recent_events: Option<Vec<ipc::DaemonObservabilityEventFrame>>,
    unavailable_reason: Option<String>,
    message: Option<String>,
}

fn lifecycle_not_running_json(
    socket_path: &Path,
    paths: &LifecyclePaths,
    reason: &str,
    include_events: bool,
) -> DaemonStatusJson {
    DaemonStatusJson {
        daemon_state: "not_running".to_string(),
        socket_path: socket_path.display().to_string(),
        lock_path: paths.lock_path.display().to_string(),
        start_lock_path: paths.start_lock_path.display().to_string(),
        log_path: paths.log_path.display().to_string(),
        event_log_path: paths.event_log_path.display().to_string(),
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
        control_mode_broker_fallback_count: None,
        control_mode_broker_subscriber_count: None,
        control_event_refresh_count: None,
        control_event_batch_count: None,
        control_event_line_count: None,
        control_event_output_line_count: None,
        control_event_output_byte_count: None,
        control_event_pane_count: None,
        control_event_title_count: None,
        control_event_window_count: None,
        control_event_session_count: None,
        control_event_resnapshot_count: None,
        control_event_ignored_count: None,
        reconcile_attempt_count: None,
        reconcile_noop_count: None,
        reconcile_changed_snapshot_count: None,
        targeted_title_update_count: None,
        targeted_pane_refresh_count: None,
        targeted_scope_refresh_count: None,
        full_snapshot_refresh_count: None,
        targeted_refresh_fallback_to_full_count: None,
        broker_fallback_count: None,
        pane_output_capture_attempt_count: None,
        pane_output_capture_hit_count: None,
        pane_output_capture_error_count: None,
        latest_snapshot_observability: None,
        recent_events: include_events.then(Vec::new),
        unavailable_reason: None,
        message: None,
    }
}

fn lifecycle_status_json(
    paths: &LifecyclePaths,
    status: &ipc::LifecycleStatusFrame,
    include_events: bool,
) -> DaemonStatusJson {
    DaemonStatusJson {
        daemon_state: lifecycle_state_label(status.state).to_string(),
        socket_path: status.identity.socket_path.clone(),
        lock_path: paths.lock_path.display().to_string(),
        start_lock_path: paths.start_lock_path.display().to_string(),
        log_path: paths.log_path.display().to_string(),
        event_log_path: paths.event_log_path.display().to_string(),
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
        control_mode_broker_fallback_count: status
            .control_mode_broker
            .as_ref()
            .and_then(|broker| broker.fallback_count),
        control_mode_broker_subscriber_count: status
            .control_mode_broker
            .as_ref()
            .and_then(|broker| broker.subscriber_count),
        control_event_refresh_count: status
            .runtime_telemetry
            .as_ref()
            .map(|telemetry| telemetry.control_event_refresh_count),
        control_event_batch_count: status
            .runtime_telemetry
            .as_ref()
            .map(|telemetry| telemetry.control_event_batch_count),
        control_event_line_count: status
            .runtime_telemetry
            .as_ref()
            .map(|telemetry| telemetry.control_event_line_count),
        control_event_output_line_count: status
            .runtime_telemetry
            .as_ref()
            .map(|telemetry| telemetry.control_event_output_line_count),
        control_event_output_byte_count: status
            .runtime_telemetry
            .as_ref()
            .map(|telemetry| telemetry.control_event_output_byte_count),
        control_event_pane_count: status
            .runtime_telemetry
            .as_ref()
            .map(|telemetry| telemetry.control_event_pane_count),
        control_event_title_count: status
            .runtime_telemetry
            .as_ref()
            .map(|telemetry| telemetry.control_event_title_count),
        control_event_window_count: status
            .runtime_telemetry
            .as_ref()
            .map(|telemetry| telemetry.control_event_window_count),
        control_event_session_count: status
            .runtime_telemetry
            .as_ref()
            .map(|telemetry| telemetry.control_event_session_count),
        control_event_resnapshot_count: status
            .runtime_telemetry
            .as_ref()
            .map(|telemetry| telemetry.control_event_resnapshot_count),
        control_event_ignored_count: status
            .runtime_telemetry
            .as_ref()
            .map(|telemetry| telemetry.control_event_ignored_count),
        reconcile_attempt_count: status
            .runtime_telemetry
            .as_ref()
            .map(|telemetry| telemetry.reconcile_attempt_count),
        reconcile_noop_count: status
            .runtime_telemetry
            .as_ref()
            .map(|telemetry| telemetry.reconcile_noop_count),
        reconcile_changed_snapshot_count: status
            .runtime_telemetry
            .as_ref()
            .map(|telemetry| telemetry.reconcile_changed_snapshot_count),
        targeted_title_update_count: status
            .runtime_telemetry
            .as_ref()
            .map(|telemetry| telemetry.targeted_title_update_count),
        targeted_pane_refresh_count: status
            .runtime_telemetry
            .as_ref()
            .map(|telemetry| telemetry.targeted_pane_refresh_count),
        targeted_scope_refresh_count: status
            .runtime_telemetry
            .as_ref()
            .map(|telemetry| telemetry.targeted_scope_refresh_count),
        full_snapshot_refresh_count: status
            .runtime_telemetry
            .as_ref()
            .map(|telemetry| telemetry.full_snapshot_refresh_count),
        targeted_refresh_fallback_to_full_count: status
            .runtime_telemetry
            .as_ref()
            .map(|telemetry| telemetry.targeted_refresh_fallback_to_full_count),
        broker_fallback_count: status
            .runtime_telemetry
            .as_ref()
            .map(|telemetry| telemetry.broker_fallback_count),
        pane_output_capture_attempt_count: status
            .runtime_telemetry
            .as_ref()
            .map(|telemetry| telemetry.pane_output_capture_attempt_count),
        pane_output_capture_hit_count: status
            .runtime_telemetry
            .as_ref()
            .map(|telemetry| telemetry.pane_output_capture_hit_count),
        pane_output_capture_error_count: status
            .runtime_telemetry
            .as_ref()
            .map(|telemetry| telemetry.pane_output_capture_error_count),
        latest_snapshot_observability: status.latest_snapshot_observability.clone(),
        recent_events: include_events.then(|| status.recent_events.clone()),
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
    include_events: bool,
) -> Result<()> {
    match format {
        OutputFormat::Text => {
            print_lifecycle_not_running(socket_path, paths, reason);
            Ok(())
        }
        OutputFormat::Json => output::print_json(&lifecycle_not_running_json(
            socket_path,
            paths,
            reason,
            include_events,
        )),
    }
}

fn emit_lifecycle_status(
    paths: &LifecyclePaths,
    status: &ipc::LifecycleStatusFrame,
    format: OutputFormat,
    include_events: bool,
) -> Result<()> {
    match format {
        OutputFormat::Text => {
            print_lifecycle_status(paths, status);
            if include_events {
                print_recent_observability_events(&status.recent_events);
            }
            Ok(())
        }
        OutputFormat::Json => {
            output::print_json(&lifecycle_status_json(paths, status, include_events))
        }
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
    println!("event_log_path: {}", paths.event_log_path.display());
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
        if let Some(fallback_count) = broker.fallback_count {
            println!("control_mode_broker_fallback_count: {fallback_count}");
        } else {
            println!("control_mode_broker_fallback_count: unavailable");
        }
        if let Some(subscriber_count) = broker.subscriber_count {
            println!("control_mode_broker_subscriber_count: {subscriber_count}");
        } else {
            println!("control_mode_broker_subscriber_count: unavailable");
        }
        if let Some(reason) = &broker.disabled_reason {
            println!("control_mode_broker_disabled_reason: {reason}");
        }
    }
    if let Some(telemetry) = &status.runtime_telemetry {
        println!(
            "control_event_refresh_count: {}",
            telemetry.control_event_refresh_count
        );
        println!(
            "control_event_batch_count: {}",
            telemetry.control_event_batch_count
        );
        println!(
            "control_event_line_count: {}",
            telemetry.control_event_line_count
        );
        println!(
            "control_event_output_line_count: {}",
            telemetry.control_event_output_line_count
        );
        println!(
            "control_event_output_byte_count: {}",
            telemetry.control_event_output_byte_count
        );
        println!(
            "control_event_pane_count: {}",
            telemetry.control_event_pane_count
        );
        println!(
            "control_event_title_count: {}",
            telemetry.control_event_title_count
        );
        println!(
            "control_event_window_count: {}",
            telemetry.control_event_window_count
        );
        println!(
            "control_event_session_count: {}",
            telemetry.control_event_session_count
        );
        println!(
            "control_event_resnapshot_count: {}",
            telemetry.control_event_resnapshot_count
        );
        println!(
            "control_event_ignored_count: {}",
            telemetry.control_event_ignored_count
        );
        println!(
            "reconcile_attempt_count: {}",
            telemetry.reconcile_attempt_count
        );
        println!("reconcile_noop_count: {}", telemetry.reconcile_noop_count);
        println!(
            "reconcile_changed_snapshot_count: {}",
            telemetry.reconcile_changed_snapshot_count
        );
        println!(
            "targeted_title_update_count: {}",
            telemetry.targeted_title_update_count
        );
        println!(
            "targeted_pane_refresh_count: {}",
            telemetry.targeted_pane_refresh_count
        );
        println!(
            "targeted_scope_refresh_count: {}",
            telemetry.targeted_scope_refresh_count
        );
        println!(
            "full_snapshot_refresh_count: {}",
            telemetry.full_snapshot_refresh_count
        );
        println!(
            "targeted_refresh_fallback_to_full_count: {}",
            telemetry.targeted_refresh_fallback_to_full_count
        );
        println!("broker_fallback_count: {}", telemetry.broker_fallback_count);
        println!(
            "pane_output_capture_attempt_count: {}",
            telemetry.pane_output_capture_attempt_count
        );
        println!(
            "pane_output_capture_hit_count: {}",
            telemetry.pane_output_capture_hit_count
        );
        println!(
            "pane_output_capture_error_count: {}",
            telemetry.pane_output_capture_error_count
        );
    } else {
        println!("runtime_telemetry: unavailable");
    }
    if let Some(observability) = &status.latest_snapshot_observability {
        println!(
            "latest_snapshot_provider_known_count: {}",
            observability.provider_known_count
        );
        println!(
            "latest_snapshot_provider_unknown_count: {}",
            observability.provider_unknown_count
        );
        println!(
            "latest_snapshot_status_source_pane_metadata_count: {}",
            observability.status_source_pane_metadata_count
        );
        println!(
            "latest_snapshot_status_source_tmux_title_count: {}",
            observability.status_source_tmux_title_count
        );
        println!(
            "latest_snapshot_status_source_pane_output_count: {}",
            observability.status_source_pane_output_count
        );
        println!(
            "latest_snapshot_status_source_not_checked_count: {}",
            observability.status_source_not_checked_count
        );
        println!(
            "latest_snapshot_proc_fallback_not_run_count: {}",
            observability.proc_fallback_not_run_count
        );
        println!(
            "latest_snapshot_proc_fallback_skipped_count: {}",
            observability.proc_fallback_skipped_count
        );
        println!(
            "latest_snapshot_proc_fallback_no_match_count: {}",
            observability.proc_fallback_no_match_count
        );
        println!(
            "latest_snapshot_proc_fallback_error_count: {}",
            observability.proc_fallback_error_count
        );
        println!(
            "latest_snapshot_proc_fallback_resolved_count: {}",
            observability.proc_fallback_resolved_count
        );
        for (provider, stats) in &observability.per_provider {
            println!(
                "latest_snapshot_provider[{provider}]: panes={} matched(metadata={},command={},title={},proc={}) status(metadata={},title={},output={},not_checked={})",
                stats.pane_count,
                stats.matched_pane_metadata_count,
                stats.matched_pane_current_command_count,
                stats.matched_pane_title_count,
                stats.matched_proc_process_tree_count,
                stats.status_source_pane_metadata_count,
                stats.status_source_tmux_title_count,
                stats.status_source_pane_output_count,
                stats.status_source_not_checked_count,
            );
        }
    }
    if let Some(reason) = status.unavailable_reason {
        println!("unavailable_reason: {}", unavailable_reason_label(reason));
    }
    if let Some(message) = &status.message {
        println!("message: {message}");
    }
}

fn print_recent_observability_events(events: &[ipc::DaemonObservabilityEventFrame]) {
    println!("recent_events:");
    if events.is_empty() {
        println!("  <empty>");
        return;
    }
    for event in events {
        println!(
            "  {} source={} detail={} refresh={} changed={} published={} duration_ms={}",
            event.at,
            event.source,
            event.detail.as_deref().unwrap_or("<none>"),
            event.refresh,
            event.changed,
            event.published,
            event
                .duration_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "<none>".to_string())
        );
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

pub(crate) fn daemon_status(format: OutputFormat, include_events: bool) -> Result<()> {
    let socket_path = ipc::resolve_socket_path()?;
    daemon_status_with_socket_path(&socket_path, format, include_events)
}

pub(crate) fn daemon_status_with_socket_path(
    socket_path: &Path,
    format: OutputFormat,
    include_events: bool,
) -> Result<()> {
    let paths = LifecyclePaths::from_socket_path(socket_path);
    match lifecycle_status_from_socket(socket_path, LIFECYCLE_CONNECT_TIMEOUT)? {
        LifecycleQuery::NotRunning(reason) => {
            emit_lifecycle_not_running(socket_path, &paths, &reason, format, include_events)
        }
        LifecycleQuery::Status(status) => {
            emit_lifecycle_status(&paths, &status, format, include_events)
        }
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
    DaemonStartCoordinator::from_current_process()?.start(socket_path, output, intent)
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
    if !intent.requires_macos_trust_preflight() {
        return DaemonStartPolicyDecision::Allowed;
    }
    let assessment = assess_macos_executable_for_daemon_autostart(executable_path);
    daemon_start_policy_decision_from_macos_assessment(intent, executable_path, assessment)
}

#[cfg(test)]
pub(crate) fn test_macos_start_requires_trust_preflight(
    explicit_start: bool,
    tui_start: bool,
) -> bool {
    let intent = match (explicit_start, tui_start) {
        (true, _) => DaemonStartIntent::ExplicitLifecycleCommand,
        (false, true) => DaemonStartIntent::TuiSubscriptionAutoStart,
        (false, false) => DaemonStartIntent::ImplicitConsumerAutoStart,
    };
    intent.requires_macos_trust_preflight()
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

fn start_daemon_from_request(
    request: DaemonStartRequest<'_>,
) -> std::result::Result<(), DaemonSnapshotError> {
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
