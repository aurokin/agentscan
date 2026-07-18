use std::{
    collections::HashMap,
    io::{BufRead, BufReader},
    process::{Child, Stdio},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::{
    contract::{PickerRow, validate_picker_rows},
    runner::{
        AgentscanRunner, LIVE_CHILD_EXIT_GRACE, agentscan_command, classify_desktop_failure,
        collect_pipe, load_daemon_status, spawn_pipe_collector,
    },
};

const LIVE_PICKER_EVENT: &str = "agentscan-live-picker";
// One live supervisor per source key (the frontend's runnerKey), so multiple
// sources can stream concurrently; starting/stopping a key never disturbs the
// other keys' workers.
static LIVE_PICKER: OnceLock<Mutex<HashMap<String, LivePickerSupervisor>>> = OnceLock::new();
// Serializes whole start operations (stop + spawn + install) so overlapping
// starts cannot interleave and leave a newer start silently no-op'ing while an
// older one wins the install race. The guarded fence holds, per source key, the
// highest subscription epoch we have honored with a start: the frontend issues
// strictly-increasing epochs (persisted across reload/HMR), so a late start()
// from a torn-down page carries a lower epoch and is rejected — keeping a stale
// start from replacing the live page's worker for that key.
static LIVE_PICKER_START: OnceLock<Mutex<StartFence>> = OnceLock::new();

// Source keys derive from the full runner settings, so every host/binary/env edit
// mints a fresh key — but a key's fence entry must outlive its stop (deleting on
// stop would let a stale start install a zombie worker). Bound the map instead:
// past the cap, evict the lowest-epoch entry and raise `floor` to it. Epochs are
// globally monotonic across keys (one frontend counter), so anything at or below
// the floor is globally stale and rejected without needing its per-key entry.
const LIVE_PICKER_FENCE_CAP: usize = 64;

#[derive(Default)]
struct StartFence {
    last_started: HashMap<String, u64>,
    floor: u64,
}

#[derive(Debug)]
struct LivePickerSupervisor {
    epoch: u64,
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    worker: Option<JoinHandle<()>>,
}

// Frames the desktop consumes from `agentscan subscribe --format json`. The
// contract is intentionally **tolerant of additive frame types**: a frame whose
// `type` is not one of the known variants deserializes to `Unknown` and is ignored
// (AUR-457), so a newer daemon can introduce frame types without breaking the live
// view on an older desktop build. A *known* type with a malformed payload, or a
// line that isn't valid JSON, is still a real protocol error and tears the
// subscription down — only brand-new `type` strings are absorbed.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SubscribeFrame {
    Connecting {
        message: String,
    },
    // The host builds the picker `rows` on the tmux-owning side and ships them in
    // this frame, so the desktop renders directly from the delivered snapshot
    // instead of spawning a second `agentscan hotkeys` scan (a full extra tmux
    // scan + SSH round-trip) per update. A `snapshot` frame missing `rows` is a
    // known-type-with-bad-payload protocol error (host too old / mismatched),
    // which tears the subscription down to reconnect rather than silently
    // rendering an empty picker.
    Snapshot {
        snapshot: serde_json::Value,
        rows: Vec<PickerRow>,
    },
    Offline {
        message: String,
        retrying: bool,
    },
    Shutdown {
        message: String,
    },
    Fatal {
        message: String,
    },
    // Idle heartbeat the daemon emits ~1/s so it can detect a closed consumer
    // while the stream is otherwise silent. It carries no state, so the live
    // worker ignores it (without it, every heartbeat would fail to parse and
    // tear down the subscription with a spurious "Offline, retrying").
    Keepalive,
    // Any unrecognized `type`. `#[serde(other)]` matches on the tag alone and
    // discards the payload, so forward/unknown frames are a no-op instead of a
    // parse error. This generalizes the Keepalive fix to the whole class: we no
    // longer need a dedicated variant per future frame type.
    #[serde(other)]
    Unknown,
}

