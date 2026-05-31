use super::*;

pub(super) struct TmuxDaemonReadProvider<'a> {
    fallback: TmuxCommandReadProvider,
    broker: Option<TmuxControlModeReadBroker<'a>>,
}

impl TmuxReadProvider for TmuxDaemonReadProvider<'_> {
    fn list_all_panes(&mut self) -> Result<Vec<TmuxPaneRow>> {
        let Some(broker) = self.broker.as_mut() else {
            return self.fallback.list_all_panes();
        };

        match broker.list_all_panes() {
            Ok(rows) => Ok(rows),
            Err(error) => {
                eprintln!(
                    "agentscan: brokered full reconcile failed; falling back to tmux command: {error:#}"
                );
                self.fallback.list_all_panes()
            }
        }
    }

    fn list_target_panes(&mut self, target: &str) -> Result<Option<Vec<TmuxPaneRow>>> {
        let Some(broker) = self.broker.as_mut() else {
            return self.fallback.list_target_panes(target);
        };

        match broker.list_target_panes(target) {
            Ok(rows) => Ok(rows),
            Err(error) => {
                eprintln!(
                    "agentscan: brokered target refresh failed for {target}; falling back to tmux command: {error:#}"
                );
                self.fallback.list_target_panes(target)
            }
        }
    }

    fn list_pane(&mut self, pane_id: &str) -> Result<Option<TmuxPaneRow>> {
        let Some(broker) = self.broker.as_mut() else {
            return self.fallback.list_pane(pane_id);
        };

        match broker.list_pane(pane_id) {
            Ok(pane) => Ok(pane),
            Err(error) => {
                eprintln!(
                    "agentscan: brokered pane refresh failed for {pane_id}; falling back to tmux command: {error:#}"
                );
                self.fallback.list_pane(pane_id)
            }
        }
    }
}

struct TmuxControlModeReadBroker<'a> {
    stdin: &'a mut std::process::ChildStdin,
    line_rx: &'a mpsc::Receiver<Result<ControlModeLine>>,
    deferred_lines: &'a mut VecDeque<ControlModeLine>,
    broker_health: &'a mut TmuxBrokerHealth,
}

impl TmuxControlModeReadBroker<'_> {
    fn list_all_panes(&mut self) -> Result<Vec<TmuxPaneRow>> {
        match control_mode_list_all_panes(self.stdin, self.line_rx, self.deferred_lines) {
            Ok(rows) => Ok(rows),
            Err(error) => {
                self.broker_health.disable_after_error(&error);
                Err(error)
            }
        }
    }

    fn list_target_panes(&mut self, target: &str) -> Result<Option<Vec<TmuxPaneRow>>> {
        match control_mode_list_panes_target(
            self.stdin,
            self.line_rx,
            target,
            self.deferred_lines,
            MissingTargetScope::PaneWindowSession,
        ) {
            Ok(rows) => Ok(rows),
            Err(error) => {
                self.broker_health.disable_after_error(&error);
                Err(error)
            }
        }
    }

    fn list_pane(&mut self, pane_id: &str) -> Result<Option<TmuxPaneRow>> {
        match control_mode_list_panes_target(
            self.stdin,
            self.line_rx,
            pane_id,
            self.deferred_lines,
            MissingTargetScope::PaneWindow,
        ) {
            Ok(rows) => Ok(rows.and_then(|mut rows| rows.pop())),
            Err(error) => {
                self.broker_health.disable_after_error(&error);
                Err(error)
            }
        }
    }
}

#[derive(Default)]
struct TmuxBrokerHealth {
    disabled_reason: Option<String>,
    reconnect_count: u32,
    fallback_count: u64,
}

impl TmuxBrokerHealth {
    fn enabled(&self) -> bool {
        self.disabled_reason.is_none()
    }

    fn disable_after_error(&mut self, error: &anyhow::Error) {
        if self.disabled_reason.is_none() {
            self.fallback_count = self.fallback_count.saturating_add(1);
            self.disabled_reason = Some(format!("{error:#}"));
        }
    }

    fn mark_reconnected(&mut self) {
        self.disabled_reason = None;
        self.reconnect_count = self.reconnect_count.saturating_add(1);
    }

