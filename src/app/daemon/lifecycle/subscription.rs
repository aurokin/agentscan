use super::*;

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
#[cfg(test)]
pub(crate) fn test_write_subscription_keepalive(
    writer: &mut impl Write,
    cancel: &AtomicBool,
) -> std::result::Result<bool, DaemonSnapshotError> {
    write_subscription_keepalive(writer, cancel)
}