// Wraps every emitted event with the source key and epoch of the subscription
// that produced it. The live event channel is global and shared by all keyed
// workers, so the frontend routes each frame to its source by `source_key`; a
// late frame from a superseded worker (e.g. after a re-arm) still carries the
// old epoch and is dropped by the per-key epoch comparison.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LivePickerEnvelope {
    source_key: String,
    epoch: u64,
    #[serde(flatten)]
    event: LivePickerEvent,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum LivePickerEvent {
    Connecting {
        message: String,
    },
    Rows {
        rows: Vec<PickerRow>,
        snapshot: LiveSnapshotSummary,
    },
    Offline {
        message: String,
        retrying: bool,
        diagnostics: Option<serde_json::Value>,
    },
    Shutdown {
        message: String,
    },
    Fatal {
        message: String,
        diagnostics: Option<serde_json::Value>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LiveSnapshotSummary {
    pane_count: usize,
    generated_at: Option<String>,
    source_kind: Option<String>,
}

// Per-key stale-start gate: honor a start only when its epoch advances past the
// highest epoch already honored for that source key — and past the fence floor,
// which stands in for evicted keys. Keys gate independently, so one source's
// stale start can never block — or tear down — another's worker.
//
// The floor fallback is gate-equivalent for evicted keys, never weaker: a worker
// running at epoch E means E was committed as that key's entry (per-key entries
// are monotone), and an absent entry means it was evicted — eviction takes only
// the map minimum and raises the floor to at least that value, so floor >= E
// whenever a running key lacks an entry. `epoch > floor` then admits exactly the
// strictly-newer starts the entry would have admitted; no superseded start can
// slip between the floor and a running worker (see commit_start_epoch).
fn epoch_advances(fence: &StartFence, source_key: &str, epoch: u64) -> bool {
    epoch > fence.floor
        && fence
            .last_started
            .get(source_key)
            .is_none_or(|last| epoch > *last)
}

// Commit a honored start into the fence, evicting the lowest-epoch entry (and
// raising the floor to it) once past the cap so edits can't grow it unboundedly.
// Eviction never weakens the gate: only the MINIMUM entry is evicted and the
// floor rises to exactly that epoch, so for the evicted key `epoch > floor` is at
// least as strict as its `epoch > last` entry was (a worker started at epoch E is
// evicted only when E is the minimum, leaving floor >= E — the would-be window
// floor < S <= E is empty), and for every other key the floor only adds strictness.
fn commit_start_epoch(fence: &mut StartFence, source_key: String, epoch: u64) {
    fence.last_started.insert(source_key, epoch);
    if fence.last_started.len() > LIVE_PICKER_FENCE_CAP
        && let Some((evict_key, evict_epoch)) = fence
            .last_started
            .iter()
            .min_by_key(|(_, last)| **last)
            .map(|(key, last)| (key.clone(), *last))
    {
        fence.last_started.remove(&evict_key);
        fence.floor = fence.floor.max(evict_epoch);
    }
}

pub(crate) fn start_live_picker_with_runner(
    app: tauri::AppHandle,
    runner: AgentscanRunner,
    source_key: String,
    epoch: u64,
    auto_start: bool,
) -> Result<(), String> {
    // Hold the start lock across the whole stop+spawn+install so overlapping
    // starts can't interleave; the loser would otherwise see a supervisor
    // installed between our stop and re-lock and silently no-op.
    let mut fence = live_picker_start_lock()
        .lock()
        .map_err(|_| "live picker start lock poisoned".to_owned())?;

    // Ignore a stale start whose epoch does not advance past the last one we
    // honored for this key. Epochs increase strictly across reloads/HMR, so a
    // lower-or-equal epoch here means this start came from a torn-down page;
    // installing it would stop the live page's worker and the live page
    // (filtering on its own higher epoch) would then drop every frame. We only
    // *commit* the epoch after the worker is installed (below), so a failed
    // start does not advance the guard and silently reject the frontend's retry
    // of the same epoch.
    if !epoch_advances(&fence, &source_key, epoch) {
        return Ok(());
    }

    // Replace any running supervisor for this key so the requested subscription
    // (and its epoch) always starts; other keys' workers are untouched. stop
    // joins the old worker without holding the supervisor lock, so re-locking
    // below is safe and (under the start lock) no other start can install in
    // between.
    stop_live_picker_supervisor(&source_key)?;

    let mut supervisors = live_picker_supervisors()
        .lock()
        .map_err(|_| "live picker supervisor lock poisoned".to_owned())?;

    let stop = Arc::new(AtomicBool::new(false));
    let child = Arc::new(Mutex::new(None));
    let worker_stop = Arc::clone(&stop);
    let worker_child = Arc::clone(&child);
    let worker_key = source_key.clone();
    let worker = thread::Builder::new()
        .name("agentscan-live-picker".to_owned())
        .spawn(move || {
            run_live_picker_worker(
                app,
                runner,
                worker_key,
                worker_stop,
                worker_child,
                epoch,
                auto_start,
            )
        })
        .map_err(|error| format!("Unable to start live picker worker: {error}"))?;

    supervisors.insert(
        source_key.clone(),
        LivePickerSupervisor {
            epoch,
            stop,
            child,
            worker: Some(worker),
        },
    );
    drop(supervisors);

    // Commit the epoch only now that the worker is installed.
    commit_start_epoch(&mut fence, source_key, epoch);

    Ok(())
}

fn stop_live_picker_supervisor(source_key: &str) -> Result<(), String> {
    stop_live_picker_supervisor_for_epoch(source_key, None)
}

// Take and tear down the supervisor for one source key. When `target` is Some,
// only stop if that key's supervisor is running this epoch (used by the
// epoch-guarded command); when None, stop unconditionally (used by start to
// replace any prior worker for the key). The worker is joined after the lock
// guard is dropped to avoid deadlocking with the worker's own supervisor cleanup.
pub(crate) fn stop_live_picker_supervisor_for_epoch(
    source_key: &str,
    target: Option<u64>,
) -> Result<(), String> {
    let supervisor = {
        let mut guard = live_picker_supervisors()
            .lock()
            .map_err(|_| "live picker supervisor lock poisoned".to_owned())?;
        let matches = guard
            .get(source_key)
            .is_some_and(|current| target.is_none_or(|epoch| current.epoch == epoch));
        if matches {
            guard.remove(source_key)
        } else {
            None
        }
    };

    if let Some(mut supervisor) = supervisor {
        supervisor.stop.store(true, Ordering::SeqCst);
        kill_live_picker_child(&supervisor.child);

        if let Some(worker) = supervisor.worker.take() {
            let _ = worker.join();
        }
    }

    Ok(())
}

fn live_picker_supervisors() -> &'static Mutex<HashMap<String, LivePickerSupervisor>> {
    LIVE_PICKER.get_or_init(|| Mutex::new(HashMap::new()))
}

fn live_picker_start_lock() -> &'static Mutex<StartFence> {
    LIVE_PICKER_START.get_or_init(|| Mutex::new(StartFence::default()))
}

