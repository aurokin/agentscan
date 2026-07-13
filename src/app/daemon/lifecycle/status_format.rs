use super::*;

pub(super) fn print_lifecycle_not_running(
    out: &mut String,
    socket_path: &Path,
    paths: &LifecyclePaths,
    reason: &str,
) {
    let _ = writeln!(out, "daemon_state: not_running");
    let _ = writeln!(out, "socket_path: {}", socket_path.display());
    let _ = writeln!(out, "lock_path: {}", paths.lock_path.display());
    let _ = writeln!(out, "start_lock_path: {}", paths.start_lock_path.display());
    let _ = writeln!(out, "log_path: {}", paths.log_path.display());
    let _ = writeln!(out, "event_log_path: {}", paths.event_log_path.display());
    let _ = writeln!(out, "reason: {reason}");
}

// Serde emits fields in declaration order and always emits `Option::None` as an
// explicit `null` (no `skip_serializing_if`), so `daemon status --format json`
// carries every key for both running and not-running daemons. That is a machine
// contract pinned by `json_shape_tests`; keep the field order and null behavior
// stable. `Default` (all-`None` / empty strings) is what makes the not-running
// constructor a thin override instead of a hand-maintained all-`None` mirror.
#[derive(Default, Serialize)]
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
    control_mode_broker_primary_session_id: Option<String>,
    control_mode_broker_subscriber_coverage_complete: Option<bool>,
    control_mode_broker_desired_subscriber_count: Option<usize>,
    control_mode_broker_active_subscriber_count: Option<usize>,
    control_mode_broker_missing_subscriber_session_ids: Option<Vec<String>>,
    control_mode_broker_dead_subscriber_count: Option<usize>,
    control_mode_broker_subscribers: Option<Vec<ipc::ControlModeSubscriberStatusFrame>>,
    control_mode_broker_last_subscriber_reconcile_at: Option<String>,
    control_mode_broker_next_subscriber_monitor_in_ms: Option<u64>,
    control_mode_broker_next_reconcile_in_ms: Option<u64>,
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
    subscriber_monitor_count: Option<u64>,
    subscriber_start_count: Option<u64>,
    subscriber_reattach_count: Option<u64>,
    subscriber_attach_failure_count: Option<u64>,
    subscriber_exit_count: Option<u64>,
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
    // Every telemetry/broker/identity field defaults to `None` (serialized as an
    // explicit `null`); only the locally-known paths, the `not_running` state,
    // the reason, and the events opt-in differ from `Default`.
    DaemonStatusJson {
        daemon_state: "not_running".to_string(),
        socket_path: socket_path.display().to_string(),
        lock_path: paths.lock_path.display().to_string(),
        start_lock_path: paths.start_lock_path.display().to_string(),
        log_path: paths.log_path.display().to_string(),
        event_log_path: paths.event_log_path.display().to_string(),
        reason: Some(reason.to_string()),
        recent_events: include_events.then(Vec::new),
        ..Default::default()
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
        control_mode_broker_primary_session_id: status
            .control_mode_broker
            .as_ref()
            .and_then(|broker| broker.primary_session_id.clone()),
        control_mode_broker_subscriber_coverage_complete: status
            .control_mode_broker
            .as_ref()
            .and_then(|broker| broker.subscriber_coverage_complete),
        control_mode_broker_desired_subscriber_count: status
            .control_mode_broker
            .as_ref()
            .and_then(|broker| broker.desired_subscriber_count),
        control_mode_broker_active_subscriber_count: status
            .control_mode_broker
            .as_ref()
            .and_then(|broker| broker.active_subscriber_count),
        control_mode_broker_missing_subscriber_session_ids: status
            .control_mode_broker
            .as_ref()
            .and_then(|broker| broker.missing_subscriber_session_ids.clone()),
        control_mode_broker_dead_subscriber_count: status
            .control_mode_broker
            .as_ref()
            .and_then(|broker| broker.dead_subscriber_count),
        control_mode_broker_subscribers: status
            .control_mode_broker
            .as_ref()
            .and_then(|broker| broker.subscribers.clone()),
        control_mode_broker_last_subscriber_reconcile_at: status
            .control_mode_broker
            .as_ref()
            .and_then(|broker| broker.last_subscriber_reconcile_at.clone()),
        control_mode_broker_next_subscriber_monitor_in_ms: status
            .control_mode_broker
            .as_ref()
            .and_then(|broker| broker.next_subscriber_monitor_in_ms),
        control_mode_broker_next_reconcile_in_ms: status
            .control_mode_broker
            .as_ref()
            .and_then(|broker| broker.next_reconcile_in_ms),
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
        subscriber_monitor_count: status
            .runtime_telemetry
            .as_ref()
            .and_then(|telemetry| telemetry.subscriber_monitor_count),
        subscriber_start_count: status
            .runtime_telemetry
            .as_ref()
            .and_then(|telemetry| telemetry.subscriber_start_count),
        subscriber_reattach_count: status
            .runtime_telemetry
            .as_ref()
            .and_then(|telemetry| telemetry.subscriber_reattach_count),
        subscriber_attach_failure_count: status
            .runtime_telemetry
            .as_ref()
            .and_then(|telemetry| telemetry.subscriber_attach_failure_count),
        subscriber_exit_count: status
            .runtime_telemetry
            .as_ref()
            .and_then(|telemetry| telemetry.subscriber_exit_count),
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

pub(super) fn emit_lifecycle_not_running(
    socket_path: &Path,
    paths: &LifecyclePaths,
    reason: &str,
    format: OutputFormat,
    include_events: bool,
) -> Result<()> {
    match format {
        OutputFormat::Text => {
            let mut out = String::new();
            print_lifecycle_not_running(&mut out, socket_path, paths, reason);
            output::write_stdout(&out)
        }
        OutputFormat::Json => output::print_json(&lifecycle_not_running_json(
            socket_path,
            paths,
            reason,
            include_events,
        )),
    }
}

pub(super) fn emit_lifecycle_status(
    paths: &LifecyclePaths,
    status: &ipc::LifecycleStatusFrame,
    format: OutputFormat,
    include_events: bool,
) -> Result<()> {
    match format {
        OutputFormat::Text => {
            let mut out = String::new();
            print_lifecycle_status(&mut out, paths, status);
            if include_events {
                print_recent_observability_events(&mut out, &status.recent_events);
            }
            output::write_stdout(&out)
        }
        OutputFormat::Json => {
            output::print_json(&lifecycle_status_json(paths, status, include_events))
        }
    }
}

pub(super) fn incompatible_daemon_guidance(message: &str) -> String {
    format!(
        "{message}; stop the incompatible daemon manually, remove the socket only if it is stale, then run `agentscan daemon start`"
    )
}

pub(super) fn lifecycle_state_label(state: ipc::LifecycleDaemonState) -> &'static str {
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

pub(super) fn print_lifecycle_status(
    out: &mut String,
    paths: &LifecyclePaths,
    status: &ipc::LifecycleStatusFrame,
) {
    let _ = writeln!(out, "daemon_state: {}", lifecycle_state_label(status.state));
    let _ = writeln!(out, "socket_path: {}", status.identity.socket_path);
    let _ = writeln!(out, "lock_path: {}", paths.lock_path.display());
    let _ = writeln!(out, "start_lock_path: {}", paths.start_lock_path.display());
    let _ = writeln!(out, "log_path: {}", paths.log_path.display());
    let _ = writeln!(out, "event_log_path: {}", paths.event_log_path.display());
    let _ = writeln!(out, "pid: {}", status.identity.pid);
    let _ = writeln!(
        out,
        "daemon_start_time: {}",
        status.identity.daemon_start_time
    );
    let _ = writeln!(out, "executable: {}", status.identity.executable);
    if let Some(executable) = &status.identity.executable_canonical {
        let _ = writeln!(out, "executable_canonical: {executable}");
    }
    let _ = writeln!(
        out,
        "protocol_version: {}",
        status.identity.protocol_version
    );
    let _ = writeln!(
        out,
        "snapshot_schema_version: {}",
        status.identity.snapshot_schema_version
    );
    let _ = writeln!(out, "subscriber_count: {}", status.subscriber_count);
    if let Some(generated_at) = &status.latest_snapshot_generated_at {
        let _ = writeln!(out, "latest_snapshot_generated_at: {generated_at}");
    }
    if let Some(pane_count) = status.latest_snapshot_pane_count {
        let _ = writeln!(out, "latest_snapshot_pane_count: {pane_count}");
    }
    if let Some(source) = &status.latest_snapshot_update_source {
        let _ = writeln!(out, "latest_snapshot_update_source: {source}");
    }
    if let Some(detail) = &status.latest_snapshot_update_detail {
        let _ = writeln!(out, "latest_snapshot_update_detail: {detail}");
    }
    if let Some(duration_ms) = status.latest_snapshot_update_duration_ms {
        let _ = writeln!(out, "latest_snapshot_update_duration_ms: {duration_ms}");
    }
    if let Some(broker) = &status.control_mode_broker {
        let _ = writeln!(
            out,
            "control_mode_broker_mode: {}",
            control_mode_broker_mode_label(broker.mode)
        );
        let _ = writeln!(
            out,
            "control_mode_broker_reconnect_count: {}",
            broker.reconnect_count
        );
        if let Some(fallback_count) = broker.fallback_count {
            let _ = writeln!(out, "control_mode_broker_fallback_count: {fallback_count}");
        } else {
            let _ = writeln!(out, "control_mode_broker_fallback_count: unavailable");
        }
        if let Some(subscriber_count) = broker.subscriber_count {
            let _ = writeln!(
                out,
                "control_mode_broker_subscriber_count: {subscriber_count}"
            );
        } else {
            let _ = writeln!(out, "control_mode_broker_subscriber_count: unavailable");
        }
        if let Some(primary_session_id) = &broker.primary_session_id {
            let _ = writeln!(
                out,
                "control_mode_broker_primary_session_id: {primary_session_id}"
            );
        }
        if let Some(coverage_complete) = broker.subscriber_coverage_complete {
            let _ = writeln!(
                out,
                "control_mode_broker_subscriber_coverage_complete: {coverage_complete}"
            );
        }
        if let Some(desired_count) = broker.desired_subscriber_count {
            let _ = writeln!(
                out,
                "control_mode_broker_desired_subscriber_count: {desired_count}"
            );
        }
        if let Some(active_count) = broker.active_subscriber_count {
            let _ = writeln!(
                out,
                "control_mode_broker_active_subscriber_count: {active_count}"
            );
        }
        if let Some(missing_session_ids) = &broker.missing_subscriber_session_ids
            && !missing_session_ids.is_empty()
        {
            let _ = writeln!(
                out,
                "control_mode_broker_missing_subscriber_session_ids: {}",
                missing_session_ids.join(",")
            );
        }
        if let Some(dead_count) = broker.dead_subscriber_count {
            let _ = writeln!(
                out,
                "control_mode_broker_dead_subscriber_count: {dead_count}"
            );
        }
        if let Some(last_reconcile_at) = &broker.last_subscriber_reconcile_at {
            let _ = writeln!(
                out,
                "control_mode_broker_last_subscriber_reconcile_at: {last_reconcile_at}"
            );
        }
        if let Some(next_monitor_ms) = broker.next_subscriber_monitor_in_ms {
            let _ = writeln!(
                out,
                "control_mode_broker_next_subscriber_monitor_in_ms: {next_monitor_ms}"
            );
        }
        if let Some(next_reconcile_ms) = broker.next_reconcile_in_ms {
            let _ = writeln!(
                out,
                "control_mode_broker_next_reconcile_in_ms: {next_reconcile_ms}"
            );
        }
        if let Some(subscribers) = &broker.subscribers {
            for subscriber in subscribers {
                let _ = writeln!(
                    out,
                    "control_mode_broker_subscriber: session_id={} pid={} restart_count={} dead={} last_line_at={} last_event_at={}",
                    subscriber.session_id,
                    subscriber.pid,
                    subscriber.restart_count,
                    subscriber.dead,
                    subscriber.last_line_at.as_deref().unwrap_or("never"),
                    subscriber.last_event_at.as_deref().unwrap_or("never"),
                );
            }
        }
        if let Some(reason) = &broker.disabled_reason {
            let _ = writeln!(out, "control_mode_broker_disabled_reason: {reason}");
        }
    }
    print_runtime_telemetry(out, status.runtime_telemetry.as_ref());
    print_snapshot_observability(out, status.latest_snapshot_observability.as_ref());
    if let Some(reason) = status.unavailable_reason {
        let _ = writeln!(
            out,
            "unavailable_reason: {}",
            unavailable_reason_label(reason)
        );
    }
    if let Some(message) = &status.message {
        let _ = writeln!(out, "message: {message}");
    }
}

fn print_runtime_telemetry(out: &mut String, telemetry: Option<&ipc::RuntimeTelemetryFrame>) {
    let Some(telemetry) = telemetry else {
        let _ = writeln!(out, "runtime_telemetry: unavailable");
        return;
    };

    let _ = writeln!(
        out,
        "control_event_refresh_count: {}",
        telemetry.control_event_refresh_count
    );
    let _ = writeln!(
        out,
        "control_event_batch_count: {}",
        telemetry.control_event_batch_count
    );
    let _ = writeln!(
        out,
        "control_event_line_count: {}",
        telemetry.control_event_line_count
    );
    let _ = writeln!(
        out,
        "control_event_output_line_count: {}",
        telemetry.control_event_output_line_count
    );
    let _ = writeln!(
        out,
        "control_event_output_byte_count: {}",
        telemetry.control_event_output_byte_count
    );
    let _ = writeln!(
        out,
        "control_event_pane_count: {}",
        telemetry.control_event_pane_count
    );
    let _ = writeln!(
        out,
        "control_event_title_count: {}",
        telemetry.control_event_title_count
    );
    let _ = writeln!(
        out,
        "control_event_window_count: {}",
        telemetry.control_event_window_count
    );
    let _ = writeln!(
        out,
        "control_event_session_count: {}",
        telemetry.control_event_session_count
    );
    let _ = writeln!(
        out,
        "control_event_resnapshot_count: {}",
        telemetry.control_event_resnapshot_count
    );
    let _ = writeln!(
        out,
        "control_event_ignored_count: {}",
        telemetry.control_event_ignored_count
    );
    let _ = writeln!(
        out,
        "reconcile_attempt_count: {}",
        telemetry.reconcile_attempt_count
    );
    let _ = writeln!(
        out,
        "reconcile_noop_count: {}",
        telemetry.reconcile_noop_count
    );
    let _ = writeln!(
        out,
        "reconcile_changed_snapshot_count: {}",
        telemetry.reconcile_changed_snapshot_count
    );
    let _ = writeln!(
        out,
        "targeted_title_update_count: {}",
        telemetry.targeted_title_update_count
    );
    let _ = writeln!(
        out,
        "targeted_pane_refresh_count: {}",
        telemetry.targeted_pane_refresh_count
    );
    let _ = writeln!(
        out,
        "targeted_scope_refresh_count: {}",
        telemetry.targeted_scope_refresh_count
    );
    let _ = writeln!(
        out,
        "full_snapshot_refresh_count: {}",
        telemetry.full_snapshot_refresh_count
    );
    let _ = writeln!(
        out,
        "targeted_refresh_fallback_to_full_count: {}",
        telemetry.targeted_refresh_fallback_to_full_count
    );
    print_optional_counter(
        out,
        "subscriber_monitor_count",
        telemetry.subscriber_monitor_count,
    );
    print_optional_counter(
        out,
        "subscriber_start_count",
        telemetry.subscriber_start_count,
    );
    print_optional_counter(
        out,
        "subscriber_reattach_count",
        telemetry.subscriber_reattach_count,
    );
    print_optional_counter(
        out,
        "subscriber_attach_failure_count",
        telemetry.subscriber_attach_failure_count,
    );
    print_optional_counter(
        out,
        "subscriber_exit_count",
        telemetry.subscriber_exit_count,
    );
    let _ = writeln!(
        out,
        "broker_fallback_count: {}",
        telemetry.broker_fallback_count
    );
    let _ = writeln!(
        out,
        "pane_output_capture_attempt_count: {}",
        telemetry.pane_output_capture_attempt_count
    );
    let _ = writeln!(
        out,
        "pane_output_capture_hit_count: {}",
        telemetry.pane_output_capture_hit_count
    );
    let _ = writeln!(
        out,
        "pane_output_capture_error_count: {}",
        telemetry.pane_output_capture_error_count
    );
}

fn print_optional_counter(out: &mut String, label: &str, value: Option<u64>) {
    match value {
        Some(value) => {
            let _ = writeln!(out, "{label}: {value}");
        }
        None => {
            let _ = writeln!(out, "{label}: unavailable");
        }
    }
}

fn print_snapshot_observability(
    out: &mut String,
    observability: Option<&ipc::SnapshotObservabilityFrame>,
) {
    let Some(observability) = observability else {
        return;
    };

    let _ = writeln!(
        out,
        "latest_snapshot_provider_known_count: {}",
        observability.provider_known_count
    );
    let _ = writeln!(
        out,
        "latest_snapshot_provider_unknown_count: {}",
        observability.provider_unknown_count
    );
    let _ = writeln!(
        out,
        "latest_snapshot_status_source_pane_metadata_count: {}",
        observability.status_source_pane_metadata_count
    );
    let _ = writeln!(
        out,
        "latest_snapshot_status_source_tmux_title_count: {}",
        observability.status_source_tmux_title_count
    );
    let _ = writeln!(
        out,
        "latest_snapshot_status_source_pane_output_count: {}",
        observability.status_source_pane_output_count
    );
    let _ = writeln!(
        out,
        "latest_snapshot_status_source_not_checked_count: {}",
        observability.status_source_not_checked_count
    );
    let _ = writeln!(
        out,
        "latest_snapshot_proc_fallback_not_run_count: {}",
        observability.proc_fallback_not_run_count
    );
    let _ = writeln!(
        out,
        "latest_snapshot_proc_fallback_skipped_count: {}",
        observability.proc_fallback_skipped_count
    );
    let _ = writeln!(
        out,
        "latest_snapshot_proc_fallback_no_match_count: {}",
        observability.proc_fallback_no_match_count
    );
    let _ = writeln!(
        out,
        "latest_snapshot_proc_fallback_error_count: {}",
        observability.proc_fallback_error_count
    );
    let _ = writeln!(
        out,
        "latest_snapshot_proc_fallback_resolved_count: {}",
        observability.proc_fallback_resolved_count
    );
    for (provider, stats) in &observability.per_provider {
        let _ = writeln!(
            out,
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

fn print_recent_observability_events(
    out: &mut String,
    events: &[ipc::DaemonObservabilityEventFrame],
) {
    let _ = writeln!(out, "recent_events:");
    if events.is_empty() {
        let _ = writeln!(out, "  <empty>");
        return;
    }
    for event in events {
        let _ = writeln!(
            out,
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

#[cfg(test)]
mod json_shape_tests {
    use super::*;

    fn sample_paths() -> LifecyclePaths {
        LifecyclePaths::from_socket_path(Path::new("/run/agentscan/agentscan.sock"))
    }

    fn sample_status_frame() -> ipc::LifecycleStatusFrame {
        ipc::LifecycleStatusFrame {
            state: ipc::LifecycleDaemonState::Ready,
            identity: ipc::DaemonIdentityFrame {
                pid: 4242,
                daemon_start_time: "2026-07-13T15:28:27.347477Z".to_string(),
                executable: "/usr/local/bin/agentscan".to_string(),
                executable_canonical: Some("/usr/local/bin/agentscan".to_string()),
                socket_path: "/run/agentscan/agentscan.sock".to_string(),
                protocol_version: 1,
                snapshot_schema_version: 5,
            },
            subscriber_count: 3,
            latest_snapshot_generated_at: Some("2026-07-13T22:37:48.177483Z".to_string()),
            latest_snapshot_pane_count: Some(22),
            latest_snapshot_update_source: Some("control_event".to_string()),
            latest_snapshot_update_detail: Some("batch".to_string()),
            latest_snapshot_update_duration_ms: Some(2),
            control_mode_broker: Some(ipc::ControlModeBrokerStatusFrame {
                mode: ipc::ControlModeBrokerMode::Active,
                // `None` here locks the "emit explicit null" behavior for the
                // broker sub-fields that are absent on the wire.
                disabled_reason: None,
                reconnect_count: 0,
                fallback_count: Some(0),
                subscriber_count: Some(5),
                primary_session_id: Some("$1".to_string()),
                subscriber_coverage_complete: Some(true),
                desired_subscriber_count: Some(5),
                active_subscriber_count: Some(5),
                missing_subscriber_session_ids: Some(Vec::new()),
                dead_subscriber_count: Some(0),
                subscribers: Some(vec![ipc::ControlModeSubscriberStatusFrame {
                    session_id: "$0".to_string(),
                    pid: 9397,
                    started_at: "2026-07-13T15:28:27.391299Z".to_string(),
                    last_line_at: Some("2026-07-13T22:27:11.083411Z".to_string()),
                    last_event_at: None,
                    restart_count: 0,
                    dead: false,
                }]),
                last_subscriber_reconcile_at: Some("2026-07-13T22:33:30.421384Z".to_string()),
                next_subscriber_monitor_in_ms: Some(249),
                next_reconcile_in_ms: Some(41921),
            }),
            runtime_telemetry: Some(ipc::RuntimeTelemetryFrame {
                control_event_refresh_count: 40255,
                control_event_batch_count: 40295,
                control_event_line_count: 74739,
                control_event_output_line_count: 0,
                control_event_output_byte_count: 0,
                control_event_pane_count: 74127,
                control_event_title_count: 0,
                control_event_window_count: 19,
                control_event_session_count: 0,
                control_event_resnapshot_count: 37,
                control_event_ignored_count: 387,
                reconcile_attempt_count: 161,
                reconcile_noop_count: 1,
                reconcile_changed_snapshot_count: 160,
                targeted_title_update_count: 0,
                targeted_pane_refresh_count: 74126,
                targeted_scope_refresh_count: 18,
                full_snapshot_refresh_count: 37,
                targeted_refresh_fallback_to_full_count: 0,
                subscriber_monitor_count: Some(98565),
                subscriber_start_count: Some(5),
                // `None` locks the "emit explicit null" behavior for optional
                // telemetry counters that are present-but-unset.
                subscriber_reattach_count: None,
                subscriber_attach_failure_count: Some(0),
                subscriber_exit_count: Some(0),
                broker_fallback_count: 0,
                pane_output_capture_attempt_count: 11,
                pane_output_capture_hit_count: 5,
                pane_output_capture_error_count: 0,
            }),
            latest_snapshot_observability: Some(ipc::SnapshotObservabilityFrame {
                provider_known_count: 4,
                provider_unknown_count: 18,
                status_source_pane_metadata_count: 0,
                status_source_tmux_title_count: 4,
                status_source_pane_output_count: 0,
                status_source_not_checked_count: 18,
                proc_fallback_not_run_count: 1,
                proc_fallback_skipped_count: 5,
                proc_fallback_no_match_count: 13,
                proc_fallback_error_count: 0,
                proc_fallback_resolved_count: 3,
                per_provider: std::collections::BTreeMap::new(),
            }),
            recent_events: vec![ipc::DaemonObservabilityEventFrame {
                at: "2026-07-13T22:37:48.177483Z".to_string(),
                source: "control_event".to_string(),
                detail: Some("batch".to_string()),
                refresh: "targeted".to_string(),
                control_sources: Vec::new(),
                control_lines: Vec::new(),
                changed: true,
                published: true,
                duration_ms: Some(2),
                diff: None,
            }],
            unavailable_reason: None,
            message: None,
        }
    }

    // Byte-for-byte golden of `daemon status --format json` for a running
    // daemon. `daemon status` output is a stable machine contract; any change
    // to field names, order, or null presence/absence must be intentional.
    #[test]
    fn running_status_json_is_byte_stable() {
        let paths = sample_paths();
        let status = sample_status_frame();
        let json =
            serde_json::to_string_pretty(&lifecycle_status_json(&paths, &status, true)).unwrap();
        let expected = include_str!("testdata/daemon_status_running.json").trim_end();
        assert_eq!(json, expected);
    }

    // Byte-for-byte golden of `daemon status --format json` for a not-running
    // daemon: every frame-derived key must still be present as an explicit
    // `null`.
    #[test]
    fn not_running_status_json_is_byte_stable() {
        let paths = sample_paths();
        let json = serde_json::to_string_pretty(&lifecycle_not_running_json(
            Path::new("/run/agentscan/agentscan.sock"),
            &paths,
            "daemon socket not found",
            true,
        ))
        .unwrap();
        let expected = include_str!("testdata/daemon_status_not_running.json").trim_end();
        assert_eq!(json, expected);
    }
}
