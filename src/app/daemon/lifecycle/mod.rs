use super::*;
// `daemon status` text output is buffered into a String and emitted through
// `output::write_stdout`, so a closed pipe surfaces as a recoverable `BrokenPipe`
// error instead of a `println!` panic. `writeln!` into a String needs this trait.
use std::fmt::Write as _;

mod client;
mod guard;
mod macos_trust;
mod status_format;
mod subscription;

use client::*;
use guard::*;
use status_format::*;

pub(crate) use client::{emit_pane_focus_event_best_effort, query_lifecycle_status};
pub(crate) use guard::{DaemonLifecycleGuard, remove_stale_socket_if_present};
#[cfg(any(test, target_os = "macos"))]
pub(crate) use macos_trust::MacExecutableAssessment;
#[cfg(target_os = "macos")]
pub(crate) use macos_trust::assess_macos_executable_for_daemon_autostart;
#[cfg(test)]
pub(crate) use macos_trust::test_macos_executable_assessment_for_outputs;
#[cfg(test)]
pub(crate) use subscription::test_write_subscription_keepalive;
pub(crate) use subscription::{spawn_subscription_worker, stream_subscription_events_json};

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

pub(crate) enum LifecycleQuery {
    NotRunning(String),
    Status(Box<ipc::LifecycleStatusFrame>),
    Incompatible {
        message: String,
        peer_pid: Option<u32>,
        can_signal: bool,
    },
    Busy(String),
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

impl StartOutput {
    fn print_ready(
        self,
        confirmation: StartConfirmation,
        paths: &LifecyclePaths,
        status: &ipc::LifecycleStatusFrame,
    ) {
        if self == Self::Verbose {
            let mut out = String::new();
            let _ = match confirmation {
                StartConfirmation::Started => writeln!(out, "agentscan daemon started"),
                StartConfirmation::AlreadyRunning => {
                    writeln!(out, "agentscan daemon already running")
                }
            };
            print_lifecycle_status(&mut out, paths, status);
            // Best-effort confirmation: the daemon is already started, so a closed
            // consumer pipe must neither error nor panic here.
            let _ = output::write_stdout(&out);
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
            LifecycleQuery::Incompatible { message, .. } => {
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
        LifecycleQuery::Incompatible { message, .. } => {
            Err(DaemonSnapshotError::Incompatible { message })
        }
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
    daemon_run_with_socket_path_and_startup(&socket_path, DaemonStartup::default())
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
        LifecycleQuery::Incompatible { message, .. } => {
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
            let mut out = String::new();
            print_lifecycle_not_running(&mut out, &socket_path, &paths, &reason);
            output::write_stdout(&out)
        }
        LifecycleQuery::Incompatible {
            message,
            peer_pid,
            can_signal,
        } => {
            if can_signal {
                stop_incompatible_daemon_from_identity(&socket_path, &paths, &message, peer_pid)
            } else {
                bail!("{message}; not signaling an incompatible daemon")
            }
        }
        LifecycleQuery::Busy(message) => bail!("{message}; not signaling daemon while busy"),
        LifecycleQuery::Status(status) => {
            stop_compatible_daemon(&socket_path, &paths, &status.identity)
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