fn run_live_picker_worker(
    app: tauri::AppHandle,
    runner: AgentscanRunner,
    source_key: String,
    stop: Arc<AtomicBool>,
    child_slot: Arc<Mutex<Option<Child>>>,
    epoch: u64,
    auto_start: bool,
) {
    // Single-shot: one subscribe attempt, no in-worker retry loop. Reconnect is owned by
    // the layers that can see it. The `agentscan subscribe` CLI self-recovers mid-stream
    // transient drops in its own loop (frames keep streaming on the live child). On a clean
    // daemon loss the CLI emits a terminal frame (Shutdown / Offline retrying:false / Fatal);
    // on an abnormal subscribe-child death (spawn/IO/protocol failure) this worker emits a
    // terminal Offline{retrying:false}. Either way the TS LiveConnection service re-arms with
    // a FRESH epoch and autoStart=false (`first && target.autoStart`, LiveConnection.ts), so
    // the desktop's latch-only recovery holds without this worker advancing the epoch or
    // auto-starting on its own. The recoverable re-arm backoff (~1s) lives in TS, matching
    // the old in-worker LIVE_RECONNECT_DELAY. See AUR-517 and the latch-only ADR.
    //
    // No connecting/reconnecting frame is emitted here: LiveConnection sets that status
    // itself before invoking start_live_picker (connecting on the first attach, reconnecting
    // on a re-arm), and the `agentscan subscribe` CLI emits its own per-connect `connecting`
    // frame (forwarded in handle_subscribe_frame). An emit here would only duplicate the
    // former and be overwritten by the latter.
    run_live_picker_subscription(
        &app,
        &runner,
        &source_key,
        &stop,
        &child_slot,
        epoch,
        auto_start,
    );

    kill_live_picker_child(&child_slot);
    let _ = live_picker_supervisors().lock().map(|mut supervisors| {
        if supervisors
            .get(&source_key)
            .is_some_and(|current| Arc::ptr_eq(&current.stop, &stop))
        {
            supervisors.remove(&source_key);
        }
    });
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LivePickerWorkerExit {
    Retry,
    Shutdown,
    Fatal,
}

// Subscribe argv for the live worker. `--no-auto-start` is appended when the
// desktop wants to *latch* onto an already-running daemon without spawning one;
// only an explicit user "Start agentscan" requests auto-start (no flag).
fn subscribe_args(auto_start: bool) -> Vec<&'static str> {
    let mut args = vec!["subscribe", "--format", "json"];
    if !auto_start {
        args.push("--no-auto-start");
    }
    args
}

