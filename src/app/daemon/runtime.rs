use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::app::{
    SnapshotEnvelope, StatusKind, StatusSource, config, ipc, proc, scanner, snapshot,
};

use super::control_mode::{
    ControlModeLine, RunningTmuxControlModeClient, StartedTmuxControlModeClient,
    SubscriberAttachOutcome, SubscriberReconcileOutcome,
};
use super::events::{ControlEventBatch, batch_changed_session_set};
use super::refresh::{
    apply_control_event_batch, reconcile_full_snapshot, reconcile_refresh_outcome,
    refresh_snapshot_for_focused_pane, refresh_snapshot_pane_with_title, snapshot_diff,
    snapshots_are_materially_equal,
};
use super::socket_server::{DaemonSocketServerHandle, PreparedSnapshot, SnapshotPublishContext};
use super::telemetry::{DaemonEventTrace, RefreshObservability, RuntimeTelemetry};
use super::{
    CONTROL_MODE_EVENT_BATCH_WINDOW, CONTROL_MODE_FALLBACK_RECONCILE_INTERVAL,
    CONTROL_MODE_MAX_WAIT, CONTROL_MODE_MIN_WAIT, DAEMON_SHUTDOWN_REQUESTED, DaemonSocketState,
    MAX_CONTROL_MODE_SUBSCRIBERS, PANE_OUTPUT_SETTLE_DELAY, PANE_OUTPUT_STATUS_CACHE_TTL,
    SOCKET_IDENTITY_CHECK_INTERVAL, SUBSCRIBER_MONITOR_POLL_INTERVAL, StartupActions,
    client_event_detail, control_mode_active_reconcile_interval, control_mode_self_heal_interval,
    deep_control_mode_telemetry_enabled, duration_millis_u64, duration_until_millis,
};

pub(crate) struct DaemonRuntime<S> {
    startup: S,
    socket_state: DaemonSocketState,
    tmux_version: Option<String>,
    snapshot: SnapshotEnvelope,
    pane_output_cache: scanner::PaneOutputStatusCache,
    control_mode: RunningTmuxControlModeClient,
    next_reconcile_at: Instant,
    next_subscriber_monitor_at: Option<Instant>,
    // When set, a pane-output provider is believed busy and the daemon should re-read it once
    // the event stream goes quiet, to catch the idle transition (which emits no event).
    settle_recapture_at: Option<Instant>,
    telemetry: RuntimeTelemetry,
    pane_focus_recency: PaneFocusRecency,
    deep_control_mode_telemetry: bool,
    disable_reconcile: bool,
    disable_proc_fallback: bool,
    event_trace: Option<DaemonEventTrace>,
}

/// Focus-recency overlay fed exclusively by explicit `PaneFocus` client
/// events (every agentscan focus path emits one after tmux focus succeeds).
/// Overlay invariant: the runtime `snapshot` always holds `last_focus_seq:
/// None`; stamps are applied only to published clones at the publish
/// boundary. This keeps every snapshot rebuild site harmless (nothing
/// in-snapshot to wipe) and means recency can never defeat no-op publication
/// suppression, whose gates compare runtime snapshots. Any future observed
/// -transition stamping (AUR-698 v2) must keep publish-boundary stamping.
#[derive(Debug, Default)]
struct PaneFocusRecency {
    /// tmux pane id (`%N`, never reused within a server run) -> ordinal seq.
    by_pane: HashMap<String, u64>,
    next_seq: u64,
}

impl PaneFocusRecency {
    /// Record a focus of `pane_id`. Repeat focus of the current MRU head is
    /// a no-op so the common re-focus gesture keeps publishing empty
    /// `changed_panes` diffs. Returns whether recency changed.
    fn record(&mut self, pane_id: &str) -> bool {
        let head = self.by_pane.values().max().copied();
        if head.is_some() && self.by_pane.get(pane_id).copied() == head {
            return false;
        }
        self.next_seq += 1;
        self.by_pane.insert(pane_id.to_string(), self.next_seq);
        true
    }

    /// Drop entries for panes no longer present, bounding the map by live
    /// panes ever focused this daemon session.
    fn prune(&mut self, snapshot: &SnapshotEnvelope) {
        self.by_pane
            .retain(|pane_id, _| snapshot.panes.iter().any(|pane| &pane.pane_id == pane_id));
    }

    /// Stamp recency onto a snapshot clone bound for publication.
    fn stamp(&self, snapshot: &mut SnapshotEnvelope) {
        for pane in &mut snapshot.panes {
            pane.last_focus_seq = self.by_pane.get(&pane.pane_id).copied();
        }
    }
}