    fn status_frame(&self) -> ipc::ControlModeBrokerStatusFrame {
        ipc::ControlModeBrokerStatusFrame {
            mode: if self.enabled() {
                ipc::ControlModeBrokerMode::Active
            } else {
                ipc::ControlModeBrokerMode::Fallback
            },
            disabled_reason: self.disabled_reason.clone(),
            reconnect_count: self.reconnect_count,
            fallback_count: Some(self.fallback_count),
            // Filled in by `RunningTmuxControlModeClient::broker_status_frame`,
            // which owns the subscriber set; broker health alone cannot see it.
            subscriber_count: None,
            primary_session_id: None,
            subscriber_coverage_complete: None,
            desired_subscriber_count: None,
            active_subscriber_count: None,
            missing_subscriber_session_ids: None,
            dead_subscriber_count: None,
            subscribers: None,
            last_subscriber_reconcile_at: None,
            next_subscriber_monitor_in_ms: None,
            next_reconcile_in_ms: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn test_disabled_reason(&self) -> Option<&str> {
        self.disabled_reason.as_deref()
    }
}

#[cfg(test)]
pub(crate) fn test_broker_health_after_error(message: &str) -> (bool, Option<String>, u64) {
    let mut health = TmuxBrokerHealth::default();
    let error = anyhow::anyhow!(message.to_string());
    health.disable_after_error(&error);
    (
        health.enabled(),
        health.test_disabled_reason().map(str::to_string),
        health.fallback_count,
    )
}

#[cfg(test)]
pub(crate) fn test_broker_health_after_repeated_error(message: &str) -> (Option<String>, u64) {
    let mut health = TmuxBrokerHealth::default();
    let error = anyhow::anyhow!(message.to_string());
    health.disable_after_error(&error);
    health.disable_after_error(&error);
    (
        health.test_disabled_reason().map(str::to_string),
        health.fallback_count,
    )
}

#[cfg(test)]
pub(crate) fn test_broker_health_after_reconnect(
    message: &str,
) -> ipc::ControlModeBrokerStatusFrame {
    let mut health = TmuxBrokerHealth::default();
    let error = anyhow::anyhow!(message.to_string());
    health.disable_after_error(&error);
    health.mark_reconnected();
    health.status_frame()
}

// Exercise `drain_control_mode_channel` over a plain channel: seed it with the
// `%begin`/`%end` of a brokered command that "timed out" (was never consumed),
// drain, and report how many frames survive. The sender is kept alive so the
// post-drain count reflects an emptied-but-open channel, mirroring the retained
// shared channel on reconnect. Returns the residual frame count (expected 0).
#[cfg(test)]
pub(crate) fn test_drain_control_mode_channel_clears_stale_frames() -> usize {
    let (line_tx, line_rx) = mpsc::channel::<Result<ControlModeLine>>();
    let mut deferred_lines = VecDeque::new();
    let source = ControlModeLineSource::Primary { session_id: None };
    line_tx
        .send(Ok(ControlModeLine::new(
            source.clone(),
            "%begin 1779870847 7 0".to_string(),
        )))
        .ok();
    line_tx
        .send(Ok(ControlModeLine::new(
            source.clone(),
            "%1 stale pane row".to_string(),
        )))
        .ok();
    line_tx
        .send(Ok(ControlModeLine::new(
            source,
            "%end 1779870847 7 0".to_string(),
        )))
        .ok();
    drain_control_mode_channel(&line_rx, &mut deferred_lines);
    // `line_tx` is still in scope, so the channel is open (not disconnected); a
    // correctly drained channel yields zero remaining frames.
    let residual = line_rx.try_iter().count();
    drop(line_tx);
    residual + deferred_lines.len()
}

#[cfg(test)]
pub(crate) fn test_drain_control_mode_channel_preserves_subscriber_frames() -> usize {
    let (line_tx, line_rx) = mpsc::channel::<Result<ControlModeLine>>();
    let mut deferred_lines = VecDeque::new();
    line_tx
        .send(Ok(ControlModeLine::new(
            ControlModeLineSource::Subscriber {
                session_id: Arc::<str>::from("$2"),
            },
            "%subscription-changed agentscan $2 @4 0 %7 : %7:codex:::::".to_string(),
        )))
        .ok();
    drain_control_mode_channel(&line_rx, &mut deferred_lines);
    let residual = line_rx.try_iter().count();
    drop(line_tx);
    residual + deferred_lines.len()
}

#[cfg(test)]
pub(crate) fn test_reconnect_preserves_deferred_lines() -> Vec<String> {
    let deferred_lines = VecDeque::from([
        "%subscription-changed agentscan $1 @1 0 %1 : %1:Codex:codex::::".to_string(),
    ]);
    let mut health = TmuxBrokerHealth::default();
    let error = anyhow::anyhow!("broken pipe");
    health.disable_after_error(&error);
    health.mark_reconnected();

    deferred_lines.into_iter().collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MissingTargetScope {
    PaneWindow,
    PaneWindowSession,
}

impl MissingTargetScope {
    fn matches(self, message: &str) -> bool {
        tmux::tmux_target_is_missing(message.as_bytes())
            || (self == MissingTargetScope::PaneWindowSession
                && message.contains("can't find session"))
    }
}

pub(crate) struct StartedTmuxControlModeClient {
    child: Option<std::process::Child>,
    stdout_reader: Option<BufReader<std::process::ChildStdout>>,
    stdin: Option<std::process::ChildStdin>,
}

impl StartedTmuxControlModeClient {
    pub(super) fn from_real(
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
}

pub(super) struct RunningTmuxControlModeClient {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    line_rx: mpsc::Receiver<Result<ControlModeLine>>,
    // Retained so per-session subscriber clients and primary reconnects can feed
    // the same shared event stream the run loop drains. Because this keeps a live
    // sender, the channel never reports `Disconnected` on its own; primary-client
    // death is instead detected by the events the primary forwards — `%exit` on a
    // clean tmux exit and a `Fatal` read error on an abnormal one — not by channel
    // closure.
    line_tx: mpsc::Sender<Result<ControlModeLine>>,
    // Event-only control clients, one per non-primary session. tmux control mode
    // is scoped to the attached session, so these provide event coverage for
    // panes the primary client cannot see. They never issue commands; their
    // reader threads feed the shared channel.
    subscribers: Vec<SubscriberClient>,
    recent_dead_subscribers: Vec<ipc::ControlModeSubscriberStatusFrame>,
    primary_session_id: Option<String>,
    desired_subscriber_session_ids: Vec<String>,
    missing_subscriber_session_ids: Vec<String>,
    subscriber_start_counts: HashMap<String, u64>,
    last_subscriber_reconcile_at: Option<String>,
    // False when there are more non-primary sessions than the subscriber cap, so
    // some sessions have no event client. The run loop keeps the reconcile poll
    // active in that case rather than relaxing to the self-heal backstop.
    subscriber_coverage_complete: bool,
    deferred_lines: VecDeque<ControlModeLine>,
    broker_health: TmuxBrokerHealth,
}

struct SubscriberClient {
    session_id: String,
    child: std::process::Child,
    started_at: String,
    last_line_at: Option<String>,
    last_event_at: Option<String>,
    restart_count: u64,
    known_dead: bool,
    // Retained for keep-alive and per-pane output gating (`refresh-client -A`).
    #[allow(dead_code)]
    stdin: std::process::ChildStdin,
}

fn subscriber_status_frame(subscriber: &SubscriberClient) -> ipc::ControlModeSubscriberStatusFrame {
    ipc::ControlModeSubscriberStatusFrame {
        session_id: subscriber.session_id.clone(),
        pid: subscriber.child.id(),
        started_at: subscriber.started_at.clone(),
        last_line_at: subscriber.last_line_at.clone(),
        last_event_at: subscriber.last_event_at.clone(),
        restart_count: subscriber.restart_count,
        dead: subscriber.known_dead,
    }
}

fn remove_recent_dead_subscriber(
    recent_dead_subscribers: &mut Vec<ipc::ControlModeSubscriberStatusFrame>,
    session_id: &str,
) {
    recent_dead_subscribers.retain(|subscriber| subscriber.session_id != session_id);
}

fn record_recent_dead_subscribers(
    recent_dead_subscribers: &mut Vec<ipc::ControlModeSubscriberStatusFrame>,
    dead_subscribers: Vec<ipc::ControlModeSubscriberStatusFrame>,
) {
    for dead_subscriber in dead_subscribers {
        remove_recent_dead_subscriber(recent_dead_subscribers, &dead_subscriber.session_id);
        recent_dead_subscribers.push(dead_subscriber);
    }
}

fn merge_subscriber_status_frames(
    active_subscribers: Vec<ipc::ControlModeSubscriberStatusFrame>,
    recent_dead_subscribers: &[ipc::ControlModeSubscriberStatusFrame],
) -> (usize, Vec<ipc::ControlModeSubscriberStatusFrame>) {
    let active_dead_count = active_subscribers
        .iter()
        .filter(|subscriber| subscriber.dead)
        .count();
    let mut subscribers = active_subscribers;
    let visible_recent_dead_subscribers = recent_dead_subscribers
        .iter()
        .filter(|dead_subscriber| {
            !subscribers
                .iter()
                .any(|subscriber| subscriber.session_id == dead_subscriber.session_id)
        })
        .cloned()
        .collect::<Vec<_>>();
    let dead_subscriber_count =
        active_dead_count.saturating_add(visible_recent_dead_subscribers.len());
    subscribers.extend(visible_recent_dead_subscribers);
    (dead_subscriber_count, subscribers)
}

#[cfg(test)]
fn test_subscriber_status_frame(
    session_id: &str,
    dead: bool,
) -> ipc::ControlModeSubscriberStatusFrame {
    ipc::ControlModeSubscriberStatusFrame {
        session_id: session_id.to_string(),
        pid: 123,
        started_at: "2026-05-03T00:00:00Z".to_string(),
        last_line_at: None,
        last_event_at: None,
        restart_count: 0,
        dead,
    }
}

#[cfg(test)]
pub(crate) fn test_subscriber_status_drops_recovered_dead_tombstone() -> (usize, Vec<(String, bool)>)
{
    let active_subscribers = vec![test_subscriber_status_frame("$2", false)];
    let recent_dead_subscribers = vec![
        test_subscriber_status_frame("$2", true),
        test_subscriber_status_frame("$3", true),
    ];
    let (dead_count, subscribers) =
        merge_subscriber_status_frames(active_subscribers, &recent_dead_subscribers);

    (
        dead_count,
        subscribers
            .into_iter()
            .map(|subscriber| (subscriber.session_id, subscriber.dead))
            .collect(),
    )
}

#[cfg(test)]
pub(crate) fn test_recent_dead_subscriber_tombstone_persists_without_new_dead() -> Vec<String> {
    let mut recent_dead_subscribers = vec![test_subscriber_status_frame("$2", true)];

    record_recent_dead_subscribers(&mut recent_dead_subscribers, Vec::new());

    recent_dead_subscribers
        .into_iter()
        .map(|subscriber| subscriber.session_id)
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ControlModeLineSource {
    Primary { session_id: Option<Arc<str>> },
    Subscriber { session_id: Arc<str> },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ControlModeLine {
    pub(super) source: ControlModeLineSource,
    pub(super) line: String,
}

impl ControlModeLine {
    pub(super) fn new(source: ControlModeLineSource, line: String) -> Self {
        Self { source, line }
    }

    pub(super) fn is_subscriber(&self) -> bool {
        matches!(self.source, ControlModeLineSource::Subscriber { .. })
    }

    pub(super) fn is_primary(&self) -> bool {
        matches!(self.source, ControlModeLineSource::Primary { .. })
    }

    pub(super) fn source_frame_seed(&self) -> ipc::ControlModeSourceFrame {
        match &self.source {
            ControlModeLineSource::Primary { session_id } => ipc::ControlModeSourceFrame {
                source: "primary".to_string(),
                session_id: session_id.as_ref().map(ToString::to_string),
                line_count: 0,
                event_count: 0,
            },
            ControlModeLineSource::Subscriber { session_id } => ipc::ControlModeSourceFrame {
                source: "subscriber".to_string(),
                session_id: Some(session_id.to_string()),
                line_count: 0,
                event_count: 0,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct SubscriberReconcileOutcome {
    pub(super) pruned_dead_count: u64,
    pub(super) started_count: u64,
    pub(super) reattached_count: u64,
    pub(super) attach_failure_count: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SubscriberAttachOutcome {
    AlreadyPresent,
    Attached { reattached: bool },
}

impl Drop for SubscriberClient {
    fn drop(&mut self) {
        cleanup_startup_child(&mut self.child);
    }
}

impl RunningTmuxControlModeClient {
    pub(super) fn from_started(
        started: StartedTmuxControlModeClient,
        primary_session_id: Option<String>,
    ) -> Result<Self> {
        let (child, stdin, line_rx, line_tx) =
            connect_primary_control_client(started, primary_session_id.clone())?;
        Ok(Self {
            child,
            stdin,
            line_rx,
            line_tx,
            subscribers: Vec::new(),
            recent_dead_subscribers: Vec::new(),
            primary_session_id,
            desired_subscriber_session_ids: Vec::new(),
            missing_subscriber_session_ids: Vec::new(),
            subscriber_start_counts: HashMap::new(),
            last_subscriber_reconcile_at: None,
            subscriber_coverage_complete: true,
            deferred_lines: VecDeque::new(),
            broker_health: TmuxBrokerHealth::default(),
        })
    }

    // Attach an event-only subscriber client for a non-primary session. Its
    // reader feeds the shared channel; a subscriber read error ends only its own
    // thread (Quiet) so one failing session does not bounce the daemon.
    pub(super) fn attach_subscriber(
        &mut self,
        session_id: String,
        started: StartedTmuxControlModeClient,
    ) -> Result<SubscriberAttachOutcome> {
        if self.subscribers.iter().any(|s| s.session_id == session_id) {
            return Ok(SubscriberAttachOutcome::AlreadyPresent);
        }
        let previous_start_count = self
            .subscriber_start_counts
            .get(&session_id)
            .copied()
            .unwrap_or_default();
        let (child, stdin) = spawn_control_client_reader(
            started,
            self.line_tx.clone(),
            ClientErrorMode::Quiet,
            ControlModeLineSource::Subscriber {
                session_id: Arc::<str>::from(session_id.as_str()),
            },
        )
        .with_context(|| {
            format!("failed to attach subscriber control client for session {session_id}")
        })?;
        let start_count = previous_start_count.saturating_add(1);
        self.subscriber_start_counts
            .insert(session_id.clone(), start_count);
        remove_recent_dead_subscriber(&mut self.recent_dead_subscribers, &session_id);
        let reattached = previous_start_count > 0;
        self.subscribers.push(SubscriberClient {
            session_id,
            child,
            started_at: snapshot::now_rfc3339().unwrap_or_else(|_| "unknown".to_string()),
            last_line_at: None,
            last_event_at: None,
            restart_count: start_count.saturating_sub(1),
            known_dead: false,
            stdin,
        });
        Ok(SubscriberAttachOutcome::Attached { reattached })
    }

    pub(super) fn has_subscriber(&self, session_id: &str) -> bool {
        self.subscribers
            .iter()
            .any(|subscriber| subscriber.session_id == session_id)
    }

    // Drop subscribers whose client process has exited so a following reconcile
    // re-attaches them. Covers the case where a subscriber client dies while its
    // session is still alive (a closed session is instead handled by
    // `retain_subscriber_sessions`, since the session leaves the desired set).
    //
    // Safe to run after `has_dead_subscriber` has already `try_wait`-ed the same
    // children: `Child::try_wait` records the exit status on the `Child` and
    // returns that cached `Ok(Some(status))` on every subsequent call (it does
    // not reap-then-report-`None`). So the earlier detection call does not consume
    // the status out from under this prune — both observe the exited child.
    pub(super) fn prune_dead_subscribers(&mut self) -> usize {
        let before = self.subscribers.len();
        let mut dead_subscribers = Vec::new();
        self.subscribers.retain_mut(|subscriber| {
            let dead = matches!(subscriber.child.try_wait(), Ok(Some(_)));
            if dead {
                subscriber.known_dead = true;
                dead_subscribers.push(subscriber_status_frame(subscriber));
            }
            !dead
        });
        record_recent_dead_subscribers(&mut self.recent_dead_subscribers, dead_subscribers);
        before.saturating_sub(self.subscribers.len())
    }

    // Whether any subscriber's client process has exited. A dead subscriber means
    // its session is no longer event-covered, but `has_subscriber` (membership
    // only) still reports it covered, so the run loop polls this on each timeout
    // to trigger a prune + re-attach + coverage recompute promptly, instead of
    // waiting for the self-heal reconcile that the stale coverage would delay.
    //
    // Calling `try_wait` here does not prevent the subsequent
    // `prune_dead_subscribers` from seeing the same exit: the status is cached on
    // the `Child` and re-reported by later `try_wait` calls (see that method).
    pub(super) fn has_dead_subscriber(&mut self) -> bool {
        let mut has_dead = false;
        for subscriber in &mut self.subscribers {
            if matches!(subscriber.child.try_wait(), Ok(Some(_))) {
                subscriber.known_dead = true;
                has_dead = true;
            }
        }
        has_dead
    }

    pub(super) fn subscriber_count(&self) -> usize {
        self.subscribers.len()
    }

    // Whether the primary control client process has exited. Because the retained
    // `line_tx` keeps the shared channel from ever reporting `Disconnected`, the
    // run loop polls this to detect a primary that died without emitting `%exit`
    // (e.g. the tmux server was SIGKILLed and the pipe closed at EOF). Checks the
    // current primary child, so it is correct across reconnects (which install a
    // fresh, live child).
    pub(super) fn primary_child_exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    pub(super) fn set_subscriber_coverage(
        &mut self,
        desired: Vec<String>,
        missing: Vec<String>,
        complete: bool,
    ) {
        self.desired_subscriber_session_ids = desired;
        self.missing_subscriber_session_ids = missing;
        self.subscriber_coverage_complete = complete;
        self.last_subscriber_reconcile_at =
            Some(snapshot::now_rfc3339().unwrap_or_else(|_| "unknown".to_string()));
    }

    pub(super) fn subscriber_coverage_complete(&self) -> bool {
        self.subscriber_coverage_complete
    }

    // Drop subscriber clients whose sessions are no longer desired (each dropped
    // `SubscriberClient` cleans up its child process).
    pub(super) fn retain_subscriber_sessions(&mut self, desired: &[String]) {
        let removed_session_ids = self
            .subscribers
            .iter()
            .filter(|subscriber| !desired.iter().any(|id| id == &subscriber.session_id))
            .map(|subscriber| subscriber.session_id.clone())
            .collect::<Vec<_>>();
        self.subscribers
            .retain(|subscriber| desired.iter().any(|id| id == &subscriber.session_id));
        for session_id in removed_session_ids {
            self.subscriber_start_counts.remove(&session_id);
            remove_recent_dead_subscriber(&mut self.recent_dead_subscribers, &session_id);
        }
    }

    pub(super) fn read_provider(&mut self) -> TmuxDaemonReadProvider<'_> {
        TmuxDaemonReadProvider {
            fallback: TmuxCommandReadProvider,
            broker: self
                .broker_health
                .enabled()
                .then_some(TmuxControlModeReadBroker {
                    stdin: &mut self.stdin,
                    line_rx: &self.line_rx,
                    deferred_lines: &mut self.deferred_lines,
                    broker_health: &mut self.broker_health,
                }),
        }
    }

    pub(super) fn recv_timeout(
        &mut self,
        timeout: Duration,
    ) -> std::result::Result<Result<ControlModeLine>, mpsc::RecvTimeoutError> {
        let result = if let Some(line) = self.deferred_lines.pop_front() {
            Ok(Ok(line))
        } else {
            self.line_rx.recv_timeout(timeout)
        };
        if let Ok(Ok(line)) = &result {
            self.mark_line_seen(line);
        }
        result
    }

    fn mark_line_seen(&mut self, line: &ControlModeLine) {
        let ControlModeLineSource::Subscriber { session_id } = &line.source else {
            return;
        };
        let now = snapshot::now_rfc3339().unwrap_or_else(|_| "unknown".to_string());
        if let Some(subscriber) = self
            .subscribers
            .iter_mut()
            .find(|subscriber| subscriber.session_id.as_str() == session_id.as_ref())
        {
            subscriber.last_line_at = Some(now.clone());
            if !matches!(control_event_from_line(&line.line), ControlEvent::Ignored) {
                subscriber.last_event_at = Some(now);
            }
        }
    }

    pub(super) fn broker_status_frame(&self) -> ipc::ControlModeBrokerStatusFrame {
        self.broker_status_frame_with_deadlines(None, None)
    }

    pub(super) fn broker_status_frame_with_deadlines(
        &self,
        next_subscriber_monitor_in_ms: Option<u64>,
        next_reconcile_in_ms: Option<u64>,
    ) -> ipc::ControlModeBrokerStatusFrame {
        let active_subscribers = self
            .subscribers
            .iter()
            .map(subscriber_status_frame)
            .collect::<Vec<_>>();
        let (dead_subscriber_count, subscribers) =
            merge_subscriber_status_frames(active_subscribers, &self.recent_dead_subscribers);

        ipc::ControlModeBrokerStatusFrame {
            subscriber_count: Some(self.subscriber_count()),
            primary_session_id: self.primary_session_id.clone(),
            subscriber_coverage_complete: Some(self.subscriber_coverage_complete),
            desired_subscriber_count: Some(self.desired_subscriber_session_ids.len()),
            active_subscriber_count: Some(self.subscribers.len()),
            missing_subscriber_session_ids: Some(self.missing_subscriber_session_ids.clone()),
            dead_subscriber_count: Some(dead_subscriber_count),
            subscribers: Some(subscribers),
            last_subscriber_reconcile_at: self.last_subscriber_reconcile_at.clone(),
            next_subscriber_monitor_in_ms,
            next_reconcile_in_ms,
            ..self.broker_health.status_frame()
        }
    }

    pub(super) fn broker_enabled(&self) -> bool {
        self.broker_health.enabled()
    }

    pub(super) fn recover_broker_if_disabled(
        &mut self,
        startup: &impl StartupActions,
        socket_state: &DaemonSocketState,
    ) -> bool {
        if self.broker_health.enabled() {
            return false;
        }

        socket_state.update_control_mode_broker_status(self.broker_status_frame());
        match self.reconnect(startup) {
            Ok(()) => {
                socket_state.update_control_mode_broker_status(self.broker_status_frame());
                true
            }
            Err(error) => {
                eprintln!(
                    "agentscan: failed to reconnect tmux control-mode broker; continuing with short-lived tmux reads: {error:#}"
                );
                false
            }
        }
    }

    fn reconnect(&mut self, startup: &impl StartupActions) -> Result<()> {
        // Reconnect runs only after a forwarded fatal error disabled the broker, so
        // the old primary reader has already terminated and any frames it emitted
        // are now sitting unread in the shared channel. Drain them before the new
        // reader starts producing, so a stale command response from the dead
        // connection cannot be misattributed to a post-reconnect brokered command.
        // See `drain_control_mode_channel`.
        drain_control_mode_channel(&self.line_rx, &mut self.deferred_lines);
        let started = startup
            .start_tmux_control_mode_client()
            .context("failed to restart tmux control-mode client")?;
        let replacement_primary_session_id = startup
            .primary_session_id_for_status()
            .or_else(|| self.primary_session_id.clone());
        // Re-spawn the primary reader into the existing shared channel rather than
        // recreating it, so any attached subscriber clients keep delivering events.
        let (mut replacement_child, replacement_stdin) = spawn_control_client_reader(
            started,
            self.line_tx.clone(),
            ClientErrorMode::Fatal,
            ControlModeLineSource::Primary {
                session_id: replacement_primary_session_id
                    .as_deref()
                    .map(Arc::<str>::from),
            },
        )
        .context("failed to configure restarted tmux control-mode client")?;

        std::mem::swap(&mut self.child, &mut replacement_child);
        cleanup_startup_child(&mut replacement_child);
        self.stdin = replacement_stdin;
        self.primary_session_id = replacement_primary_session_id;
        self.broker_health.mark_reconnected();
        Ok(())
    }

    pub(super) fn wait_for_exit(&mut self) -> Result<()> {
        let status = self
            .child
            .wait()
            .context("failed while waiting for tmux control-mode client to exit")?;
        if !status.success() {
            bail!("tmux control-mode client exited with status {status}");
        }
        Ok(())
    }

    pub(super) fn terminate(&mut self) {
        // Dropping each subscriber cleans up its child process.
        self.subscribers.clear();
        cleanup_startup_child(&mut self.child);
    }
}

impl Drop for RunningTmuxControlModeClient {
    fn drop(&mut self) {
        cleanup_startup_child(&mut self.child);
    }
}

// Connect a control client's stdout to a shared event channel. The channel is
// owned at the broker level and stays stable across primary reconnects, so the
// returned `line_tx` can be cloned to feed per-session subscriber clients into
// the same stream the run loop drains.
type PrimaryControlConnection = (
    std::process::Child,
    std::process::ChildStdin,
    mpsc::Receiver<Result<ControlModeLine>>,
    mpsc::Sender<Result<ControlModeLine>>,
);

fn connect_primary_control_client(
    started: StartedTmuxControlModeClient,
    primary_session_id: Option<String>,
) -> Result<PrimaryControlConnection> {
    let (line_tx, line_rx) = mpsc::channel();
    let (child, stdin) = spawn_control_client_reader(
        started,
        line_tx.clone(),
        ClientErrorMode::Fatal,
        ControlModeLineSource::Primary {
            session_id: primary_session_id.as_deref().map(Arc::<str>::from),
        },
    )?;
    Ok((child, stdin, line_rx, line_tx))
}

// Discard every frame currently buffered in the shared control-mode channel.
//
// Used on primary reconnect. The shared channel is created once and retained
// across reconnects (so subscriber clients keep feeding the same stream), which
// means it is never replaced the way `main` replaced the receiver on reconnect.
// Without an explicit drain, command frames left over from the dead connection —
// e.g. the `%begin`/`%end` of a brokered command that timed out just before the
// reconnect — stay buffered in the receiver. The production command collector
// (`collect_control_mode_command_outcome`) treats the first `%begin` it reads as
// the active command, so a later brokered command could otherwise consume that
// stale response and return an old or mismatched snapshot. Only the primary issues
// commands, so the primary is the only source of stray `%begin`/`%end`; draining
// here restores `main`'s discard-on-reconnect behavior for the shared channel.
fn drain_control_mode_channel(
    line_rx: &mpsc::Receiver<Result<ControlModeLine>>,
    deferred_lines: &mut VecDeque<ControlModeLine>,
) {
    while let Ok(frame) = line_rx.try_recv() {
        if let Ok(line) = frame
            && line.is_subscriber()
        {
            deferred_lines.push_back(line);
        }
    }
}

// How a client's reader thread reports a read error on its stdout. The primary
// client's errors are fatal (they propagate to the run loop and bounce the
// daemon into broker recovery), but a subscriber client dying must not take the
// daemon down — it just ends its own thread and is reconnected by the lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ClientErrorMode {
    Fatal,
    Quiet,
}

fn spawn_control_client_reader(
    mut started: StartedTmuxControlModeClient,
    line_tx: mpsc::Sender<Result<ControlModeLine>>,
    error_mode: ClientErrorMode,
    source: ControlModeLineSource,
) -> Result<(std::process::Child, std::process::ChildStdin)> {
    let stdout_reader = started
        .stdout_reader
        .take()
        .context("tmux control-mode client did not provide stdout")?;
    let child = started
        .child
        .take()
        .context("tmux control-mode client did not provide child process")?;
    let stdin = started
        .stdin
        .take()
        .context("tmux control-mode client did not provide stdin")?;
    spawn_control_mode_line_reader(stdout_reader, line_tx, error_mode, source);
    Ok((child, stdin))
}

// A `%exit` on a subscriber (Quiet) client signals only that this one
// non-primary control client is detaching, so it must not be forwarded to the
// shared stream (where `%exit` parses as a server-wide `ControlEvent::Exit`).
// The primary (Fatal) client still forwards `%exit` to drive daemon shutdown.
fn subscriber_local_exit(error_mode: ClientErrorMode, line: &str) -> bool {
    matches!(error_mode, ClientErrorMode::Quiet) && is_control_exit_line(line)
}

#[cfg(test)]
pub(crate) fn test_subscriber_local_exit(quiet: bool, line: &str) -> bool {
    let error_mode = if quiet {
        ClientErrorMode::Quiet
    } else {
        ClientErrorMode::Fatal
    };
    subscriber_local_exit(error_mode, line)
}

fn spawn_control_mode_line_reader(
    stdout_reader: BufReader<std::process::ChildStdout>,
    line_tx: mpsc::Sender<Result<ControlModeLine>>,
    error_mode: ClientErrorMode,
    source: ControlModeLineSource,
) {
    std::thread::spawn(move || {
        let mut reader = stdout_reader;
        loop {
            match read_control_mode_line(&mut reader) {
                Ok(Some(line)) => {
                    // A subscriber client's `%exit` reports only that this one
                    // client is detaching (e.g. its session was killed); it must
                    // not reach the shared stream, where `%exit` is parsed as a
                    // server-wide `ControlEvent::Exit` that stops the daemon loop.
                    // The subscriber's removal is handled by `%sessions-changed`
                    // reconcile and dead-subscriber pruning instead. The primary
                    // client (Fatal) still forwards `%exit` to drive shutdown.
                    if subscriber_local_exit(error_mode, &line) {
                        break;
                    }
                    if line_tx
                        .send(Ok(ControlModeLine::new(source.clone(), line)))
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    if matches!(error_mode, ClientErrorMode::Fatal) {
                        let _ = line_tx.send(Err(error));
                    }
                    break;
                }
            }
        }
    });
}

extern "C" fn daemon_shutdown_signal_handler(_signal: libc::c_int) {
    DAEMON_SHUTDOWN_REQUESTED.store(true, Ordering::Relaxed);
}

pub(super) fn install_shutdown_signal_handlers() {
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

pub(super) struct DaemonClosingGuard {
    state: DaemonSocketState,
    marked: bool,
}

impl DaemonClosingGuard {
    pub(super) fn new(state: DaemonSocketState) -> Self {
        Self {
            state,
            marked: false,
        }
    }

    pub(super) fn mark_closing(&mut self) {
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

pub(super) fn startup_failure_message(context: &str, error: &anyhow::Error) -> String {
    format!(
        "{context} failed before daemon socket readiness; no usable socket snapshot was published: {error:#}"
    )
}

// All control clients attach with these flags:
//   ignore-size: the client never participates in window-size calculation, so
//     attaching (especially to a detached session) cannot resize the user's panes.
//   no-output: tmux never sends `%output` to the client. The daemon does not need
//     the pty firehose — status/title/command/metadata arrive via the 1s
//     `%subscription-changed` stream and topology via structural notifications.
//     This is what keeps cost flat as the number of active agents grows.
const CONTROL_CLIENT_ATTACH_FLAGS: &str = "ignore-size,no-output";

// Start the primary control client attached to an explicit session target. The
// caller resolves and owns the primary session id (once, at startup) so the
// primary attach and the subscriber-exclusion set agree and do not drift if the
// launching tmux client later switches sessions.
pub(super) fn start_tmux_control_mode_client_for(
    session_target: &str,
) -> Result<(
    std::process::Child,
    BufReader<std::process::ChildStdout>,
    std::process::ChildStdin,
)> {
    start_control_mode_client_for(session_target)
}

pub(super) fn start_subscriber_control_mode_client(
    session_id: &str,
) -> Result<(
    std::process::Child,
    BufReader<std::process::ChildStdout>,
    std::process::ChildStdin,
)> {
    start_control_mode_client_for(session_id)
}

fn start_control_mode_client_for(
    session_target: &str,
) -> Result<(
    std::process::Child,
    BufReader<std::process::ChildStdout>,
    std::process::ChildStdin,
)> {
    let mut child = tmux::tmux_command()
        .args([
            "-C",
            "attach-session",
            "-f",
            CONTROL_CLIENT_ATTACH_FLAGS,
            "-t",
            session_target,
        ])
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

pub(super) fn control_mode_startup_response_from_line(line: &str, context: &str) -> Result<bool> {
    if line.starts_with("%error") {
        bail!("tmux rejected {context}: {line}");
    }
    Ok(line.starts_with("%end"))
}

fn control_mode_list_all_panes(
    stdin: &mut std::process::ChildStdin,
    line_rx: &mpsc::Receiver<Result<ControlModeLine>>,
    deferred_lines: &mut VecDeque<ControlModeLine>,
) -> Result<Vec<TmuxPaneRow>> {
    writeln!(stdin, "list-panes -a -F '{PANE_FORMAT}'")
        .context("failed to write brokered tmux list-panes -a")?;
    stdin
        .flush()
        .context("failed to flush brokered tmux list-panes -a")?;

    match collect_control_mode_command_outcome(
        line_rx,
        CONTROL_MODE_COMMAND_TIMEOUT,
        deferred_lines,
    )? {
        ControlModeBrokerCommandOutcome::Response(response) => {
            tmux::parse_pane_rows(&response.output.join("\n"))
        }
        ControlModeBrokerCommandOutcome::Error { message, .. } => {
            bail!("tmux control-mode command failed: {message}");
        }
    }
}

fn control_mode_list_panes_target(
    stdin: &mut std::process::ChildStdin,
    line_rx: &mpsc::Receiver<Result<ControlModeLine>>,
    target: &str,
    deferred_lines: &mut VecDeque<ControlModeLine>,
    missing_target_scope: MissingTargetScope,
) -> Result<Option<Vec<TmuxPaneRow>>> {
    writeln!(stdin, "list-panes -t {target} -F '{PANE_FORMAT}'")
        .with_context(|| format!("failed to write brokered tmux list-panes for {target}"))?;
    stdin
        .flush()
        .with_context(|| format!("failed to flush brokered tmux list-panes for {target}"))?;

    match collect_control_mode_command_outcome(
        line_rx,
        CONTROL_MODE_COMMAND_TIMEOUT,
        deferred_lines,
    )? {
        ControlModeBrokerCommandOutcome::Response(response) => {
            let rows = tmux::parse_pane_rows(&response.output.join("\n"))?;
            Ok(Some(rows))
        }
        ControlModeBrokerCommandOutcome::Error {
            message,
            deferred_events: _,
        } if missing_target_scope.matches(&message) => Ok(None),
        ControlModeBrokerCommandOutcome::Error { message, .. } => {
            bail!("tmux control-mode command failed: {message}");
        }
    }
}

fn collect_control_mode_command_outcome(
    line_rx: &mpsc::Receiver<Result<ControlModeLine>>,
    timeout: Duration,
    deferred_lines: &mut VecDeque<ControlModeLine>,
) -> Result<ControlModeBrokerCommandOutcome> {
    let deadline = Instant::now() + timeout;
    let mut active_id = None;
    let mut output = Vec::new();

    loop {
        let now = Instant::now();
        if now >= deadline {
            bail!("timed out waiting for control-mode command response");
        }
        let frame = line_rx
            .recv_timeout(deadline.saturating_duration_since(now))
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => {
                    anyhow::anyhow!("timed out waiting for control-mode command response")
                }
                mpsc::RecvTimeoutError::Disconnected => {
                    anyhow::anyhow!(
                        "tmux control-mode stream ended before command response completed"
                    )
                }
            })??;
        if !frame.is_primary() {
            deferred_lines.push_back(frame);
            continue;
        }
        let line = frame.line.as_str();

        if active_id.is_some() && control_mode_broker_line_is_command_output(line) {
            output.push(frame.line);
            continue;
        }

        match (active_id.as_ref(), control_mode_command_marker(line)) {
            (None, Some(ControlModeCommandMarker::Begin(id))) => active_id = Some(id),
            (Some(id), Some(ControlModeCommandMarker::End(end_id))) if *id == end_id => {
                return Ok(ControlModeBrokerCommandOutcome::Response(
                    ControlModeCommandPrototypeResponse {
                        output,
                        deferred_events: Vec::new(),
                    },
                ));
            }
            (
                Some(id),
                Some(ControlModeCommandMarker::Error {
                    id: error_id,
                    message,
                }),
            ) if *id == error_id => {
                let message = control_mode_command_error_message(message, &output);
                return Ok(ControlModeBrokerCommandOutcome::Error {
                    message,
                    deferred_events: Vec::new(),
                });
            }
            (Some(_), Some(_)) => {
                bail!("interleaved control-mode command frame before expected %end");
            }
            (Some(_), None) if control_mode_broker_should_defer_line(line) => {
                deferred_lines.push_back(frame);
            }
            (Some(_), None) => output.push(frame.line),
            (None, Some(_)) | (None, None) => deferred_lines.push_back(frame),
        }
    }
}

fn control_mode_broker_line_is_command_output(line: &str) -> bool {
    line.contains(PANE_DELIM) || line.contains(TMUX_FORMAT_DELIM)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ControlModeCommandFrameId {
    pub(crate) timestamp: String,
    pub(crate) command_number: String,
    pub(crate) flags: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ControlModeCommandMarker {
    Begin(ControlModeCommandFrameId),
    End(ControlModeCommandFrameId),
    Error {
        id: ControlModeCommandFrameId,
        message: String,
    },
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn control_mode_command_marker(line: &str) -> Option<ControlModeCommandMarker> {
    let mut parts = line.splitn(5, ' ');
    let marker = parts.next()?;
    if !matches!(marker, "%begin" | "%end" | "%error") {
        return None;
    }

    let id = ControlModeCommandFrameId {
        timestamp: parts.next()?.to_string(),
        command_number: parts.next()?.to_string(),
        flags: parts.next()?.to_string(),
    };

    match marker {
        "%begin" => Some(ControlModeCommandMarker::Begin(id)),
        "%end" => Some(ControlModeCommandMarker::End(id)),
        "%error" => Some(ControlModeCommandMarker::Error {
            id,
            message: parts.next().unwrap_or_default().to_string(),
        }),
        _ => None,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ControlModeCommandPrototypeResponse {
    pub(crate) output: Vec<String>,
    pub(crate) deferred_events: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ControlModeBrokerCommandOutcome {
    Response(ControlModeCommandPrototypeResponse),
    Error {
        message: String,
        deferred_events: Vec<String>,
    },
}

#[cfg(test)]
pub(crate) fn test_collect_control_mode_command_response<'a>(
    expected_id: &ControlModeCommandFrameId,
    lines: impl IntoIterator<Item = &'a str>,
) -> Result<ControlModeCommandPrototypeResponse> {
    let mut started = false;
    let mut output = Vec::new();
    let mut deferred_events = Vec::new();

    for line in lines {
        match control_mode_command_marker(line) {
            Some(ControlModeCommandMarker::Begin(id)) if id == *expected_id && !started => {
                started = true;
            }
            Some(ControlModeCommandMarker::Begin(_)) if started => {
                bail!("interleaved control-mode command frame before expected %end");
            }
            Some(ControlModeCommandMarker::End(id)) if id == *expected_id && started => {
                return Ok(ControlModeCommandPrototypeResponse {
                    output,
                    deferred_events,
                });
            }
            Some(ControlModeCommandMarker::Error { id, message })
                if id == *expected_id && started =>
            {
                bail!("tmux control-mode command failed: {message}");
            }
            Some(_) if started => {
                bail!("interleaved control-mode command frame before expected %end");
            }
            Some(_) | None if started => output.push(line.to_string()),
            Some(_) | None => deferred_events.push(line.to_string()),
        }
    }

    bail!("control-mode command response ended before expected %end")
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ControlModeBrokerTranscriptStep {
    Line(String),
    Timeout,
    Eof,
}

#[cfg(test)]
impl ControlModeBrokerTranscriptStep {
    pub(crate) fn line(line: impl Into<String>) -> Self {
        Self::Line(line.into())
    }
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct ControlModeBrokerTranscriptHarness {
    steps: std::collections::VecDeque<ControlModeBrokerTranscriptStep>,
    written_commands: Vec<String>,
}

#[cfg(test)]
impl ControlModeBrokerTranscriptHarness {
    pub(crate) fn new(steps: impl IntoIterator<Item = ControlModeBrokerTranscriptStep>) -> Self {
        Self {
            steps: steps.into_iter().collect(),
            written_commands: Vec::new(),
        }
    }

    pub(crate) fn written_commands(&self) -> &[String] {
        &self.written_commands
    }

    pub(crate) fn list_all_panes(
        &mut self,
        expected_id: &ControlModeCommandFrameId,
    ) -> Result<ControlModeBrokerListAllResponse> {
        self.written_commands
            .push(format!("list-panes -a -F {PANE_FORMAT}"));

        let response = match self.collect_command_outcome(expected_id)? {
            ControlModeBrokerCommandOutcome::Response(response) => response,
            ControlModeBrokerCommandOutcome::Error { message, .. } => {
                bail!("tmux control-mode command failed: {message}");
            }
        };
        let rows = tmux::parse_pane_rows(&response.output.join("\n"))?;
        Ok(ControlModeBrokerListAllResponse {
            rows,
            deferred_events: response.deferred_events,
        })
    }

    pub(crate) fn list_pane(
        &mut self,
        pane_id: &str,
        expected_id: &ControlModeCommandFrameId,
    ) -> Result<ControlModeBrokerListPaneResponse> {
        let response = self.list_target_panes_with_scope(
            pane_id,
            expected_id,
            MissingTargetScope::PaneWindow,
        )?;
        Ok(ControlModeBrokerListPaneResponse {
            pane: response.rows.and_then(|mut rows| rows.pop()),
            deferred_events: response.deferred_events,
        })
    }

    pub(crate) fn list_target_panes(
        &mut self,
        target: &str,
        expected_id: &ControlModeCommandFrameId,
    ) -> Result<ControlModeBrokerListTargetResponse> {
        self.list_target_panes_with_scope(
            target,
            expected_id,
            MissingTargetScope::PaneWindowSession,
        )
    }

    fn list_target_panes_with_scope(
        &mut self,
        target: &str,
        expected_id: &ControlModeCommandFrameId,
        missing_target_scope: MissingTargetScope,
    ) -> Result<ControlModeBrokerListTargetResponse> {
        self.written_commands
            .push(format!("list-panes -t {target} -F {PANE_FORMAT}"));

        let response = match self.collect_command_outcome(expected_id)? {
            ControlModeBrokerCommandOutcome::Response(response) => response,
            ControlModeBrokerCommandOutcome::Error {
                message,
                deferred_events,
            } if missing_target_scope.matches(&message) => {
                return Ok(ControlModeBrokerListTargetResponse {
                    rows: None,
                    deferred_events,
                });
            }
            ControlModeBrokerCommandOutcome::Error { message, .. } => {
                bail!("tmux control-mode command failed: {message}");
            }
        };
        let rows = tmux::parse_pane_rows(&response.output.join("\n"))?;
        Ok(ControlModeBrokerListTargetResponse {
            rows: Some(rows),
            deferred_events: response.deferred_events,
        })
    }

    pub(crate) fn collect_command_response(
        &mut self,
        expected_id: &ControlModeCommandFrameId,
    ) -> Result<ControlModeCommandPrototypeResponse> {
        match self.collect_command_outcome(expected_id)? {
            ControlModeBrokerCommandOutcome::Response(response) => Ok(response),
            ControlModeBrokerCommandOutcome::Error { message, .. } => {
                bail!("tmux control-mode command failed: {message}");
            }
        }
    }

    fn collect_command_outcome(
        &mut self,
        expected_id: &ControlModeCommandFrameId,
    ) -> Result<ControlModeBrokerCommandOutcome> {
        let mut started = false;
        let mut output = Vec::new();
        let mut deferred_events = Vec::new();

        loop {
            let Some(line) = self.next_line()? else {
                bail!("tmux control-mode stream ended before command response completed");
            };

            if started && control_mode_broker_line_is_command_output(&line) {
                output.push(line);
                continue;
            }

            match control_mode_command_marker(&line) {
                Some(ControlModeCommandMarker::Begin(id)) if id == *expected_id && !started => {
                    started = true;
                }
                Some(ControlModeCommandMarker::Begin(_)) if started => {
                    bail!("interleaved control-mode command frame before expected %end");
                }
                Some(ControlModeCommandMarker::End(id)) if id == *expected_id && started => {
                    return Ok(ControlModeBrokerCommandOutcome::Response(
                        ControlModeCommandPrototypeResponse {
                            output,
                            deferred_events,
                        },
                    ));
                }
                Some(ControlModeCommandMarker::Error { id, message })
                    if id == *expected_id && started =>
                {
                    let message = control_mode_command_error_message(message, &output);
                    return Ok(ControlModeBrokerCommandOutcome::Error {
                        message,
                        deferred_events,
                    });
                }
                Some(_) if started => {
                    bail!("interleaved control-mode command frame before expected %end");
                }
                None if started && control_mode_broker_should_defer_line(&line) => {
                    deferred_events.push(line);
                }
                Some(_) | None if started => output.push(line),
                Some(_) | None => deferred_events.push(line),
            }
        }
    }

    fn next_line(&mut self) -> Result<Option<String>> {
        match self.steps.pop_front() {
            Some(ControlModeBrokerTranscriptStep::Line(line)) => Ok(Some(line)),
            Some(ControlModeBrokerTranscriptStep::Timeout) => {
                bail!("timed out waiting for control-mode command response");
            }
            Some(ControlModeBrokerTranscriptStep::Eof) | None => Ok(None),
        }
    }
}

fn control_mode_command_error_message(marker_message: String, output: &[String]) -> String {
    if marker_message.is_empty() {
        output.last().cloned().unwrap_or_default()
    } else {
        marker_message
    }
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct ControlModeBrokerListPaneResponse {
    pub(crate) pane: Option<TmuxPaneRow>,
    pub(crate) deferred_events: Vec<String>,
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct ControlModeBrokerListAllResponse {
    pub(crate) rows: Vec<TmuxPaneRow>,
    pub(crate) deferred_events: Vec<String>,
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct ControlModeBrokerListTargetResponse {
    pub(crate) rows: Option<Vec<TmuxPaneRow>>,
    pub(crate) deferred_events: Vec<String>,
}

fn control_mode_broker_should_defer_line(line: &str) -> bool {
    !matches!(control_event_from_line(line), ControlEvent::Ignored)
}