fn run_live_picker_subscription(
    app: &tauri::AppHandle,
    runner: &AgentscanRunner,
    source_key: &str,
    stop: &AtomicBool,
    child_slot: &Arc<Mutex<Option<Child>>>,
    epoch: u64,
    auto_start: bool,
) {
    // Single-shot per AUR-517: this runs ONE subscribe. Any abnormal end of the child
    // (spawn/IO/protocol failure, or a bare exit with no terminal frame) is reported as a
    // terminal Offline{retrying:false} so the TS LiveConnection service re-arms with a fresh
    // epoch (latch-only). Only frames that keep the live child streaming — the daemon's own
    // Offline{retrying:true} self-heal and a transient row-fetch miss, both in
    // handle_subscribe_frame — stay retrying:true.
    let mut command = match agentscan_command(runner, &subscribe_args(auto_start)) {
        Ok(command) => command,
        Err(error) => {
            let message = classify_desktop_failure(runner, "subscribe", &error);
            emit_live_picker_event(
                app,
                source_key,
                epoch,
                LivePickerEvent::Fatal {
                    message,
                    diagnostics: None,
                },
            );
            return;
        }
    };
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let message = classify_desktop_failure(
                runner,
                "subscribe",
                &format!("Unable to start agentscan subscribe: {error}"),
            );
            emit_live_picker_event(
                app,
                source_key,
                epoch,
                LivePickerEvent::Offline {
                    message,
                    retrying: false,
                    diagnostics: load_daemon_status(runner).ok(),
                },
            );
            return;
        }
    };

    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            emit_live_picker_event(
                app,
                source_key,
                epoch,
                LivePickerEvent::Offline {
                    message: "agentscan subscribe did not expose stdout".to_owned(),
                    retrying: false,
                    diagnostics: load_daemon_status(runner).ok(),
                },
            );
            return;
        }
    };

    let stderr = child.stderr.take();
    match child_slot.lock() {
        Ok(mut slot) => {
            // A stop that raced ahead of us already ran its kill against an empty
            // slot (the child wasn't stored yet), so re-check the flag under the
            // lock. Otherwise we'd store the child and block in the read loop on a
            // process nobody will kill, wedging the stop that joins this worker.
            if stop.load(Ordering::SeqCst) {
                drop(slot);
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
            *slot = Some(child);
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            // Poisoned mutex (a holder panicked): the child is unreachable for a later
            // stop, so kill it and report a recoverable terminal — TS re-arms (latch-only).
            emit_live_picker_event(
                app,
                source_key,
                epoch,
                LivePickerEvent::Offline {
                    message: classify_desktop_failure(
                        runner,
                        "subscribe",
                        "agentscan subscribe state was poisoned",
                    ),
                    retrying: false,
                    diagnostics: load_daemon_status(runner).ok(),
                },
            );
            return;
        }
    }

    // Drain stderr on its own thread, accumulating into a shared buffer rather
    // than joining the thread directly: a descendant that inherited this pipe
    // (e.g. an auto-started daemon) can hold it open after the subscribe child
    // exits, so an unbounded join would wedge the worker — and any stop joining
    // it — forever. The shared buffer also means the bounded collection below
    // still returns the stderr the thread already read instead of discarding it.
    let stderr_collector = spawn_pipe_collector(stderr);
    // Set once a terminal frame (shutdown / offline-retrying-false / fatal) ended the read,
    // so the generic process-exit fallback below doesn't also emit for a clean terminal.
    let mut saw_terminal = false;
    // Set when the loop already emitted an Offline describing why it ended, so
    // the generic exit-reason emit below doesn't overwrite it with a vaguer
    // (or, after we kill the child, misleading) message.
    let mut reported_offline = false;

    for line in BufReader::new(stdout).lines() {
        if stop.load(Ordering::SeqCst) {
            break;
        }

        match line {
            Ok(line) if line.trim().is_empty() => {}
            Ok(line) => match serde_json::from_str::<SubscribeFrame>(&line) {
                Ok(frame) => match handle_subscribe_frame(app, runner, frame, source_key, epoch) {
                    LivePickerWorkerExit::Retry => {}
                    _ => {
                        saw_terminal = true;
                        break;
                    }
                },
                Err(error) => {
                    let message = classify_desktop_failure(
                        runner,
                        "subscribe",
                        &format!("Invalid agentscan subscribe frame: {error}"),
                    );
                    emit_live_picker_event(
                        app,
                        source_key,
                        epoch,
                        LivePickerEvent::Offline {
                            message,
                            retrying: false,
                            diagnostics: load_daemon_status(runner).ok(),
                        },
                    );
                    // A malformed frame is a protocol error, not a process exit:
                    // the child keeps stdout open and would block the wait below
                    // forever. Kill it so the worker can fall through to teardown.
                    kill_live_picker_child(child_slot);
                    reported_offline = true;
                    break;
                }
            },
            Err(error) => {
                if !stop.load(Ordering::SeqCst) {
                    let message = classify_desktop_failure(
                        runner,
                        "subscribe",
                        &format!("Unable to read agentscan subscribe output: {error}"),
                    );
                    emit_live_picker_event(
                        app,
                        source_key,
                        epoch,
                        LivePickerEvent::Offline {
                            message,
                            retrying: false,
                            diagnostics: load_daemon_status(runner).ok(),
                        },
                    );
                    reported_offline = true;
                }
                break;
            }
        }
    }

    let status_message = wait_for_live_picker_child(child_slot);
    let stderr = filter_stderr_text(&collect_pipe(stderr_collector, LIVE_CHILD_EXIT_GRACE));

    if stop.load(Ordering::SeqCst) {
        return;
    }

    if !saw_terminal && !reported_offline {
        let message = classify_desktop_failure(
            runner,
            "subscribe",
            &process_exit_message(status_message.as_deref(), &stderr),
        );
        emit_live_picker_event(
            app,
            source_key,
            epoch,
            LivePickerEvent::Offline {
                message,
                retrying: false,
                diagnostics: load_daemon_status(runner).ok(),
            },
        );
    }
}

