use super::*;

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize)]
struct DaemonSignalIdentity {
    pid: u32,
    #[serde(default)]
    daemon_start_time: Option<String>,
    executable: String,
    #[serde(default)]
    executable_canonical: Option<String>,
    socket_path: String,
}

impl DaemonSignalIdentity {
    fn from_frame(identity: &ipc::DaemonIdentityFrame) -> Self {
        Self {
            pid: identity.pid,
            daemon_start_time: Some(identity.daemon_start_time.clone()),
            executable: identity.executable.clone(),
            executable_canonical: identity.executable_canonical.clone(),
            socket_path: identity.socket_path.clone(),
        }
    }
}
pub(super) struct DaemonStartGuard {
    lock_file: File,
}

impl DaemonStartGuard {
    pub(crate) fn acquire(paths: &LifecyclePaths) -> Result<Self> {
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

pub(crate) struct DaemonLifecycleGuard {
    lock_file: File,
    identity_path: PathBuf,
    identity: ipc::DaemonIdentityFrame,
}

impl DaemonLifecycleGuard {
    pub(crate) fn acquire(
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
pub(crate) fn remove_stale_socket_if_present(socket_path: &Path) -> Result<()> {
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
        LifecycleQuery::Incompatible { message, .. } => {
            bail!("{message}; not signaling an incompatible daemon")
        }
        LifecycleQuery::Busy(message) => bail!("{message}; not signaling daemon while busy"),
    }
}

fn read_identity_sidecar(identity_path: &Path) -> Result<DaemonSignalIdentity> {
    let bytes = fs::read(identity_path)
        .with_context(|| format!("failed to read identity {}", identity_path.display()))?;
    serde_json::from_slice::<DaemonSignalIdentity>(&bytes)
        .with_context(|| format!("failed to parse identity {}", identity_path.display()))
}

fn validate_sidecar_identity_for_signal(
    socket_path: &Path,
    paths: &LifecyclePaths,
    identity: &DaemonSignalIdentity,
    peer_pid: Option<u32>,
) -> Result<()> {
    if Path::new(&identity.socket_path) != socket_path {
        bail!(
            "daemon identity sidecar socket path {} does not match {}; not signaling incompatible daemon",
            identity.socket_path,
            socket_path.display()
        );
    }
    validate_sidecar_peer_pid(identity, peer_pid)?;
    validate_live_identity_for_signal(identity)?;
    validate_lifecycle_lock_held(paths)?;
    validate_live_executable_matches_sidecar(identity)?;
    Ok(())
}

fn validate_sidecar_peer_pid(identity: &DaemonSignalIdentity, peer_pid: Option<u32>) -> Result<()> {
    let Some(peer_pid) = peer_pid else {
        bail!("daemon socket peer pid is unavailable; not signaling incompatible daemon");
    };
    if peer_pid != identity.pid {
        bail!(
            "daemon identity sidecar pid {} does not match socket peer pid {}; not signaling incompatible daemon",
            identity.pid,
            peer_pid
        );
    }
    Ok(())
}

fn validate_lifecycle_lock_held(paths: &LifecyclePaths) -> Result<()> {
    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&paths.lock_path)
        .with_context(|| format!("failed to open daemon lock {}", paths.lock_path.display()))?;
    let result = unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        unsafe {
            libc::flock(lock_file.as_raw_fd(), libc::LOCK_UN);
        }
        bail!(
            "daemon identity sidecar is stale because lock {} is not held; not signaling incompatible daemon",
            paths.lock_path.display()
        );
    }

    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) || error.raw_os_error() == Some(libc::EAGAIN)
    {
        Ok(())
    } else {
        Err(error).with_context(|| format!("failed to inspect {}", paths.lock_path.display()))
    }
}

fn validate_live_executable_matches_sidecar(identity: &DaemonSignalIdentity) -> Result<()> {
    let Some(live_executable) = live_process_executable(identity.pid) else {
        return Ok(());
    };
    let expected = identity
        .executable_canonical
        .as_deref()
        .filter(|path| !path.trim().is_empty())
        .unwrap_or(&identity.executable);
    if expected.trim().is_empty() {
        return Ok(());
    }

    let expected_path = normalize_executable_path(Path::new(expected));
    let live_path = normalize_executable_path(&live_executable);
    if expected_path == live_path {
        return Ok(());
    }

    bail!(
        "daemon identity pid {} now points at executable {}; expected {}; not signaling incompatible daemon",
        identity.pid,
        live_path.display(),
        expected_path.display()
    );
}

fn validate_initially_matched_identity_for_forced_signal(
    identity: &DaemonSignalIdentity,
) -> Result<()> {
    validate_live_identity_for_signal(identity)?;
    validate_live_executable_matches_sidecar(identity)
}