pub(crate) enum RefreshRequest<'a> {
    IntervalReconcile,
    TimeoutReconcile,
    ControlModeLines(&'a [ControlModeLine]),
    ClientEvent(&'a ipc::ClientEventFrame),
    SettleRecapture,
}

pub(crate) struct RefreshOutcome {
    should_exit: bool,
    publish_context: Option<SnapshotPublishContext>,
    reset_reconcile_timer: bool,
}

impl RefreshOutcome {
    fn no_publish() -> Self {
        Self {
            should_exit: false,
            publish_context: None,
            reset_reconcile_timer: false,
        }
    }

    pub(super) fn no_publish_and_reset_reconcile_timer() -> Self {
        Self {
            should_exit: false,
            publish_context: None,
            reset_reconcile_timer: true,
        }
    }

    fn publish(publish_context: SnapshotPublishContext) -> Self {
        Self {
            should_exit: false,
            publish_context: Some(publish_context),
            reset_reconcile_timer: false,
        }
    }

    pub(super) fn publish_and_reset_reconcile_timer(
        publish_context: SnapshotPublishContext,
    ) -> Self {
        Self {
            should_exit: false,
            publish_context: Some(publish_context),
            reset_reconcile_timer: true,
        }
    }
}

// Reconcile the set of event-only subscriber clients against the live sessions:
// attach a subscriber for every non-primary session that lacks one and drop
// subscribers whose sessions have closed. Run at startup and on every
// `%sessions-changed`, so sessions created or destroyed at runtime get event
// coverage without relying on the periodic reconcile. Best-effort: failures are
// logged and skipped (the primary session is always covered by the primary
// client, and a failed subscriber falls back to self-heal reconcile latency).
// Bound the subscriber set so a pathological session count cannot spawn an
// unbounded number of `tmux -C` clients. The selection is deterministic (sorted)
// so the same sessions keep their clients across reconciles instead of churning;
// the dropped remainder relies on the self-heal reconcile for cross-session
// coverage.
// Numeric ordering key for a tmux session id (`$12` -> 12). Ids that do not fit
// the `$<number>` shape sort last (by `u64::MAX`) and then fall back to the
// lexical tiebreak in the caller, so selection stays deterministic.
fn subscriber_session_sort_key(session_id: &str) -> u64 {
    session_id
        .strip_prefix('$')
        .and_then(|digits| digits.parse::<u64>().ok())
        .unwrap_or(u64::MAX)
}

fn capped_subscriber_session_ids(mut session_ids: Vec<String>) -> Vec<String> {
    if session_ids.len() > MAX_CONTROL_MODE_SUBSCRIBERS {
        // Sort by numeric session index, not lexically: tmux session ids are
        // unpadded (`$2` sorts after `$19` as strings), so a plain string sort
        // would mis-select which sessions keep their event clients. Keeping the
        // lowest indices is deterministic and stable across reconciles.
        session_ids.sort_by_key(|id| (subscriber_session_sort_key(id), id.clone()));
        eprintln!(
            "agentscan: {} non-primary sessions exceed the subscriber cap ({}); \
             {} sessions fall back to the self-heal reconcile for cross-session coverage",
            session_ids.len(),
            MAX_CONTROL_MODE_SUBSCRIBERS,
            session_ids.len() - MAX_CONTROL_MODE_SUBSCRIBERS,
        );
        session_ids.truncate(MAX_CONTROL_MODE_SUBSCRIBERS);
    }
    session_ids
}

fn reconcile_subscribers<S: StartupActions>(
    startup: &S,
    control_mode: &mut RunningTmuxControlModeClient,
) -> SubscriberReconcileOutcome {
    // Drop subscribers whose client process died so the loop below re-attaches
    // them; a closed session is handled separately by retain (it leaves the set).
    let mut outcome = SubscriberReconcileOutcome {
        pruned_dead_count: control_mode
            .prune_dead_subscribers()
            .try_into()
            .unwrap_or(u64::MAX),
        ..Default::default()
    };
    let desired_session_ids = match startup.additional_subscriber_session_ids() {
        Ok(session_ids) => session_ids,
        Err(error) => {
            eprintln!(
                "agentscan: failed to enumerate sessions for subscriber clients; \
                 keeping the active reconcile until coverage is re-established: {error:#}"
            );
            // We could not verify subscriber coverage this pass (and may have
            // started none yet, e.g. at startup where the flag defaults to true),
            // so mark coverage incomplete to keep the active reconcile poll rather
            // than relaxing to the self-heal backstop. A later reconcile retries.
            control_mode.set_subscriber_coverage(Vec::new(), Vec::new(), false);
            return outcome;
        }
    };
    let under_cap = desired_session_ids.len() <= MAX_CONTROL_MODE_SUBSCRIBERS;
    let capped_session_ids = capped_subscriber_session_ids(desired_session_ids.clone());
    control_mode.retain_subscriber_sessions(&capped_session_ids);
    for session_id in &capped_session_ids {
        if control_mode.has_subscriber(session_id) {
            continue;
        }
        match startup.start_subscriber_client(session_id) {
            Ok(started) => match control_mode.attach_subscriber(session_id.clone(), started) {
                Ok(SubscriberAttachOutcome::AlreadyPresent) => {}
                Ok(SubscriberAttachOutcome::Attached { reattached }) => {
                    outcome.started_count = outcome.started_count.saturating_add(1);
                    if reattached {
                        outcome.reattached_count = outcome.reattached_count.saturating_add(1);
                    }
                }
                Err(error) => {
                    outcome.attach_failure_count = outcome.attach_failure_count.saturating_add(1);
                    eprintln!(
                        "agentscan: failed to attach subscriber client for session {session_id}: {error:#}"
                    );
                }
            },
            Err(error) => {
                outcome.attach_failure_count = outcome.attach_failure_count.saturating_add(1);
                eprintln!(
                    "agentscan: failed to start subscriber client for session {session_id}: {error:#}"
                );
            }
        }
    }
    // Coverage is complete only when nothing was dropped by the cap *and* every
    // desired session actually ended up with a live subscriber. A failed attach
    // (transient tmux error, resource limit) leaves that session event-uncovered,
    // so coverage is incomplete and the reconcile poll stays active (see
    // `reconcile_interval_for`) until a later reconcile re-attaches it, rather than
    // relaxing to the self-heal backstop and starving the session.
    let coverage_complete = subscriber_coverage_complete(under_cap, &desired_session_ids, |id| {
        control_mode.has_subscriber(id)
    });
    let missing_session_ids = desired_session_ids
        .iter()
        .filter(|session_id| !control_mode.has_subscriber(session_id))
        .cloned()
        .collect();
    control_mode.set_subscriber_coverage(
        desired_session_ids,
        missing_session_ids,
        coverage_complete,
    );
    outcome
}

// Subscriber coverage is complete only if the cap dropped nothing (`under_cap`)
// and every desired session currently has a subscriber. Pure for testability.
fn subscriber_coverage_complete(
    under_cap: bool,
    desired: &[String],
    has_subscriber: impl Fn(&str) -> bool,
) -> bool {
    under_cap && desired.iter().all(|id| has_subscriber(id))
}

#[cfg(test)]
pub(super) fn run_subscriber_coverage_complete(
    under_cap: bool,
    desired: &[String],
    present: &[String],
) -> bool {
    subscriber_coverage_complete(under_cap, desired, |id| {
        present.iter().any(|candidate| candidate == id)
    })
}

impl<S: StartupActions> DaemonRuntime<S> {
    pub(super) fn from_started(
        startup: S,
        socket_state: DaemonSocketState,
        tmux_version: Option<String>,
        pending_snapshot: PreparedSnapshot,
        tmux_client: StartedTmuxControlModeClient,
        runtime_options: config::ResolvedRuntimeOptions,
        event_trace: Option<DaemonEventTrace>,
    ) -> Result<Self> {
        let snapshot = pending_snapshot.snapshot.clone();
        socket_state.publish_prepared_snapshot(pending_snapshot);
        let mut control_mode = RunningTmuxControlModeClient::from_started(
            tmux_client,
            startup.primary_session_id_for_status(),
        )?;
        socket_state.set_client_event_sender(control_mode.event_sender());
        let mut telemetry = RuntimeTelemetry::default();
        let subscriber_reconcile = reconcile_subscribers(&startup, &mut control_mode);
        telemetry.record_subscriber_reconcile(subscriber_reconcile);
        let pane_output_cache = scanner::PaneOutputStatusCache::new(PANE_OUTPUT_STATUS_CACHE_TTL);
        let now = Instant::now();
        let next_reconcile_at = now
            + reconcile_interval_for(
                control_mode.broker_enabled(),
                runtime_options.disable_reconcile,
                control_mode.subscriber_coverage_complete(),
            );
        let next_subscriber_monitor_at = next_subscriber_monitor_deadline(&control_mode, now);
        let broker_status = control_mode.broker_status_frame_with_deadlines(
            next_subscriber_monitor_at.map(duration_until_millis),
            Some(duration_until_millis(next_reconcile_at)),
        );
        socket_state.update_control_mode_broker_status(broker_status.clone());
        socket_state.update_runtime_telemetry(
            telemetry.frame(&broker_status, pane_output_cache.capture_stats()),
        );
        Ok(Self {
            startup,
            socket_state,
            tmux_version,
            snapshot,
            pane_output_cache,
            control_mode,
            next_reconcile_at,
            next_subscriber_monitor_at,
            settle_recapture_at: None,
            telemetry,
            pane_focus_recency: PaneFocusRecency::default(),
            deep_control_mode_telemetry: deep_control_mode_telemetry_enabled(),
            disable_reconcile: runtime_options.disable_reconcile,
            disable_proc_fallback: runtime_options.disable_proc_fallback,
            event_trace,
        })
    }

    pub(super) fn run(&mut self, server_handle: &DaemonSocketServerHandle) -> Result<()> {
        // Arm the settle re-check from the boot snapshot: a pane already classified `Busy` from
        // pane output at startup that then goes quiet would otherwise never get a busy->idle
        // re-check (the deadline is only refreshed after a refresh request runs), leaving it
        // stuck busy until the next reconcile. `update_settle_deadline` is set-when-None, so this
        // is a no-op when nothing is busy.
        self.update_settle_deadline();
        // The socket-identity check is a coarse self-heal backstop; stat the path on a
        // fixed cadence rather than every wakeup. Seed it so the first loop iteration runs
        // the check immediately.
        let mut next_socket_identity_check_at = Instant::now();
        loop {
            if DAEMON_SHUTDOWN_REQUESTED.load(Ordering::Relaxed) {
                break;
            }
            if Instant::now() >= next_socket_identity_check_at {
                next_socket_identity_check_at = Instant::now() + SOCKET_IDENTITY_CHECK_INTERVAL;
                if !server_handle.socket_still_matches() {
                    eprintln!(
                        "agentscan: daemon socket path no longer matches this daemon; exiting"
                    );
                    DAEMON_SHUTDOWN_REQUESTED.store(true, Ordering::Relaxed);
                    break;
                }
            }
            if !server_handle.accept_thread_alive() {
                // The acceptor stopped without a recorded shutdown reason (e.g. a
                // panic in the accept loop). A daemon that no longer accepts is deaf;
                // exit so the next client auto-starts a healthy one.
                eprintln!("agentscan: daemon socket acceptor stopped unexpectedly; exiting");
                DAEMON_SHUTDOWN_REQUESTED.store(true, Ordering::Relaxed);
                break;
            }
            if Instant::now() >= self.next_reconcile_at {
                self.apply_refresh_request(RefreshRequest::IntervalReconcile)?;
                // The periodic reconcile is also the self-heal backstop for the
                // subscriber set: prune subscribers whose client died and re-attach
                // any missing sessions, even without a `%sessions-changed` event.
                self.reconcile_subscriber_clients();
            }

            if self
                .next_subscriber_monitor_at
                .is_some_and(|at| Instant::now() >= at)
            {
                self.monitor_subscriber_clients();
            }

            // A pane-output provider's idle transition emits no tmux event, so poll any pane
            // believed busy on the settle cadence. Clear the deadline before firing so the
            // post-refresh re-arm reflects the fresh result (re-armed if still busy, else
            // cleared) rather than the stale past instant.
            if self
                .settle_recapture_at
                .is_some_and(|at| Instant::now() >= at)
            {
                self.settle_recapture_at = None;
                self.apply_refresh_request(RefreshRequest::SettleRecapture)?;
            }

            let timeout = self.next_control_mode_wait();
            match self.control_mode.recv_timeout(timeout) {
                Ok(line) => {
                    let line = line?;
                    if let Some(event) = line.emitted_client_event() {
                        if self.apply_refresh_request(RefreshRequest::ClientEvent(&event))? {
                            break;
                        }
                        continue;
                    }
                    let lines = self.collect_control_mode_batch(line)?;
                    let session_set_changed = batch_changed_session_set(&lines);
                    if self.apply_refresh_request(RefreshRequest::ControlModeLines(&lines))? {
                        break;
                    }
                    if session_set_changed {
                        self.reconcile_subscriber_clients();
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // The retained sender means the channel never reports
                    // `Disconnected`, so poll the primary child directly to catch a
                    // primary that died without a `%exit` (e.g. the tmux server was
                    // SIGKILLed). MAX_WAIT bounds this to sub-second detection.
                    if self.control_mode.primary_child_exited() {
                        eprintln!(
                            "agentscan: tmux control-mode primary client exited; daemon stopping"
                        );
                        break;
                    }
                    // A subscriber client that died while its session is still alive
                    // leaves coverage stale (reported complete, so the interval stays
                    // at the 300s self-heal). Detect it here, bounded by MAX_WAIT, and
                    // reconcile to prune + re-attach and recompute coverage promptly.
                    if self.control_mode.has_dead_subscriber() {
                        self.reconcile_subscriber_clients();
                    }
                    if Instant::now() >= self.next_reconcile_at {
                        self.apply_refresh_request(RefreshRequest::TimeoutReconcile)?;
                        self.reconcile_subscriber_clients();
                    }
                }
                // Best-effort backstop only: with the retained sender the channel
                // does not disconnect on its own; primary death is detected by the
                // `%exit` event, a forwarded read error, or the liveness poll above.
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        Ok(())
    }

    fn next_control_mode_wait(&self) -> Duration {
        next_control_mode_wait_for(
            self.next_reconcile_at,
            self.next_subscriber_monitor_at,
            self.settle_recapture_at,
            Instant::now(),
        )
    }

    // Re-derive the subscriber client set from the live sessions. Called when a
    // `%sessions-changed` notification indicates a session was created or
    // destroyed, so runtime session changes get event coverage immediately
    // rather than waiting for the self-heal reconcile.
    fn reconcile_subscriber_clients(&mut self) {
        let outcome = reconcile_subscribers(&self.startup, &mut self.control_mode);
        self.telemetry.record_subscriber_reconcile(outcome);
        self.next_subscriber_monitor_at =
            next_subscriber_monitor_deadline(&self.control_mode, Instant::now());
        // Coverage may have just become incomplete (pushed over the cap), which
        // shortens the reconcile interval. Pull the next reconcile in so we do not
        // wait out an older, longer self-heal deadline before polling the
        // un-subscribed sessions. Never push the deadline out (min only).
        self.next_reconcile_at = self
            .next_reconcile_at
            .min(Instant::now() + self.reconcile_interval());
        // Republish broker status after deadline adjustment so telemetry reflects
        // the subscriber coverage state and the actual next reconcile deadline.
        let broker_status = self.broker_status_frame();
        self.socket_state
            .update_control_mode_broker_status(broker_status.clone());
        self.socket_state.update_runtime_telemetry(
            self.telemetry
                .frame(&broker_status, self.pane_output_cache.capture_stats()),
        );
    }

    fn monitor_subscriber_clients(&mut self) {
        self.telemetry.record_subscriber_monitor();
        if self.control_mode.has_dead_subscriber() {
            self.reconcile_subscriber_clients();
        } else {
            self.next_subscriber_monitor_at =
                next_subscriber_monitor_deadline(&self.control_mode, Instant::now());
            self.update_runtime_telemetry();
        }
    }

    fn collect_control_mode_batch(
        &mut self,
        first_line: ControlModeLine,
    ) -> Result<Vec<ControlModeLine>> {
        let mut lines = vec![first_line];
        let deadline = Instant::now() + CONTROL_MODE_EVENT_BATCH_WINDOW;
        loop {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            match self
                .control_mode
                .recv_timeout(deadline.saturating_duration_since(now))
            {
                Ok(line) => {
                    let line = line?;
                    if line.is_client_event() {
                        self.control_mode.defer_line(line);
                        break;
                    }
                    lines.push(line);
                }
                Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {
                    break;
                }
            }
        }
        Ok(lines)
    }

    fn apply_refresh_request(&mut self, request: RefreshRequest<'_>) -> Result<bool> {
        let started_at = Instant::now();
        let observability = RefreshObservability::from_request(&request);
        // Single pre-refresh clone shared by every consumer that needs the before-state:
        // the observability diff below and the reconcile/publish gates inside each refresh
        // method (threaded in as `pre_refresh` so they no longer each re-clone the snapshot).
        let previous_snapshot = observability
            .should_capture_snapshot_diff()
            .then(|| self.snapshot.clone());
        let pre_refresh = previous_snapshot.as_ref();
        let mut outcome = match request {
            RefreshRequest::IntervalReconcile => self.apply_reconcile_refresh(
                SnapshotPublishContext::new("reconcile").with_detail("interval"),
                pre_refresh,
            )?,
            RefreshRequest::TimeoutReconcile => self.apply_reconcile_refresh(
                SnapshotPublishContext::new("reconcile").with_detail("timeout"),
                pre_refresh,
            )?,
            RefreshRequest::ControlModeLines(lines) => {
                self.apply_control_mode_refresh(lines, pre_refresh)?
            }
            RefreshRequest::ClientEvent(event) => {
                self.apply_client_event_refresh(event, pre_refresh)?
            }
            RefreshRequest::SettleRecapture => self.apply_settle_recapture_refresh(pre_refresh)?,
        };
        let publish_context = outcome.publish_context.take();
        let published = if let Some(publish_context) = publish_context {
            self.publish_current_snapshot(publish_context)
        } else {
            false
        };
        // The current snapshot is `self.snapshot` itself; `record_observability_event`
        // borrows it directly rather than taking a redundant clone here.
        self.record_observability_event(
            observability,
            previous_snapshot.as_ref(),
            &outcome,
            published,
            started_at.elapsed(),
        );
        if outcome.reset_reconcile_timer {
            self.next_reconcile_at = Instant::now() + self.reconcile_interval();
        }
        // Re-arm (or clear) the settle deadline from the current snapshot: any refresh that
        // leaves a pane-output provider busy or waiting means we must re-read it once the event
        // stream goes quiet. Activity-bearing refreshes keep pushing the deadline out; the pass
        // only fires after the turn's output stops.
        self.update_settle_deadline();
        Ok(outcome.should_exit)
    }

    /// Maintain `settle_recapture_at` as a steady re-check deadline whenever any pane reads
    /// busy or waiting from captured pane output. Such a status has no tmux event to refresh it
    /// when the turn ends, so the daemon polls it: the deadline is armed once when an active
    /// pane-output pane appears and is left alone while set, so unrelated panes' activity cannot
    /// push it out (which would starve the re-check). It is re-armed after each fire and cleared
    /// once no pane-output pane is busy or waiting.
    fn update_settle_deadline(&mut self) {
        let has_active_pane_output = self.snapshot.panes.iter().any(|pane| {
            pane.status.source == StatusSource::PaneOutput
                && matches!(pane.status.kind, StatusKind::Busy | StatusKind::Waiting)
        });
        self.settle_recapture_at = next_settle_deadline(
            has_active_pane_output,
            self.settle_recapture_at,
            Instant::now(),
            PANE_OUTPUT_SETTLE_DELAY,
        );
    }

    /// Re-read pane-output providers currently believed busy or waiting, to catch an idle
    /// transition that emitted no tmux event. The cache entry is invalidated first so the
    /// re-read forces a fresh capture (these panes are otherwise not fallback candidates).
    fn apply_settle_recapture_refresh(
        &mut self,
        pre_refresh: Option<&SnapshotEnvelope>,
    ) -> Result<RefreshOutcome> {
        let active_ids: Vec<String> = self
            .snapshot
            .panes
            .iter()
            .filter(|pane| {
                pane.status.source == StatusSource::PaneOutput
                    && matches!(pane.status.kind, StatusKind::Busy | StatusKind::Waiting)
            })
            .map(|pane| pane.pane_id.clone())
            .collect();
        if active_ids.is_empty() {
            return Ok(RefreshOutcome::no_publish());
        }

        let owned_previous;
        let previous_snapshot = match pre_refresh {
            Some(previous) => previous,
            None => {
                owned_previous = self.snapshot.clone();
                &owned_previous
            }
        };
        let mut tmux_reads = self.control_mode.read_provider();
        // One lazily-captured process table for the whole settle pass, matching
        // the control-event batch path.
        let proc_inspector = proc::ProcProcessInspector;
        let proc_snapshot = proc::LazyProcessSnapshot::new(&proc_inspector);
        for pane_id in &active_ids {
            self.pane_output_cache.invalidate(pane_id);
            refresh_snapshot_pane_with_title(
                &mut self.snapshot,
                &mut tmux_reads,
                pane_id,
                None,
                &mut self.pane_output_cache,
                &proc_snapshot,
                self.disable_proc_fallback,
            )?;
        }

        if snapshots_are_materially_equal(previous_snapshot, &self.snapshot) {
            self.update_runtime_telemetry();
            Ok(RefreshOutcome::no_publish())
        } else {
            Ok(RefreshOutcome::publish(
                SnapshotPublishContext::new("pane_output_settle").with_detail("busy_recheck"),
            ))
        }
    }

    fn record_observability_event(
        &mut self,
        observability: RefreshObservability,
        previous_snapshot: Option<&SnapshotEnvelope>,
        outcome: &RefreshOutcome,
        published: bool,
        duration: Duration,
    ) {
        if !observability.should_record && !outcome.reset_reconcile_timer && !published {
            return;
        }
        // The current snapshot is `self.snapshot`; borrow it for the diff instead of
        // cloning. The borrow ends once `event` is built, before the `&mut self` writes.
        let current_snapshot = &self.snapshot;
        let changed = previous_snapshot
            .is_some_and(|previous| !snapshots_are_materially_equal(previous, current_snapshot));
        let event = ipc::DaemonObservabilityEventFrame {
            at: snapshot::now_rfc3339().unwrap_or_else(|_| "unknown".to_string()),
            source: observability.source.to_string(),
            detail: observability.detail.into_detail(),
            refresh: observability.refresh.to_string(),
            control_sources: observability.control_sources,
            control_lines: observability.control_lines,
            changed,
            published,
            duration_ms: Some(duration_millis_u64(duration)),
            diff: previous_snapshot
                .and_then(|previous| changed.then(|| snapshot_diff(previous, current_snapshot))),
        };
        self.socket_state.record_observability_event(event.clone());
        if let Some(trace) = &mut self.event_trace {
            trace.write(&event);
        }
    }

    fn reconcile_interval(&self) -> Duration {
        reconcile_interval_for(
            self.control_mode.broker_enabled(),
            self.disable_reconcile,
            self.control_mode.subscriber_coverage_complete(),
        )
    }

    fn apply_control_mode_refresh(
        &mut self,
        lines: &[ControlModeLine],
        pre_refresh: Option<&SnapshotEnvelope>,
    ) -> Result<RefreshOutcome> {
        let batch = ControlEventBatch::from_control_lines(lines);
        self.telemetry.record_control_event_volume(&batch);
        let should_record_batch_telemetry =
            batch.has_telemetry_event() || self.deep_control_mode_telemetry;
        let has_subscriber_line = lines.iter().any(ControlModeLine::is_subscriber);
        if should_record_batch_telemetry {
            self.telemetry.record_control_event_kinds(&batch);
        }
        let should_exit = batch.should_exit;
        let event_publish_context = batch.publish_context();
        // The before-state for the reconcile telemetry and the publish gate below is
        // `pre_refresh`, the single pre-refresh clone taken in `apply_refresh_request`. It is
        // present on every path that reaches those uses: both are gated on the batch having
        // materially refreshed (`can_refresh_full_snapshot`/`event_outcome.changed`), which in
        // turn forces `observability_refresh() != "none"` and hence `should_capture_snapshot_diff`.
        let broker_enabled_before_refresh = self.control_mode.broker_enabled();
        let mut event_tmux_reads = self.control_mode.read_provider();
        let event_outcome = apply_control_event_batch(
            &mut self.snapshot,
            &mut event_tmux_reads,
            &batch,
            &mut self.pane_output_cache,
            self.disable_proc_fallback,
        )?;
        if !event_outcome.changed {
            let (reconnected, reset_reconcile_timer) =
                if control_event_should_recover_broker(should_exit) {
                    let reconnected = self.recover_broker_and_reconcile_if_needed()?;
                    let reset_reconcile_timer = control_event_refresh_should_reset_reconcile_timer(
                        broker_enabled_before_refresh,
                        reconnected,
                        self.control_mode.broker_enabled(),
                    );
                    (reconnected, reset_reconcile_timer)
                } else {
                    (false, false)
                };
            if should_record_batch_telemetry || has_subscriber_line {
                self.update_runtime_telemetry();
            }
            let mut outcome = if reconnected {
                RefreshOutcome::publish(
                    SnapshotPublishContext::new("reconcile").with_detail("broker_reconnect"),
                )
            } else if reset_reconcile_timer {
                RefreshOutcome::no_publish_and_reset_reconcile_timer()
            } else {
                RefreshOutcome::no_publish()
            };
            outcome.should_exit = should_exit;
            outcome.reset_reconcile_timer = reset_reconcile_timer;
            return Ok(outcome);
        }
        self.telemetry.record_control_event_refresh(&event_outcome);
        if event_outcome.full_snapshot_refresh
            && batch.can_refresh_full_snapshot()
            && let Some(previous_snapshot) = pre_refresh
        {
            self.telemetry
                .record_reconcile_result(previous_snapshot, &self.snapshot);
        }
        if event_outcome.fallback_to_full {
            self.telemetry.record_targeted_refresh_fallback_to_full();
        }

        let reconnected = self.recover_broker_and_reconcile_if_needed()?;
        let mut outcome = if reconnected {
            RefreshOutcome::publish(
                SnapshotPublishContext::new("reconcile").with_detail("broker_reconnect"),
            )
        } else if pre_refresh
            .is_some_and(|before| snapshots_are_materially_equal(before, &self.snapshot))
        {
            // The refresh ran but produced no material change (for example, a pane-output
            // activity tick whose status stayed busy); skip the redundant publish.
            self.update_runtime_telemetry();
            RefreshOutcome::no_publish()
        } else {
            RefreshOutcome::publish(event_publish_context.unwrap_or_else(|| {
                SnapshotPublishContext::new("control_event").with_detail("unknown")
            }))
        };
        outcome.should_exit = should_exit;
        if control_event_refresh_should_reset_reconcile_timer(
            broker_enabled_before_refresh,
            reconnected,
            self.control_mode.broker_enabled(),
        ) {
            outcome.reset_reconcile_timer = true;
        }
        Ok(outcome)
    }

    fn apply_reconcile_refresh(
        &mut self,
        publish_context: SnapshotPublishContext,
        pre_refresh: Option<&SnapshotEnvelope>,
    ) -> Result<RefreshOutcome> {
        let owned_previous;
        let previous_snapshot = match pre_refresh {
            Some(previous) => previous,
            None => {
                owned_previous = self.snapshot.clone();
                &owned_previous
            }
        };
        let mut reconcile_tmux_reads = self.control_mode.read_provider();
        reconcile_full_snapshot(
            &mut self.snapshot,
            &mut reconcile_tmux_reads,
            self.tmux_version.as_deref(),
            &mut self.pane_output_cache,
            self.disable_proc_fallback,
        )?;
        self.telemetry
            .record_reconcile_result(previous_snapshot, &self.snapshot);
        self.recover_broker_and_reconcile_if_needed()?;
        let outcome = reconcile_refresh_outcome(previous_snapshot, &self.snapshot, publish_context);
        if outcome.publish_context.is_none() {
            self.update_runtime_telemetry();
        }
        Ok(outcome)
    }

    fn apply_client_event_refresh(
        &mut self,
        event: &ipc::ClientEventFrame,
        pre_refresh: Option<&SnapshotEnvelope>,
    ) -> Result<RefreshOutcome> {
        match event {
            ipc::ClientEventFrame::PaneFocus { pane_id } => {
                let owned_previous;
                let previous_snapshot = match pre_refresh {
                    Some(previous) => previous,
                    None => {
                        owned_previous = self.snapshot.clone();
                        &owned_previous
                    }
                };
                self.pane_focus_recency.record(pane_id);
                let mut event_tmux_reads = self.control_mode.read_provider();
                refresh_snapshot_for_focused_pane(
                    &mut self.snapshot,
                    &mut event_tmux_reads,
                    pane_id,
                    self.tmux_version.as_deref(),
                    &mut self.pane_output_cache,
                    self.disable_proc_fallback,
                )?;
                self.pane_focus_recency.prune(&self.snapshot);
                self.telemetry
                    .record_reconcile_result(previous_snapshot, &self.snapshot);
                self.recover_broker_and_reconcile_if_needed()?;
                Ok(RefreshOutcome::publish(
                    SnapshotPublishContext::new("client_event")
                        .with_detail(client_event_detail(event)),
                ))
            }
        }
    }

    fn recover_broker_and_reconcile_if_needed(&mut self) -> Result<bool> {
        let reconnected = self
            .control_mode
            .recover_broker_if_disabled(&self.startup, &self.socket_state);
        if reconnected {
            let previous_snapshot = self.snapshot.clone();
            let tmux_version = self.snapshot.source.tmux_version.clone();
            let mut reconnect_tmux_reads = self.control_mode.read_provider();
            reconcile_full_snapshot(
                &mut self.snapshot,
                &mut reconnect_tmux_reads,
                tmux_version.as_deref(),
                &mut self.pane_output_cache,
                self.disable_proc_fallback,
            )?;
            self.telemetry
                .record_reconcile_result(&previous_snapshot, &self.snapshot);
        }
        Ok(reconnected)
    }

    fn publish_current_snapshot(&self, publish_context: SnapshotPublishContext) -> bool {
        self.update_runtime_telemetry();
        // TODO(alloc): `publish_later_snapshot_with_context` takes the snapshot by value and
        // stores it in `PreparedSnapshot` (owned by the socket state), so the daemon must keep
        // its own copy — this clone is required by the current socket_server API boundary.
        // `encode_snapshot_frame` then clones it a second time to build the wire frame. Both
        // clones live behind socket_server (owned by another workstream); collapsing them needs
        // an `Arc<SnapshotEnvelope>` handoff there, not a change here.
        let mut snapshot = self.snapshot.clone();
        // Publish boundary: the only place recency reaches a snapshot (see
        // the PaneFocusRecency overlay invariant).
        self.pane_focus_recency.stamp(&mut snapshot);
        self.socket_state
            .publish_later_snapshot_with_context(snapshot, publish_context)
    }

    fn broker_status_frame(&self) -> ipc::ControlModeBrokerStatusFrame {
        self.control_mode.broker_status_frame_with_deadlines(
            self.next_subscriber_monitor_at.map(duration_until_millis),
            Some(duration_until_millis(self.next_reconcile_at)),
        )
    }

    fn update_runtime_telemetry(&self) {
        let broker_status = self.broker_status_frame();
        self.socket_state
            .update_control_mode_broker_status(broker_status.clone());
        self.socket_state.update_runtime_telemetry(
            self.telemetry
                .frame(&broker_status, self.pane_output_cache.capture_stats()),
        );
    }

    pub(super) fn terminate_control_mode(mut self) {
        self.control_mode.terminate();
    }

    pub(super) fn shutdown_control_mode(mut self) -> Result<()> {
        if DAEMON_SHUTDOWN_REQUESTED.load(Ordering::Relaxed) {
            self.control_mode.terminate();
        } else {
            self.control_mode.wait_for_exit()?;
        }
        Ok(())
    }
}

fn control_event_refresh_should_reset_reconcile_timer(
    broker_enabled_before_refresh: bool,
    reconnected: bool,
    broker_enabled: bool,
) -> bool {
    reconnected || (broker_enabled_before_refresh && !broker_enabled)
}

fn control_event_should_recover_broker(should_exit: bool) -> bool {
    !should_exit
}

/// Decide the next pane-output busy re-check deadline.
///
/// While a pane-output provider is busy the deadline is armed once and then left untouched
/// until it fires, so activity from *other* panes (which arrives continuously when any agent
/// is streaming) cannot keep pushing it out and starve the re-check. It clears as soon as no
/// pane-output pane is busy.
fn next_settle_deadline(
    has_busy_pane_output: bool,
    current: Option<Instant>,
    now: Instant,
    delay: Duration,
) -> Option<Instant> {
    if !has_busy_pane_output {
        return None;
    }
    current.or(Some(now + delay))
}

fn next_control_mode_wait_for(
    next_reconcile_at: Instant,
    next_subscriber_monitor_at: Option<Instant>,
    settle_recapture_at: Option<Instant>,
    now: Instant,
) -> Duration {
    // Wake for whichever comes first: the next reconcile, subscriber health
    // monitor, or pending settle re-capture.
    let mut next_wake = next_subscriber_monitor_at
        .map(|monitor_at| next_reconcile_at.min(monitor_at))
        .unwrap_or(next_reconcile_at);
    if let Some(settle_at) = settle_recapture_at {
        next_wake = next_wake.min(settle_at);
    }
    next_wake
        .saturating_duration_since(now)
        .max(CONTROL_MODE_MIN_WAIT)
        .min(CONTROL_MODE_MAX_WAIT)
}

fn next_subscriber_monitor_deadline(
    control_mode: &RunningTmuxControlModeClient,
    now: Instant,
) -> Option<Instant> {
    (control_mode.subscriber_count() > 0).then_some(now + SUBSCRIBER_MONITOR_POLL_INTERVAL)
}

fn reconcile_interval_for(
    broker_enabled: bool,
    disable_reconcile: bool,
    subscriber_coverage_complete: bool,
) -> Duration {
    if !broker_enabled {
        // No event stream at all: the reconcile poll is the sole update path, so
        // it stays fast regardless of `disable_reconcile`.
        return CONTROL_MODE_FALLBACK_RECONCILE_INTERVAL;
    }
    if disable_reconcile && subscriber_coverage_complete {
        // Every session is event-driven via its own subscriber client; the
        // reconcile is reduced to an infrequent self-heal/drift backstop.
        //
        // Known, intentional trade-off (default `disable_reconcile = true`): a
        // provider whose status comes from captured pane output
        // (`status.source = "pane_output"`, i.e. no pane-metadata or tmux-title
        // signal) only refreshes on a snapshot-changing event or a reconcile pass.
        // With `%output` paused, a pure busy/idle content change emits no event,
        // so such a provider's status can lag by up to this self-heal interval.
        // Metadata/title-driven providers are unaffected (they are event-driven).
        // This is accepted under the event-driven-first default; run with
        // `disable_reconcile = false` for 30s status refresh. See
        // docs/daemon-operations.md.
        return control_mode_self_heal_interval();
    }
    // Either redundancy reconcile is explicitly enabled, or subscriber coverage
    // is incomplete (more sessions than the cap) so the poll must stay active to
    // cover the sessions that have no event client.
    control_mode_active_reconcile_interval()
}

#[cfg(test)]
pub(super) fn run_reconcile_refresh_publish_decision(
    previous: &SnapshotEnvelope,
    current: &SnapshotEnvelope,
) -> (bool, bool) {
    let outcome = reconcile_refresh_outcome(
        previous,
        current,
        SnapshotPublishContext::new("reconcile").with_detail("test"),
    );
    (
        outcome.publish_context.is_some(),
        outcome.reset_reconcile_timer,
    )
}

#[cfg(test)]
pub(super) fn run_control_event_refresh_should_reset_reconcile_timer(
    broker_enabled_before_refresh: bool,
    reconnected: bool,
    broker_enabled: bool,
) -> bool {
    control_event_refresh_should_reset_reconcile_timer(
        broker_enabled_before_refresh,
        reconnected,
        broker_enabled,
    )
}

#[cfg(test)]
pub(super) fn run_control_event_should_recover_broker(should_exit: bool) -> bool {
    control_event_should_recover_broker(should_exit)
}

#[cfg(test)]
pub(super) fn run_reconcile_interval_for(
    broker_enabled: bool,
    disable_reconcile: bool,
    subscriber_coverage_complete: bool,
) -> Duration {
    reconcile_interval_for(
        broker_enabled,
        disable_reconcile,
        subscriber_coverage_complete,
    )
}

#[cfg(test)]
pub(super) fn run_next_settle_deadline(
    has_busy_pane_output: bool,
    current: Option<Instant>,
    now: Instant,
    delay: Duration,
) -> Option<Instant> {
    next_settle_deadline(has_busy_pane_output, current, now, delay)
}

#[cfg(test)]
pub(super) fn run_next_control_mode_wait_for(
    next_reconcile_after: Duration,
    next_subscriber_monitor_after: Option<Duration>,
    settle_recapture_after: Option<Duration>,
) -> Duration {
    let now = Instant::now();
    next_control_mode_wait_for(
        now + next_reconcile_after,
        next_subscriber_monitor_after.map(|duration| now + duration),
        settle_recapture_after.map(|duration| now + duration),
        now,
    )
}

#[cfg(test)]
pub(super) fn run_capped_subscriber_session_ids(session_ids: Vec<String>) -> Vec<String> {
    capped_subscriber_session_ids(session_ids)
}
#[cfg(test)]
mod pane_focus_recency_tests {
    use super::PaneFocusRecency;
    use crate::app::{
        CACHE_SCHEMA_VERSION, SnapshotEnvelope, SnapshotSource, SourceKind, TmuxPaneRow, classify,
    };

    fn snapshot_with_pane_ids(pane_ids: &[&str]) -> SnapshotEnvelope {
        SnapshotEnvelope {
            schema_version: CACHE_SCHEMA_VERSION,
            generated_at: "2026-07-15T00:00:00Z".to_string(),
            source: SnapshotSource {
                kind: SourceKind::Daemon,
                tmux_version: Some("3.4".to_string()),
                daemon_generated_at: None,
            },
            panes: pane_ids
                .iter()
                .map(|pane_id| {
                    classify::pane_from_row(TmuxPaneRow {
                        session_name: "session".to_string(),
                        window_index: 0,
                        pane_index: 0,
                        pane_id: (*pane_id).to_string(),
                        pane_pid: 4242,
                        pane_current_command: "codex".to_string(),
                        pane_title_raw: "title".to_string(),
                        pane_tty: "/dev/ttys0".to_string(),
                        pane_current_path: "/tmp/agentscan".to_string(),
                        window_name: "window".to_string(),
                        session_id: Some("$1".to_string()),
                        window_id: Some("@0".to_string()),
                        agent_provider: None,
                        agent_label: None,
                        agent_cwd: None,
                        agent_state: None,
                        agent_session_id: None,
                        agent_pid: None,
                        agent_version: None,
                        agent_model: None,
                        pane_active: false,
                        window_active: false,
                    })
                })
                .collect(),
        }
    }

    #[test]
    fn record_assigns_strictly_increasing_seqs() {
        let mut recency = PaneFocusRecency::default();
        assert!(recency.record("%1"));
        assert!(recency.record("%2"));
        assert!(recency.by_pane["%2"] > recency.by_pane["%1"]);
    }

    #[test]
    fn repeat_focus_of_mru_head_is_a_no_op() {
        let mut recency = PaneFocusRecency::default();
        recency.record("%1");
        recency.record("%2");
        let head_seq = recency.by_pane["%2"];
        assert!(
            !recency.record("%2"),
            "re-focusing the MRU head must not change recency"
        );
        assert_eq!(recency.by_pane["%2"], head_seq);
    }

    #[test]
    fn refocusing_an_older_pane_promotes_it_over_the_head() {
        let mut recency = PaneFocusRecency::default();
        recency.record("%1");
        recency.record("%2");
        assert!(recency.record("%1"));
        assert!(recency.by_pane["%1"] > recency.by_pane["%2"]);
    }

    #[test]
    fn prune_retains_only_live_panes() {
        let mut recency = PaneFocusRecency::default();
        recency.record("%1");
        recency.record("%2");
        recency.prune(&snapshot_with_pane_ids(&["%2"]));
        assert!(!recency.by_pane.contains_key("%1"));
        assert!(recency.by_pane.contains_key("%2"));
    }

    #[test]
    fn stamp_writes_only_known_panes_and_leaves_source_map_intact() {
        let mut recency = PaneFocusRecency::default();
        recency.record("%2");
        let mut snapshot = snapshot_with_pane_ids(&["%1", "%2"]);
        recency.stamp(&mut snapshot);
        assert_eq!(snapshot.panes[0].last_focus_seq, None);
        assert_eq!(
            snapshot.panes[1].last_focus_seq,
            Some(recency.by_pane["%2"])
        );
        // Stamping a stale clone back to None must not corrupt the map.
        assert_eq!(recency.by_pane.len(), 1);
    }
}
#[cfg(test)]
mod migrated_tests {
    use super::super::migrated_tests::empty_socket_snapshot;
    use crate::app::{daemon, tests::proc_fallback_pane};

    #[test]
    fn daemon_reconcile_publish_decision_suppresses_timestamp_only_changes() {
        let previous = empty_socket_snapshot("2026-05-23T18:00:00Z");
        let mut current = previous.clone();
        current.generated_at = "2026-05-23T18:00:01Z".to_string();
        current.source.daemon_generated_at = Some("2026-05-23T18:00:01Z".to_string());

        let (should_publish, reset_reconcile_timer) =
            super::run_reconcile_refresh_publish_decision(&previous, &current);

        assert!(!should_publish);
        assert!(reset_reconcile_timer);
    }

    #[test]
    fn daemon_reconcile_publish_decision_publishes_material_changes() {
        let previous = empty_socket_snapshot("2026-05-23T18:00:00Z");
        let mut current = previous.clone();
        current
            .panes
            .push(proc_fallback_pane(42, "claude", "claude"));

        let (should_publish, reset_reconcile_timer) =
            super::run_reconcile_refresh_publish_decision(&previous, &current);

        assert!(should_publish);
        assert!(reset_reconcile_timer);
    }

    #[test]
    fn daemon_control_event_timer_reset_tracks_broker_recovery_and_fallback() {
        assert!(super::run_control_event_refresh_should_reset_reconcile_timer(true, true, true));
        assert!(super::run_control_event_refresh_should_reset_reconcile_timer(true, false, false));
        assert!(
            !super::run_control_event_refresh_should_reset_reconcile_timer(false, false, false)
        );
        assert!(!super::run_control_event_refresh_should_reset_reconcile_timer(true, false, true));
    }

    #[test]
    fn daemon_control_exit_event_skips_broker_recovery() {
        assert!(super::run_control_event_should_recover_broker(false));
        assert!(!super::run_control_event_should_recover_broker(true));
    }

    #[test]
    fn daemon_reconcile_interval_uses_fallback_when_broker_is_disabled() {
        // Broker fallback has no event stream, so the reconcile poll is the sole
        // update path and stays fast regardless of `disable_reconcile`.
        assert_eq!(
            super::run_reconcile_interval_for(false, false, true),
            std::time::Duration::from_secs(1)
        );
        assert_eq!(
            super::run_reconcile_interval_for(false, true, true),
            std::time::Duration::from_secs(1)
        );
    }

    #[test]
    fn daemon_reconcile_interval_uses_self_heal_when_reconcile_disabled() {
        // Broker active + reconcile disabled: all sessions are event-driven via
        // per-session subscriber clients, so the poll is reduced to the infrequent
        // self-heal/drift backstop cadence.
        assert_eq!(
            super::run_reconcile_interval_for(true, true, true),
            std::time::Duration::from_secs(300)
        );
        // Broker active + reconcile enabled keeps the full redundancy interval.
        assert_eq!(
            super::run_reconcile_interval_for(true, false, true),
            std::time::Duration::from_secs(30)
        );
    }

    #[test]
    fn daemon_settle_deadline_arms_once_and_is_not_pushed_by_unrelated_activity() {
        use std::time::Duration;
        let now = std::time::Instant::now();
        let delay = Duration::from_millis(2200);

        // No busy pane-output pane: never armed (and cleared if previously set).
        assert_eq!(
            super::run_next_settle_deadline(false, None, now, delay),
            None
        );
        assert_eq!(
            super::run_next_settle_deadline(false, Some(now + delay), now, delay),
            None
        );

        // First busy observation arms the deadline. This is also the boot path: `run` calls
        // `update_settle_deadline` once at startup, so a pane already busy in the initial snapshot
        // arms the re-check even if no control event ever follows.
        assert_eq!(
            super::run_next_settle_deadline(true, None, now, delay),
            Some(now + delay)
        );

        // Already armed: a later refresh (e.g. another pane streaming) must NOT push the
        // deadline out, or the busy->idle re-check would be starved and never fire.
        let armed_at = now + delay;
        let later = now + Duration::from_millis(1000);
        assert_eq!(
            super::run_next_settle_deadline(true, Some(armed_at), later, delay),
            Some(armed_at)
        );
    }

    #[test]
    fn daemon_subscriber_coverage_requires_every_desired_session_attached() {
        let desired = vec!["$0".to_string(), "$1".to_string(), "$2".to_string()];

        // Under the cap and all desired sessions attached: coverage is complete.
        assert!(super::run_subscriber_coverage_complete(
            true, &desired, &desired
        ));
        // A failed attach (one desired session missing a subscriber) is incomplete,
        // even though the count is under the cap, so the poll stays active.
        assert!(!super::run_subscriber_coverage_complete(
            true,
            &desired,
            &["$0".to_string(), "$2".to_string()],
        ));
        // Over the cap is always incomplete regardless of attachments.
        assert!(!super::run_subscriber_coverage_complete(
            false, &desired, &desired
        ));
        // No desired sessions is vacuously complete.
        assert!(super::run_subscriber_coverage_complete(true, &[], &[]));
    }

    #[test]
    fn daemon_reconcile_interval_stays_active_when_subscriber_coverage_is_incomplete() {
        // More sessions than the subscriber cap means some sessions have no event
        // client, so even with reconcile "disabled" the poll must stay at the active
        // interval to cover them rather than relaxing to the 300s self-heal backstop.
        assert_eq!(
            super::run_reconcile_interval_for(true, true, false),
            std::time::Duration::from_secs(30)
        );
        // Broker fallback still wins: no event stream means the fast poll regardless.
        assert_eq!(
            super::run_reconcile_interval_for(false, true, false),
            std::time::Duration::from_secs(1)
        );
    }

    #[test]
    fn daemon_control_mode_wait_wakes_for_subscriber_monitor_before_reconcile() {
        let wait = super::run_next_control_mode_wait_for(
            std::time::Duration::from_secs(300),
            Some(std::time::Duration::from_millis(250)),
            None,
        );

        assert_eq!(wait, std::time::Duration::from_millis(250));
    }

    #[test]
    fn daemon_control_mode_wait_does_not_arm_subscriber_monitor_without_subscribers() {
        let wait =
            super::run_next_control_mode_wait_for(std::time::Duration::from_secs(300), None, None);

        // With no near deadline the wait falls back to the idle cap (CONTROL_MODE_MAX_WAIT).
        assert_eq!(wait, std::time::Duration::from_secs(2));
    }

    #[test]
    fn daemon_subscriber_session_ids_pass_through_under_the_cap() {
        // At or under the cap the set is returned unchanged (and un-reordered), so
        // existing subscriber clients are never churned by reconcile.
        let session_ids: Vec<String> = (0..daemon::MAX_CONTROL_MODE_SUBSCRIBERS)
            .map(|index| format!("${index}"))
            .collect();
        assert_eq!(
            super::run_capped_subscriber_session_ids(session_ids.clone()),
            session_ids
        );
    }

    #[test]
    fn daemon_subscriber_session_ids_capped_to_lowest_numeric_ids_over_the_cap() {
        // Use real, unpadded tmux ids in shuffled order. The cap must keep the lowest
        // numeric session indices, not a lexical prefix (where `$2` sorts after
        // `$19`), and the result must be deterministic across reconciles.
        let over = daemon::MAX_CONTROL_MODE_SUBSCRIBERS + 10;
        let mut session_ids: Vec<String> = (0..over).map(|index| format!("${index}")).collect();
        session_ids.reverse();

        let capped = super::run_capped_subscriber_session_ids(session_ids);
        assert_eq!(capped.len(), daemon::MAX_CONTROL_MODE_SUBSCRIBERS);

        // Expect exactly the lowest-numbered ids, numerically ordered. A lexical sort
        // would instead have kept ids like `$10`..`$19` ahead of `$2`..`$9` and
        // dropped some low indices, so this also guards against regressing the sort.
        let expected: Vec<String> = (0..daemon::MAX_CONTROL_MODE_SUBSCRIBERS)
            .map(|index| format!("${index}"))
            .collect();
        assert_eq!(capped, expected);
        // The highest-numbered sessions are the ones dropped.
        assert!(!capped.contains(&format!("${over}")));

        // Capping is idempotent: re-capping the already-capped set is a no-op.
        assert_eq!(
            super::run_capped_subscriber_session_ids(capped.clone()),
            capped
        );
    }
}