fn handle_subscribe_frame(
    app: &tauri::AppHandle,
    runner: &AgentscanRunner,
    frame: SubscribeFrame,
    source_key: &str,
    epoch: u64,
) -> LivePickerWorkerExit {
    match live_event_from_subscribe_frame(runner, frame) {
        // A heartbeat (or any frame the worker doesn't act on) maps to no event:
        // keep reading the stream without disturbing the picker.
        Ok(None) => LivePickerWorkerExit::Retry,
        Ok(Some((event, exit))) => {
            emit_live_picker_event(app, source_key, epoch, event);
            exit
        }
        Err(message) => {
            emit_live_picker_event(
                app,
                source_key,
                epoch,
                LivePickerEvent::Fatal {
                    message,
                    diagnostics: load_daemon_status(runner).ok(),
                },
            );
            LivePickerWorkerExit::Fatal
        }
    }
}

fn live_event_from_subscribe_frame(
    runner: &AgentscanRunner,
    frame: SubscribeFrame,
) -> Result<Option<(LivePickerEvent, LivePickerWorkerExit)>, String> {
    match frame {
        SubscribeFrame::Connecting { message } => Ok(Some((
            LivePickerEvent::Connecting { message },
            LivePickerWorkerExit::Retry,
        ))),
        SubscribeFrame::Snapshot { snapshot, rows } => {
            // Rows arrive already built by the host (correct focus, client count,
            // and workspace grouping), so no second `agentscan hotkeys` scan is
            // spawned per frame. Validate them the same way the standalone fetch
            // does; a semantic problem degrades to Offline/retry, keeping the last
            // good picker visible rather than tearing the subscription down.
            if let Err(message) = validate_picker_rows(&rows) {
                return Ok(Some((
                    LivePickerEvent::Offline {
                        message: classify_desktop_failure(runner, "subscribe", &message),
                        retrying: true,
                        diagnostics: load_daemon_status(runner).ok(),
                    },
                    LivePickerWorkerExit::Retry,
                )));
            }
            let snapshot = summarize_snapshot(&snapshot);
            Ok(Some((
                LivePickerEvent::Rows { rows, snapshot },
                LivePickerWorkerExit::Retry,
            )))
        }
        SubscribeFrame::Offline { message, retrying } => Ok(Some((
            LivePickerEvent::Offline {
                message,
                retrying,
                diagnostics: load_daemon_status(runner).ok(),
            },
            // Honor the daemon's own retry decision: a terminal offline frame
            // (retrying:false, e.g. auto-start disabled) must settle, not loop
            // the subscription forever.
            if retrying {
                LivePickerWorkerExit::Retry
            } else {
                LivePickerWorkerExit::Shutdown
            },
        ))),
        SubscribeFrame::Shutdown { message } => Ok(Some((
            LivePickerEvent::Shutdown { message },
            LivePickerWorkerExit::Shutdown,
        ))),
        SubscribeFrame::Fatal { message } => Ok(Some((
            LivePickerEvent::Fatal {
                message,
                diagnostics: load_daemon_status(runner).ok(),
            },
            LivePickerWorkerExit::Fatal,
        ))),
        // Heartbeat or any unrecognized (forward-compat) frame type: no
        // picker-visible state, so emit nothing and keep reading the stream.
        SubscribeFrame::Keepalive | SubscribeFrame::Unknown => Ok(None),
    }
}