fn normalize_executable_path(path: &Path) -> PathBuf {
    let path = strip_deleted_executable_suffix(path);
    fs::canonicalize(&path).unwrap_or(path)
}

#[cfg(target_os = "linux")]
fn strip_deleted_executable_suffix(path: &Path) -> PathBuf {
    let path_text = path.as_os_str().to_string_lossy();
    path_text
        .strip_suffix(" (deleted)")
        .map_or_else(|| path.to_path_buf(), PathBuf::from)
}

#[cfg(not(target_os = "linux"))]
fn strip_deleted_executable_suffix(path: &Path) -> PathBuf {
    path.to_path_buf()
}

#[cfg(target_os = "linux")]
fn live_process_executable(pid: u32) -> Option<PathBuf> {
    fs::read_link(format!("/proc/{pid}/exe")).ok()
}

#[cfg(target_os = "macos")]
fn live_process_executable(pid: u32) -> Option<PathBuf> {
    let mut buffer = vec![0_u8; 4096];
    let length = unsafe {
        libc::proc_pidpath(
            pid as libc::c_int,
            buffer.as_mut_ptr().cast(),
            u32::try_from(buffer.len()).ok()?,
        )
    };
    if length <= 0 {
        return None;
    }
    let length = usize::try_from(length).ok()?;
    buffer.truncate(length);
    let path = std::str::from_utf8(&buffer).ok()?.trim_end_matches('\0');
    (!path.trim().is_empty()).then(|| PathBuf::from(path))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn live_process_executable(_pid: u32) -> Option<PathBuf> {
    None
}

fn validate_live_identity_for_signal(identity: &DaemonSignalIdentity) -> Result<()> {
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

fn remove_matching_identity(identity_path: &Path, identity: &DaemonSignalIdentity) -> Result<()> {
    let bytes = match fs::read(identity_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read identity {}", identity_path.display()));
        }
    };
    let current = serde_json::from_slice::<DaemonSignalIdentity>(&bytes)
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
pub(super) fn stop_compatible_daemon(
    socket_path: &Path,
    paths: &LifecyclePaths,
    identity: &ipc::DaemonIdentityFrame,
) -> Result<()> {
    let signal_identity = DaemonSignalIdentity::from_frame(identity);
    validate_live_identity_for_signal(&signal_identity)?;
    signal_process(signal_identity.pid, libc::SIGTERM)?;
    if !wait_for_process_exit(signal_identity.pid, DAEMON_STOP_TIMEOUT)? {
        let live_status = matching_live_status(socket_path, identity)?;
        let live_signal_identity = DaemonSignalIdentity::from_frame(&live_status.identity);
        validate_live_identity_for_signal(&live_signal_identity)?;
        signal_process(live_signal_identity.pid, libc::SIGKILL)?;
        if !wait_for_process_exit(live_signal_identity.pid, DAEMON_STOP_TIMEOUT)? {
            bail!(
                "timed out waiting for daemon pid {} to exit after SIGKILL",
                live_signal_identity.pid
            );
        }
    }
    finish_daemon_stop(socket_path, paths, &signal_identity)
}

pub(super) fn stop_incompatible_daemon_from_identity(
    socket_path: &Path,
    paths: &LifecyclePaths,
    handshake_message: &str,
    peer_pid: Option<u32>,
) -> Result<()> {
    let identity = read_identity_sidecar(&paths.identity_path)
        .with_context(|| format!("{handshake_message}; identity sidecar is unavailable"))?;
    validate_sidecar_identity_for_signal(socket_path, paths, &identity, peer_pid)
        .with_context(|| format!("{handshake_message}; identity sidecar is not safe to signal"))?;

    signal_process(identity.pid, libc::SIGTERM)?;
    if !wait_for_process_exit(identity.pid, DAEMON_STOP_TIMEOUT)? {
        validate_initially_matched_identity_for_forced_signal(&identity)?;
        signal_process(identity.pid, libc::SIGKILL)?;
        if !wait_for_process_exit(identity.pid, DAEMON_STOP_TIMEOUT)? {
            bail!(
                "timed out waiting for daemon pid {} to exit after SIGKILL",
                identity.pid
            );
        }
    }
    finish_daemon_stop(socket_path, paths, &identity)
}

fn finish_daemon_stop(
    socket_path: &Path,
    paths: &LifecyclePaths,
    identity: &DaemonSignalIdentity,
) -> Result<()> {
    remove_stale_socket_if_present(socket_path)?;
    remove_matching_identity(&paths.identity_path, identity)?;
    output::write_stdout("agentscan daemon stopped\n")
}
