use super::*;

enum SubscriptionConnect {
    Subscribed {
        reader: BufReader<std::os::unix::net::UnixStream>,
        bootstrap: SnapshotEnvelope,
        bootstrap_seq: u64,
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

    #[test]
    fn daemon_hello_write_stale_socket_errors_are_retryable_not_running() {
        for kind in [
            std::io::ErrorKind::BrokenPipe,
            std::io::ErrorKind::ConnectionReset,
            std::io::ErrorKind::NotConnected,
        ] {
            let error = std::io::Error::from(kind);
            let reason = daemon_hello_write_not_running_reason(&error, "subscription")
                .expect("stale socket write should be retryable");

            assert!(
                reason.contains("socket closed before accepting daemon subscription hello"),
                "{reason}"
            );
        }
    }

    #[test]
    fn daemon_hello_write_other_errors_stay_fatal() {
        let error = std::io::Error::from(std::io::ErrorKind::PermissionDenied);

        assert!(daemon_hello_write_not_running_reason(&error, "subscription").is_none());
    }
}
pub(crate) fn spawn_subscription_worker(
    policy: AutoStartPolicy,
    events: mpsc::Sender<LiveClientEvent>,
    cancel: Arc<AtomicBool>,
    row_mode: SubscriptionRowMode,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let result = subscription_worker_loop(policy, &events, &cancel, row_mode);
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
    let worker = spawn_subscription_worker(
        policy,
        events_tx,
        Arc::clone(&cancel),
        SubscriptionRowMode::Build,
    );
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

/// How long the subscribe writer waits for a real event before emitting a heartbeat frame to
/// probe whether the consumer is still attached.
const SUBSCRIPTION_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(1);

fn write_subscription_events_json(
    events: mpsc::Receiver<LiveClientEvent>,
    cancel: &AtomicBool,
) -> std::result::Result<SubscriptionStreamCompletion, DaemonSnapshotError> {
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();

    loop {
        match events.recv_timeout(SUBSCRIPTION_KEEPALIVE_INTERVAL) {
            Ok(event) => {
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
            // No event within the interval: probe the consumer with a heartbeat so a closed
            // stdout (e.g. `agentscan subscribe | head`) is detected promptly even when the
            // daemon publishes nothing. The daemon suppresses materially-equal snapshots, so the
            // stream is otherwise free to stay silent indefinitely while the consumer is gone.
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if !write_subscription_keepalive(&mut stdout, cancel)? {
                    return Ok(SubscriptionStreamCompletion::StdoutClosed);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Ok(SubscriptionStreamCompletion::WorkerFinished);
            }
        }
    }
}

/// Heartbeat frame for a silent subscribe stream. It carries the same `type`-tagged shape as
/// `LiveClientEvent`, so consumers that switch on the frame type ignore it. Returns `false` when
/// stdout has closed (a broken pipe), which is how the writer learns the consumer is gone when no
/// real events are flowing.
fn write_subscription_keepalive(
    writer: &mut impl Write,
    cancel: &AtomicBool,
) -> std::result::Result<bool, DaemonSnapshotError> {
    if let Err(error) = writer
        .write_all(b"{\"type\":\"keepalive\"}\n")
        .and_then(|()| writer.flush())
    {
        if error.kind() == std::io::ErrorKind::BrokenPipe {
            cancel.store(true, Ordering::Relaxed);
            return Ok(false);
        }
        return Err(DaemonSnapshotError::UnexpectedFrame {
            message: format!("failed to write subscription keepalive: {error}"),
        });
    }
    Ok(true)
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

/// Whether the subscription worker should assemble picker rows for each
/// snapshot event. The JSON stream needs them (the desktop renders them); the
/// in-process TUI discards them, and building rows costs a `tmux list-clients`
/// spawn plus row assembly per frame, so the TUI opts out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SubscriptionRowMode {
    Build,
    Skip,
}

/// Builds the picker `rows` that ride alongside each `snapshot` frame on the
/// subscribe stream. Owning this here (in the process that owns tmux) keeps the
/// desktop from spawning a second `agentscan hotkeys` scan per update: the same
/// `picker::picker_rows` assembly the `hotkeys` command uses runs once, on the
/// host, so remote clients get correct focus/client resolution and host-local
/// workspace grouping they could not reproduce from the snapshot alone.
enum SubscriptionRowContext {
    Build {
        picker_group_by: picker::PickerGroupBy,
        picker_keys: picker::PickerKeySet,
    },
    Skip,
}

impl SubscriptionRowContext {
    fn resolve(mode: SubscriptionRowMode) -> Result<Self> {
        match mode {
            SubscriptionRowMode::Build => {
                let config = config::resolve_picker_config()?;
                Ok(Self::Build {
                    picker_group_by: config.picker_group_by,
                    picker_keys: config.picker_keys,
                })
            }
            SubscriptionRowMode::Skip => Ok(Self::Skip),
        }
    }

    /// Wrap a snapshot into a `Snapshot` event, deriving the picker rows from that
    /// same snapshot. Hotkey assignment is stable across frames because
    /// `picker_rows` orders panes deterministically (by workspace, then tmux
    /// location, then pane id) before zipping keys — arrival order never affects it.
    fn snapshot_event(&self, snapshot: SnapshotEnvelope) -> LiveClientEvent {
        let rows = self.build_rows(&snapshot);
        LiveClientEvent::Snapshot {
            snapshot: Box::new(snapshot),
            rows,
        }
    }

    fn build_rows(&self, snapshot: &SnapshotEnvelope) -> Vec<picker::PickerRow> {
        let Self::Build {
            picker_group_by,
            picker_keys,
        } = self
        else {
            return Vec::new();
        };
        // Match `hotkeys`' default (agent panes only) and its live focus/client
        // resolution; any tmux error degrades to "no focus" rather than failing.
        let agent_panes: Vec<PaneRecord> = snapshot
            .panes
            .iter()
            .filter(|pane| pane.provider.is_some())
            .cloned()
            .collect();
        let focus = tmux::tmux_focus_state().unwrap_or_default();
        picker::picker_rows(
            &agent_panes,
            focus.focused_session.as_deref(),
            u32::try_from(focus.attached_client_count).unwrap_or(u32::MAX),
            *picker_group_by,
            picker_keys,
        )
    }
}

fn subscription_worker_loop(
    policy: AutoStartPolicy,
    events: &mpsc::Sender<LiveClientEvent>,
    cancel: &Arc<AtomicBool>,
    row_mode: SubscriptionRowMode,
) -> Result<()> {
    let socket_path = ipc::resolve_socket_path()?;
    let paths = LifecyclePaths::from_socket_path(&socket_path);
    let mut state = SubscriptionState::new();
    let row_context = SubscriptionRowContext::resolve(row_mode)?;

    while !cancel.load(Ordering::Relaxed) {
        send_subscription_event(events, state.connecting_event(&socket_path))?;

        match subscribe_once_from_socket(&socket_path)? {
            SubscriptionConnect::Subscribed {
                mut reader,
                bootstrap,
                bootstrap_seq,
            } => {
                state.mark_subscribed();
                // Seed the reconstruction state from the bootstrap full snapshot;
                // subsequent diff frames are applied on top of it and must advance
                // `seq` by exactly one.
                let mut snapshot_state = SubscriptionSnapshotState {
                    snapshot: bootstrap.clone(),
                    seq: bootstrap_seq,
                };
                send_subscription_event(events, row_context.snapshot_event(bootstrap))?;
                match read_subscription_frames(
                    &mut reader,
                    events,
                    cancel,
                    &row_context,
                    &mut snapshot_state,
                ) {
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
    let mut reader = match open_daemon_client(
        socket_path,
        ipc::ClientMode::Subscribe,
        "subscription",
        false,
    )? {
        DaemonClientOpen::NotRunning(reason) => {
            return Ok(SubscriptionConnect::NotRunning(reason));
        }
        DaemonClientOpen::Connected(connection) => connection.reader,
    };
    let Some(first_frame) = (match read_subscription_bootstrap_frame(&mut reader) {
        BootstrapFrameRead::Frame(frame) => frame,
        BootstrapFrameRead::Connect(connect) => return Ok(connect),
    }) else {
        return Ok(SubscriptionConnect::Unexpected(
            "daemon closed without subscription response".to_string(),
        ));
    };
    match classify_daemon_hello_frame(first_frame, "subscription") {
        DaemonHello::Busy(message) => Ok(SubscriptionConnect::Retryable(message)),
        DaemonHello::Rejected { message, .. } | DaemonHello::Incompatible { message, .. } => {
            Ok(SubscriptionConnect::Incompatible(message))
        }
        DaemonHello::Acked => {
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
                ipc::DaemonFrame::Snapshot { seq, snapshot } => {
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
                        bootstrap_seq: seq,
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
        DaemonHello::Unexpected(other) => Ok(SubscriptionConnect::Unexpected(format!(
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

#[cfg_attr(test, derive(Debug))]
enum SubscriptionReadResult {
    Reconnect(String),
    Shutdown(String),
    Cancelled,
}

/// The last full snapshot a subscriber reconstructed, plus the seq it was
/// observed at. A full `snapshot` frame (bootstrap or coalesce fallback) replaces
/// both fields wholesale; a `snapshot_diff` frame is applied on top and must carry
/// exactly `seq + 1`.
struct SubscriptionSnapshotState {
    snapshot: SnapshotEnvelope,
    seq: u64,
}

/// Reconstructs the current snapshot from a `snapshot_diff`: adopts the
/// envelope-level fields, upserts every changed pane, drops removed panes, and
/// restores the daemon's canonical pane sort order so rows/consumers see exactly
/// what a fresh full snapshot would carry.
fn apply_snapshot_diff(
    base: &SnapshotEnvelope,
    schema_version: u32,
    generated_at: String,
    source: SnapshotSource,
    changed_panes: Vec<PaneRecord>,
    removed_pane_ids: Vec<String>,
) -> SnapshotEnvelope {
    let mut snapshot = base.clone();
    snapshot.schema_version = schema_version;
    snapshot.generated_at = generated_at;
    snapshot.source = source;

    if !removed_pane_ids.is_empty() {
        let removed = removed_pane_ids
            .iter()
            .map(String::as_str)
            .collect::<std::collections::HashSet<_>>();
        snapshot
            .panes
            .retain(|pane| !removed.contains(pane.pane_id.as_str()));
    }

    for pane in changed_panes {
        if let Some(existing) = snapshot
            .panes
            .iter_mut()
            .find(|existing| existing.pane_id == pane.pane_id)
        {
            *existing = pane;
        } else {
            snapshot.panes.push(pane);
        }
    }

    snapshot::sort_snapshot_panes(&mut snapshot);
    snapshot
}

fn read_subscription_frames(
    reader: &mut BufReader<std::os::unix::net::UnixStream>,
    events: &mpsc::Sender<LiveClientEvent>,
    cancel: &Arc<AtomicBool>,
    row_context: &SubscriptionRowContext,
    snapshot_state: &mut SubscriptionSnapshotState,
) -> SubscriptionReadResult {
    loop {
        if cancel.load(Ordering::Relaxed) {
            return SubscriptionReadResult::Cancelled;
        }

        match ipc::read_daemon_frame(reader) {
            Ok(Some(ipc::DaemonFrame::Snapshot { seq, snapshot })) => {
                // An absolute full frame (coalesce fallback): validate, adopt it
                // wholesale, and reset the expected seq. Full frames re-sync a
                // subscriber regardless of any diffs it missed.
                if let Err(error) = snapshot::validate_snapshot(&snapshot) {
                    return SubscriptionReadResult::Reconnect(format!(
                        "invalid daemon snapshot: {error:#}"
                    ));
                }
                snapshot_state.snapshot = snapshot.clone();
                snapshot_state.seq = seq;
                if send_subscription_event(events, row_context.snapshot_event(snapshot)).is_err() {
                    return SubscriptionReadResult::Cancelled;
                }
            }
            Ok(Some(ipc::DaemonFrame::SnapshotDiff {
                seq,
                schema_version,
                generated_at,
                source,
                changed_panes,
                removed_pane_ids,
            })) => {
                // Diffs are strictly ordered: any gap (or an out-of-order seq)
                // means a frame was lost, so we cannot safely reconstruct. Force a
                // reconnect, which re-bootstraps a full snapshot — never guess.
                let expected = snapshot_state.seq.saturating_add(1);
                if seq != expected {
                    return SubscriptionReadResult::Reconnect(format!(
                        "daemon snapshot diff seq gap: expected {expected}, got {seq}"
                    ));
                }
                let reconstructed = apply_snapshot_diff(
                    &snapshot_state.snapshot,
                    schema_version,
                    generated_at,
                    source,
                    changed_panes,
                    removed_pane_ids,
                );
                if let Err(error) = snapshot::validate_snapshot(&reconstructed) {
                    return SubscriptionReadResult::Reconnect(format!(
                        "invalid reconstructed daemon snapshot: {error:#}"
                    ));
                }
                snapshot_state.snapshot = reconstructed.clone();
                snapshot_state.seq = seq;
                if send_subscription_event(events, row_context.snapshot_event(reconstructed))
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
#[cfg(test)]
pub(crate) fn test_write_subscription_keepalive(
    writer: &mut impl Write,
    cancel: &AtomicBool,
) -> std::result::Result<bool, DaemonSnapshotError> {
    write_subscription_keepalive(writer, cancel)
}

#[cfg(test)]
mod diff_apply_tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    fn test_source() -> SnapshotSource {
        SnapshotSource {
            kind: SourceKind::Daemon,
            tmux_version: Some("3.4".to_string()),
            daemon_generated_at: Some("2026-07-13T00:00:00Z".to_string()),
        }
    }

    fn test_pane(session: &str, window_index: u32, pane_index: u32, pane_id: &str) -> PaneRecord {
        PaneRecord {
            pane_id: pane_id.to_string(),
            location: PaneLocation {
                session_name: session.to_string(),
                window_index,
                pane_index,
                window_name: "win".to_string(),
            },
            tmux: TmuxPaneMetadata {
                pane_pid: 100,
                pane_tty: "/dev/ttys000".to_string(),
                pane_current_path: "/tmp".to_string(),
                pane_current_command: "node".to_string(),
                pane_title_raw: "title".to_string(),
                session_id: Some(format!("${session}")),
                window_id: Some(format!("@{window_index}")),
                pane_active: false,
                window_active: false,
            },
            display: DisplayMetadata {
                label: pane_id.to_string(),
                activity_label: None,
            },
            provider: None,
            status: PaneStatus::not_checked(),
            classification: PaneClassification {
                matched_by: None,
                confidence: None,
                reasons: Vec::new(),
            },
            agent_metadata: AgentMetadata::default(),
            diagnostics: PaneDiagnostics {
                cache_origin: "daemon_snapshot".to_string(),
                proc_fallback: ProcFallbackDiagnostics::default(),
            },
        }
    }

    fn snapshot_with(panes: Vec<PaneRecord>, generated_at: &str) -> SnapshotEnvelope {
        let mut snapshot = SnapshotEnvelope {
            schema_version: CACHE_SCHEMA_VERSION,
            generated_at: generated_at.to_string(),
            source: test_source(),
            panes,
        };
        snapshot::sort_snapshot_panes(&mut snapshot);
        snapshot
    }

    #[test]
    fn apply_diff_upserts_and_removes_preserving_sort_order() {
        // Base: two panes in canonical sort order (session, window, pane, id).
        let base = snapshot_with(
            vec![
                test_pane("alpha", 0, 0, "%1"),
                test_pane("alpha", 2, 0, "%3"),
            ],
            "2026-07-13T00:00:00Z",
        );

        // Change %3's status, add %2 (which must sort between %1 and %3), remove %1.
        let mut changed_pane3 = test_pane("alpha", 2, 0, "%3");
        changed_pane3.status = PaneStatus::metadata(StatusKind::Busy);
        let added_pane2 = test_pane("alpha", 1, 0, "%2");

        let result = apply_snapshot_diff(
            &base,
            CACHE_SCHEMA_VERSION,
            "2026-07-13T00:00:05Z".to_string(),
            test_source(),
            vec![changed_pane3.clone(), added_pane2.clone()],
            vec!["%1".to_string()],
        );

        // Envelope-level field adopted from the diff.
        assert_eq!(result.generated_at, "2026-07-13T00:00:05Z");
        // %1 removed; %2 inserted in sorted position ahead of %3; %3 updated.
        let ids: Vec<&str> = result.panes.iter().map(|p| p.pane_id.as_str()).collect();
        assert_eq!(ids, vec!["%2", "%3"]);
        assert_eq!(
            result.panes[1].status,
            PaneStatus::metadata(StatusKind::Busy)
        );

        // Reconstruction equals a fresh snapshot built from the same final panes.
        let expected = snapshot_with(vec![added_pane2, changed_pane3], "2026-07-13T00:00:05Z");
        assert_eq!(result, expected);
    }

    fn write_frame(stream: &mut UnixStream, frame: &ipc::DaemonFrame) {
        let bytes = ipc::encode_frame(frame).expect("frame should encode");
        stream.write_all(&bytes).expect("frame should write");
    }

    fn drive_read_frames(
        bootstrap: SnapshotEnvelope,
        bootstrap_seq: u64,
        frames: Vec<ipc::DaemonFrame>,
    ) -> (SubscriptionReadResult, Vec<LiveClientEvent>) {
        let (mut server, client) = UnixStream::pair().expect("socket pair");
        for frame in &frames {
            write_frame(&mut server, frame);
        }
        server
            .shutdown(std::net::Shutdown::Write)
            .expect("server write side should close");

        let mut reader = BufReader::new(client);
        let (events_tx, events_rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let row_context = SubscriptionRowContext::resolve(SubscriptionRowMode::Build)
            .expect("row context should resolve");
        let mut state = SubscriptionSnapshotState {
            snapshot: bootstrap,
            seq: bootstrap_seq,
        };
        let result =
            read_subscription_frames(&mut reader, &events_tx, &cancel, &row_context, &mut state);
        drop(events_tx);
        (result, events_rx.into_iter().collect())
    }

    #[test]
    fn contiguous_diff_is_applied_and_emitted() {
        let base = snapshot_with(vec![test_pane("alpha", 0, 0, "%1")], "2026-07-13T00:00:00Z");
        let mut changed = test_pane("alpha", 0, 0, "%1");
        changed.status = PaneStatus::metadata(StatusKind::Busy);
        let diff = ipc::DaemonFrame::SnapshotDiff {
            seq: 2,
            schema_version: CACHE_SCHEMA_VERSION,
            generated_at: "2026-07-13T00:00:01Z".to_string(),
            source: test_source(),
            changed_panes: vec![changed],
            removed_pane_ids: Vec::new(),
        };

        let (result, events) = drive_read_frames(base, 1, vec![diff]);
        // End of stream after the applied diff surfaces as a reconnect.
        assert!(matches!(result, SubscriptionReadResult::Reconnect(_)));
        let snapshot_events: Vec<_> = events
            .iter()
            .filter(|event| matches!(event, LiveClientEvent::Snapshot { .. }))
            .collect();
        assert_eq!(snapshot_events.len(), 1, "diff should emit one snapshot");
        if let LiveClientEvent::Snapshot { snapshot, .. } = snapshot_events[0] {
            assert_eq!(snapshot.generated_at, "2026-07-13T00:00:01Z");
            assert_eq!(
                snapshot.panes[0].status,
                PaneStatus::metadata(StatusKind::Busy)
            );
        }
    }

    #[test]
    fn seq_gap_forces_reconnect_without_emitting() {
        let base = snapshot_with(vec![test_pane("alpha", 0, 0, "%1")], "2026-07-13T00:00:00Z");
        // Bootstrap seq is 1, but the diff claims seq 3: a lost frame.
        let gap_diff = ipc::DaemonFrame::SnapshotDiff {
            seq: 3,
            schema_version: CACHE_SCHEMA_VERSION,
            generated_at: "2026-07-13T00:00:02Z".to_string(),
            source: test_source(),
            changed_panes: Vec::new(),
            removed_pane_ids: Vec::new(),
        };

        let (result, events) = drive_read_frames(base, 1, vec![gap_diff]);
        match result {
            SubscriptionReadResult::Reconnect(message) => {
                assert!(message.contains("seq gap"), "{message}");
                assert!(message.contains("expected 2"), "{message}");
                assert!(message.contains("got 3"), "{message}");
            }
            other => panic!("expected reconnect on seq gap, got {other:?}"),
        }
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, LiveClientEvent::Snapshot { .. })),
            "a gapped diff must not emit a reconstructed snapshot"
        );
    }

    #[test]
    fn mid_stream_full_snapshot_resets_seq() {
        let base = snapshot_with(vec![test_pane("alpha", 0, 0, "%1")], "2026-07-13T00:00:00Z");
        // A coalesce-fallback full frame at seq 5 (a jump), then a contiguous diff
        // at seq 6 that must apply because the full frame reset the expected seq.
        let full = snapshot_with(
            vec![
                test_pane("alpha", 0, 0, "%1"),
                test_pane("beta", 0, 0, "%9"),
            ],
            "2026-07-13T00:00:05Z",
        );
        let full_frame = ipc::DaemonFrame::Snapshot {
            seq: 5,
            snapshot: full,
        };
        let follow_diff = ipc::DaemonFrame::SnapshotDiff {
            seq: 6,
            schema_version: CACHE_SCHEMA_VERSION,
            generated_at: "2026-07-13T00:00:06Z".to_string(),
            source: test_source(),
            changed_panes: Vec::new(),
            removed_pane_ids: vec!["%9".to_string()],
        };

        let (result, events) = drive_read_frames(base, 1, vec![full_frame, follow_diff]);
        assert!(matches!(result, SubscriptionReadResult::Reconnect(_)));
        let snapshots: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                LiveClientEvent::Snapshot { snapshot, .. } => Some(snapshot.as_ref()),
                _ => None,
            })
            .collect();
        assert_eq!(snapshots.len(), 2, "full frame + contiguous diff each emit");
        assert_eq!(snapshots[0].panes.len(), 2);
        // The follow-up diff removed %9, so the final snapshot has just %1.
        let final_ids: Vec<&str> = snapshots[1]
            .panes
            .iter()
            .map(|p| p.pane_id.as_str())
            .collect();
        assert_eq!(final_ids, vec!["%1"]);
    }
}