fn summarize_snapshot(snapshot: &serde_json::Value) -> LiveSnapshotSummary {
    LiveSnapshotSummary {
        pane_count: snapshot
            .get("panes")
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len),
        generated_at: snapshot
            .get("generated_at")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        source_kind: snapshot
            .get("source")
            .and_then(|source| source.get("kind"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
    }
}

// Render collected stderr bytes into a compact message, dropping blank lines.
// Takes already-buffered bytes (from a pipe collector) so partial diagnostics
// survive even when the pipe never reaches EOF because a descendant holds it.
fn filter_stderr_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn wait_for_live_picker_child(child_slot: &Arc<Mutex<Option<Child>>>) -> Option<String> {
    // Take the child out and drop the slot guard before blocking, so a
    // concurrent kill (profile switch / shutdown) can always reach the slot
    // while we reap. (If that kill ran first it already took the child, and
    // this returns None.)
    let mut child = child_slot.lock().ok().and_then(|mut slot| slot.take())?;

    // The child has almost always exited already (that is why stdout reached
    // EOF). If it lingers after signaling termination, give it a short grace
    // period and then kill it rather than waiting unbounded.
    let deadline = Instant::now() + LIVE_CHILD_EXIT_GRACE;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Some(format!("agentscan subscribe exited with status {status}"));
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                return Some(match child.wait() {
                    Ok(status) => {
                        format!("agentscan subscribe did not exit; terminated ({status})")
                    }
                    Err(error) => format!("Unable to wait for agentscan subscribe: {error}"),
                });
            }
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(error) => return Some(format!("Unable to wait for agentscan subscribe: {error}")),
        }
    }
}

fn kill_live_picker_child(child_slot: &Arc<Mutex<Option<Child>>>) {
    // Take the child out and release the slot lock before blocking in wait(),
    // so other lifecycle paths can still acquire the slot while we reap.
    let child = child_slot.lock().ok().and_then(|mut slot| slot.take());
    if let Some(mut child) = child {
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn process_exit_message(status_message: Option<&str>, stderr: &str) -> String {
    let stderr = stderr.trim();

    match (status_message, stderr.is_empty()) {
        (Some(status), true) => status.to_owned(),
        (Some(status), false) => format!("{status}: {stderr}"),
        (None, true) => "agentscan subscribe exited".to_owned(),
        (None, false) => stderr.to_owned(),
    }
}

fn emit_live_picker_event(
    app: &tauri::AppHandle,
    source_key: &str,
    epoch: u64,
    event: LivePickerEvent,
) {
    let _ = tauri::Emitter::emit(
        app,
        LIVE_PICKER_EVENT,
        LivePickerEnvelope {
            source_key: source_key.to_owned(),
            epoch,
            event,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::LocalRunnerSettings;

    #[test]
    fn subscribe_args_appends_no_auto_start_when_latching() {
        // Auto-start enabled (explicit "Start agentscan"): no flag, daemon may spawn.
        assert_eq!(subscribe_args(true), vec!["subscribe", "--format", "json"]);
        // Latch-only (reconnect/launch): never spawn a daemon, only attach to one.
        assert_eq!(
            subscribe_args(false),
            vec!["subscribe", "--format", "json", "--no-auto-start"]
        );
    }

    #[test]
    fn start_epoch_gate_is_per_key() {
        let mut fence = StartFence::default();
        fence.last_started.insert("source-a".to_owned(), 5);

        // Same key: only a strictly newer epoch advances; equal/older is stale.
        assert!(!epoch_advances(&fence, "source-a", 4));
        assert!(!epoch_advances(&fence, "source-a", 5));
        assert!(epoch_advances(&fence, "source-a", 6));

        // A different key gates independently — its own history starts empty, so
        // source-a's higher watermark cannot reject source-b's first start.
        assert!(epoch_advances(&fence, "source-b", 1));
    }

    #[test]
    fn start_fence_evicts_lowest_epoch_and_holds_the_floor() {
        let mut fence = StartFence::default();
        // Fill to the cap, then one more: the lowest-epoch entry is evicted and
        // its epoch becomes the floor.
        for n in 1..=(LIVE_PICKER_FENCE_CAP as u64 + 1) {
            commit_start_epoch(&mut fence, format!("source-{n}"), n);
        }
        assert_eq!(fence.last_started.len(), LIVE_PICKER_FENCE_CAP);
        assert_eq!(fence.floor, 1);

        // The evicted key's stale epoch is still rejected via the floor (epochs
        // are globally monotonic, so at-or-below-floor means globally stale), and
        // fresh epochs pass for any key, evicted or new.
        assert!(!fence.last_started.contains_key("source-1"));
        assert!(!epoch_advances(&fence, "source-1", 1));
        assert!(epoch_advances(
            &fence,
            "source-1",
            LIVE_PICKER_FENCE_CAP as u64 + 2
        ));
        assert!(epoch_advances(
            &fence,
            "source-new",
            LIVE_PICKER_FENCE_CAP as u64 + 2
        ));

        // Gate equivalence for the evicted key: the floor equals exactly the epoch
        // its entry held, so eviction cannot admit any start its entry would have
        // rejected — a worker still running at the evicted epoch stays protected.
        assert!(!epoch_advances(&fence, "source-1", fence.floor));
    }

    #[test]
    fn live_picker_envelope_tags_source_key_and_epoch() {
        let envelope = LivePickerEnvelope {
            source_key: "ssh:koopa".to_owned(),
            epoch: 7,
            event: LivePickerEvent::Connecting {
                message: "connecting".to_owned(),
            },
        };

        let json = serde_json::to_value(&envelope).expect("envelope serializes");

        // The frontend routes frames per source by `sourceKey` (camelCase over the
        // wire) and fences stale workers by `epoch`; the event payload stays flattened.
        assert_eq!(json["sourceKey"], "ssh:koopa");
        assert_eq!(json["epoch"], 7);
        assert_eq!(json["kind"], "connecting");
        assert_eq!(json["message"], "connecting");
    }

    #[test]
    fn stop_supervisor_epoch_gate_only_stops_its_own_key() {
        // Keys are unique to this test so the shared global map stays isolated
        // from other tests running in parallel.
        let key_a = "test-stop-gate-a";
        let key_b = "test-stop-gate-b";
        let supervisor = |epoch: u64| LivePickerSupervisor {
            epoch,
            stop: Arc::new(AtomicBool::new(false)),
            child: Arc::new(Mutex::new(None)),
            worker: None,
        };
        {
            let mut guard = live_picker_supervisors().lock().expect("supervisor lock");
            guard.insert(key_a.to_owned(), supervisor(5));
            guard.insert(key_b.to_owned(), supervisor(9));
        }

        // A stale stop (wrong epoch) leaves the key's supervisor running.
        stop_live_picker_supervisor_for_epoch(key_a, Some(4)).expect("stale stop is a no-op");
        {
            let guard = live_picker_supervisors().lock().expect("supervisor lock");
            assert_eq!(guard.get(key_a).map(|current| current.epoch), Some(5));
        }
        // A matching stop removes ONLY its own key; the sibling key is untouched.
        stop_live_picker_supervisor_for_epoch(key_a, Some(5)).expect("matching stop succeeds");

        let mut guard = live_picker_supervisors().lock().expect("supervisor lock");
        assert!(!guard.contains_key(key_a));
        assert_eq!(guard.get(key_b).map(|current| current.epoch), Some(9));
        guard.remove(key_b);
    }

    #[test]
    fn subscribe_lifecycle_frames_parse() {
        let frame: SubscribeFrame =
            serde_json::from_str(r#"{"type":"connecting","message":"connecting"}"#)
                .expect("connecting frame parses");

        assert_eq!(
            frame,
            SubscribeFrame::Connecting {
                message: "connecting".to_owned()
            }
        );

        let frame: SubscribeFrame =
            serde_json::from_str(r#"{"type":"offline","message":"lost","retrying":true}"#)
                .expect("offline frame parses");

        assert_eq!(
            frame,
            SubscribeFrame::Offline {
                message: "lost".to_owned(),
                retrying: true
            }
        );
    }

    #[test]
    fn subscribe_keepalive_frame_parses_to_keepalive_variant() {
        // The daemon emits this idle heartbeat ~1/s; the consumer must accept it
        // rather than tear the subscription down with a spurious "Offline, retrying".
        let frame: SubscribeFrame =
            serde_json::from_str(r#"{"type":"keepalive"}"#).expect("keepalive frame parses");

        assert_eq!(frame, SubscribeFrame::Keepalive);
    }

    #[test]
    fn keepalive_frame_maps_to_no_event() {
        // Keepalive is a no-op for the picker: it produces no event and keeps the
        // worker reading the stream.
        let event = live_event_from_subscribe_frame(
            &AgentscanRunner::Local(LocalRunnerSettings {
                binary_path: None,
                env: Vec::new(),
            }),
            SubscribeFrame::Keepalive,
        )
        .expect("keepalive maps cleanly");

        assert!(event.is_none(), "keepalive must not emit a picker event");
    }

    #[test]
    fn subscribe_unknown_frame_type_parses_to_unknown_variant() {
        // AUR-457: a frame whose `type` is not a known variant must be absorbed as
        // Unknown (forward-compat) rather than failing to parse, even with a payload.
        let frame: SubscribeFrame = serde_json::from_str(r#"{"type":"future_thing","x":1}"#)
            .expect("unknown frame type parses to Unknown");

        assert_eq!(frame, SubscribeFrame::Unknown);
    }

    #[test]
    fn unknown_frame_maps_to_no_event() {
        // Unknown is a no-op for the picker (same as Keepalive): no event, keep reading.
        let event = live_event_from_subscribe_frame(
            &AgentscanRunner::Local(LocalRunnerSettings {
                binary_path: None,
                env: Vec::new(),
            }),
            SubscribeFrame::Unknown,
        )
        .expect("unknown maps cleanly");

        assert!(
            event.is_none(),
            "unknown frame must not emit a picker event"
        );
    }

    #[test]
    fn snapshot_frame_renders_delivered_rows_without_second_scan() {
        // The host ships picker rows in the snapshot frame, so the desktop maps them
        // straight into a Rows event — no `agentscan hotkeys` spawn. With a Local
        // runner that has no binary, this succeeding at all proves nothing was run.
        let frame: SubscribeFrame = serde_json::from_str(
            r#"{
              "type": "snapshot",
              "snapshot": {
                "generated_at": "2026-05-23T20:00:00Z",
                "source": { "kind": "daemon" },
                "panes": [{ "pane_id": "%1" }]
              },
              "rows": [
                {
                  "key": "1",
                  "pane_id": "%1",
                  "provider": "codex",
                  "status": { "kind": "idle" },
                  "display_label": "Root Task",
                  "location_tag": "work:0.0",
                  "location": { "session_name": "work" }
                }
              ]
            }"#,
        )
        .expect("snapshot frame with rows parses");

        let (event, exit) = live_event_from_subscribe_frame(
            &AgentscanRunner::Local(LocalRunnerSettings {
                binary_path: None,
                env: Vec::new(),
            }),
            frame,
        )
        .expect("snapshot frame maps cleanly")
        .expect("snapshot frame emits an event");

        assert_eq!(exit, LivePickerWorkerExit::Retry);
        match event {
            LivePickerEvent::Rows { rows, snapshot } => {
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].pane_id, "%1");
                assert_eq!(snapshot.pane_count, 1);
                assert_eq!(snapshot.source_kind.as_deref(), Some("daemon"));
            }
            other => panic!("expected Rows event, got {other:?}"),
        }
    }

    #[test]
    fn snapshot_frame_missing_rows_is_a_protocol_error() {
        // `rows` is a required field: a host too old to send it yields a malformed
        // known frame, which the reader treats as a protocol error (teardown +
        // reconnect) rather than silently rendering an empty picker.
        assert!(
            serde_json::from_str::<SubscribeFrame>(
                r#"{"type":"snapshot","snapshot":{"panes":[]}}"#
            )
            .is_err(),
            "snapshot frame missing `rows` must error"
        );
    }

    #[test]
    fn malformed_known_frame_still_errors() {
        // A *known* type with a missing/bad payload is a real protocol violation and
        // must still error (→ teardown + reconnect), not be swallowed as Unknown.
        assert!(
            serde_json::from_str::<SubscribeFrame>(r#"{"type":"snapshot"}"#).is_err(),
            "snapshot missing its `snapshot` field must error"
        );
        // Non-JSON is likewise a hard error.
        assert!(serde_json::from_str::<SubscribeFrame>("not json").is_err());
    }

    #[test]
    fn snapshot_summary_reads_canonical_fields() {
        let snapshot: serde_json::Value = serde_json::from_str(
            r#"{
              "generated_at": "2026-05-23T20:00:00Z",
              "source": { "kind": "daemon" },
              "panes": [{ "pane_id": "%1" }, { "pane_id": "%2" }]
            }"#,
        )
        .expect("snapshot parses");

        assert_eq!(
            summarize_snapshot(&snapshot),
            LiveSnapshotSummary {
                pane_count: 2,
                generated_at: Some("2026-05-23T20:00:00Z".to_owned()),
                source_kind: Some("daemon".to_owned())
            }
        );
    }

    #[test]
    fn snapshot_summary_defaults_missing_optional_fields() {
        let snapshot: serde_json::Value =
            serde_json::from_str(r#"{ "panes": [] }"#).expect("snapshot parses");

        assert_eq!(
            summarize_snapshot(&snapshot),
            LiveSnapshotSummary {
                pane_count: 0,
                generated_at: None,
                source_kind: None
            }
        );
    }

    #[test]
    fn process_exit_message_preserves_stderr_context() {
        assert_eq!(
            process_exit_message(
                Some("agentscan subscribe exited with status 1"),
                "tmux missing"
            ),
            "agentscan subscribe exited with status 1: tmux missing"
        );

        assert_eq!(process_exit_message(None, ""), "agentscan subscribe exited");
    }
}
