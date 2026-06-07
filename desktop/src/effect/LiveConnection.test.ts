import { Deferred, Duration, Effect, Layer, Option, Queue, Ref, Stream } from "effect";
import { describe, expect, it } from "vitest";
import { LiveConnection, LiveConnectionConfig } from "./LiveConnection";
import { IpcError, TauriIpc } from "./TauriIpc";
import type {
  ConnectionStatus,
  DesktopRunnerSettings,
  LivePickerEnvelope,
  LivePickerEvent,
  LiveSnapshotSummary,
  PickerRow,
} from "./types";

const SETTINGS: DesktopRunnerSettings = { kind: "local", binaryPath: "", env: [] };

const SNAPSHOT: LiveSnapshotSummary = {
  paneCount: 1,
  generatedAt: null,
  sourceKind: "tmux",
};

const ROW: PickerRow = {
  key: "1",
  pane_id: "%1",
  provider: "claude",
  status: { kind: "idle" },
  display_label: "claude late refresh",
  location_tag: "agentscan:0.0",
  is_active: true,
};

// recoverable=zero keeps the event-driven re-arm (e.g. shutdown → latch) instant, so
// the loop is driven purely by injected frames. noDaemon is parked far past the test
// window: with zero noDaemon backoff the slow auto-latch poll would fire immediately
// and its latch-only re-arm (autoStart:false) could land in `startCalls` and race an
// assertion that's waiting for the NEXT user action (e.g. Start's autoStart:true). The
// pending sleep is interrupted by that action's switch, so a large value never slows a
// test — it only removes the spurious poll. Tests that DO assert the poll use
// EagerBackoff instead.
const StableBackoff = Layer.succeed(LiveConnectionConfig, {
  backoff: { recoverable: Duration.zero, noDaemon: Duration.minutes(60) },
});

// Zero noDaemon backoff so the auto-latch poll re-arms immediately — used only by the
// test that asserts the poll resumes latch-only after a post-Start daemon loss.
const EagerBackoff = Layer.succeed(LiveConnectionConfig, {
  backoff: { recoverable: Duration.zero, noDaemon: Duration.zero },
});

describe("LiveConnection", () => {
  it("latches without auto-start, recovers on shutdown, surfaces noDaemon, and only Start auto-starts", () =>
    Effect.gen(function* () {
      // Scripted IPC: record each start (epoch + autoStart) and feed live frames.
      const startCalls = yield* Queue.unbounded<{ epoch: number; autoStart: boolean }>();
      const events = yield* Queue.unbounded<LivePickerEnvelope>();

      const MockTauri = Layer.succeed(TauriIpc, {
        startLivePicker: ({ epoch, autoStart }) =>
          Queue.offer(startCalls, { epoch, autoStart }).pipe(Effect.asVoid),
        stopLivePicker: () => Effect.void,
        loadPickerRows: () => Effect.succeed<PickerRow[]>([]),
        pollDaemonStatus: () => Effect.succeed({ reachable: true }),
        liveEvents: Effect.succeed(events as Queue.Dequeue<LivePickerEnvelope>),
      });

      const program = Effect.gen(function* () {
        const lc = yield* LiveConnection;

        const emit = (epoch: number, event: LivePickerEvent) =>
          Queue.offer(events, { ...event, epoch } as LivePickerEnvelope);

        // Block until the connection state reaches a given status (changes replays
        // the current value, so this resolves immediately if already there).
        const awaitStatus = (status: ConnectionStatus["status"]) =>
          lc.state.changes.pipe(
            Stream.filter((state) => state.connection.status === status),
            Stream.runHead,
            Effect.flatMap(
              Option.match({
                onNone: () => Effect.die("state stream ended early"),
                onSome: Effect.succeed,
              }),
            ),
          );

        // 1. Enable the source → first subscription must LATCH (autoStart false),
        //    and a rows frame brings it online.
        yield* lc.configure({ settings: SETTINGS, runnerKey: "k1", enabled: true });
        const first = yield* Queue.take(startCalls);
        expect(first.autoStart).toBe(false);
        yield* emit(first.epoch, { kind: "rows", rows: [ROW], snapshot: SNAPSHOT });
        const online = yield* awaitStatus("online");
        expect(online.rows.map((r) => r.pane_id)).toEqual(["%1"]);

        // 2. Daemon closes → auto re-arm, still latch-only (autoStart false).
        yield* emit(first.epoch, {
          kind: "shutdown",
          message: "daemon socket server is closing",
        });
        const second = yield* Queue.take(startCalls);
        expect(second.autoStart).toBe(false);

        // 3. No daemon reachable (the latch subscribe reports auto-start disabled) →
        //    the dock surfaces noDaemon rather than wedging.
        yield* emit(second.epoch, {
          kind: "fatal",
          message: "daemon auto-start is disabled: socket is missing",
          diagnostics: null,
        });
        const noDaemon = yield* awaitStatus("noDaemon");
        expect(noDaemon.connection.status).toBe("noDaemon");
        expect(noDaemon.rows).toEqual([]);

        // 4. Explicit "Start agentscan" → re-arm WITH auto-start (the only such path).
        yield* lc.start;
        const third = yield* Queue.take(startCalls);
        expect(third.autoStart).toBe(true);
      });

      yield* program.pipe(
        Effect.provide(
          LiveConnection.DefaultWithoutDependencies.pipe(
            Layer.provide(Layer.merge(MockTauri, StableBackoff)),
          ),
        ),
      );
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));

  it("re-arms latch-only on an abnormal subscribe-child death (offline retrying:false, non-noDaemon)", () =>
    Effect.gen(function* () {
      // AUR-517: the Rust worker is single-shot, so an abnormal subscribe-child death
      // (spawn/IO/protocol failure or a bare exit) arrives as a terminal Offline with
      // retrying:false whose message is NOT "auto-start is disabled". This service must
      // classify it Recoverable and re-arm the subscription with a FRESH epoch, latch-only
      // (autoStart:false), surfacing "reconnecting" — never noDaemon and never auto-start.
      // This is where the latch-on-retry invariant now lives (it used to be the Rust
      // auto_start_for_attempt guard inside the worker's own loop).
      const startCalls = yield* Queue.unbounded<{ epoch: number; autoStart: boolean }>();
      const events = yield* Queue.unbounded<LivePickerEnvelope>();

      const MockTauri = Layer.succeed(TauriIpc, {
        startLivePicker: ({ epoch, autoStart }) =>
          Queue.offer(startCalls, { epoch, autoStart }).pipe(Effect.asVoid),
        stopLivePicker: () => Effect.void,
        loadPickerRows: () => Effect.succeed<PickerRow[]>([]),
        pollDaemonStatus: () => Effect.succeed({ reachable: true }),
        liveEvents: Effect.succeed(events as Queue.Dequeue<LivePickerEnvelope>),
      });

      const program = Effect.gen(function* () {
        const lc = yield* LiveConnection;

        const emit = (epoch: number, event: LivePickerEvent) =>
          Queue.offer(events, { ...event, epoch } as LivePickerEnvelope);

        const awaitStatus = (status: ConnectionStatus["status"]) =>
          lc.state.changes.pipe(
            Stream.filter((state) => state.connection.status === status),
            Stream.runHead,
            Effect.flatMap(
              Option.match({
                onNone: () => Effect.die("state stream ended early"),
                onSome: Effect.succeed,
              }),
            ),
          );

        // Latch onto a daemon and come online.
        yield* lc.configure({ settings: SETTINGS, runnerKey: "k1", enabled: true });
        const first = yield* Queue.take(startCalls);
        expect(first.autoStart).toBe(false);
        yield* emit(first.epoch, { kind: "rows", rows: [ROW], snapshot: SNAPSHOT });
        yield* awaitStatus("online");

        // Abnormal child death: terminal offline, retrying:false, NOT a no-daemon message.
        yield* emit(first.epoch, {
          kind: "offline",
          message: "Unable to read agentscan subscribe output: broken pipe",
          retrying: false,
          diagnostics: null,
        });

        // Recovery: a re-arm on a fresh epoch, latch-only, surfacing "reconnecting".
        const second = yield* Queue.take(startCalls);
        expect(second.autoStart).toBe(false);
        expect(second.epoch).toBeGreaterThan(first.epoch);
        const reconnecting = yield* awaitStatus("reconnecting");
        expect(reconnecting.connection.status).toBe("reconnecting");
      });

      yield* program.pipe(
        Effect.provide(
          LiveConnection.DefaultWithoutDependencies.pipe(
            Layer.provide(Layer.merge(MockTauri, StableBackoff)),
          ),
        ),
      );
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));

  it("surfaces a worker-install failure as fatal instead of looping forever", () =>
    Effect.gen(function* () {
      // startLivePicker rejects (worker could not be installed at all). With zero
      // backoff, a "Recoverable" misclassification would spin this test forever; the
      // 5s timeout below is the safety net proving it settles on fatal instead.
      const events = yield* Queue.unbounded<LivePickerEnvelope>();
      const FailingTauri = Layer.succeed(TauriIpc, {
        startLivePicker: () =>
          Effect.fail(new IpcError({ op: "start_live_picker", message: "boom: command failed" })),
        stopLivePicker: () => Effect.void,
        loadPickerRows: () => Effect.succeed<PickerRow[]>([]),
        pollDaemonStatus: () => Effect.succeed({ reachable: true }),
        liveEvents: Effect.succeed(events as Queue.Dequeue<LivePickerEnvelope>),
      });

      const program = Effect.gen(function* () {
        const lc = yield* LiveConnection;
        yield* lc.configure({ settings: SETTINGS, runnerKey: "k1", enabled: true });
        const fatal = yield* lc.state.changes.pipe(
          Stream.filter((state) => state.connection.status === "fatal"),
          Stream.runHead,
          Effect.flatMap(
            Option.match({
              onNone: () => Effect.die("state stream ended early"),
              onSome: Effect.succeed,
            }),
          ),
        );
        // The real cause is surfaced, not swallowed under a generic message.
        expect((fatal.connection as { message: string }).message).toContain("boom");
        expect(fatal.rows).toEqual([]);
      });

      yield* program.pipe(
        Effect.provide(
          LiveConnection.DefaultWithoutDependencies.pipe(
            Layer.provide(Layer.merge(FailingTauri, StableBackoff)),
          ),
        ),
      );
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));

  it("survives an event-listener install failure and recovers on reconnect", () =>
    Effect.gen(function* () {
      // liveEvents (the Tauri `listen` install) rejects the FIRST time, then succeeds.
      // If a listener failure killed the supervisor fiber, the later reconnect would
      // vanish with no consumer and this test would hang to its 5s timeout. Reaching
      // online proves the supervisor parked on fatal and re-armed instead of dying.
      const events = yield* Queue.unbounded<LivePickerEnvelope>();
      const startCalls = yield* Queue.unbounded<{ epoch: number }>();
      const failNextListen = yield* Ref.make(true);

      const FlakyListenerTauri = Layer.succeed(TauriIpc, {
        startLivePicker: ({ epoch }) => Queue.offer(startCalls, { epoch }).pipe(Effect.asVoid),
        stopLivePicker: () => Effect.void,
        loadPickerRows: () => Effect.succeed<PickerRow[]>([]),
        pollDaemonStatus: () => Effect.succeed({ reachable: true }),
        liveEvents: Effect.flatMap(Ref.getAndSet(failNextListen, false), (shouldFail) =>
          shouldFail
            ? Effect.fail(new IpcError({ op: "listen", message: "listener boom" }))
            : Effect.succeed(events as Queue.Dequeue<LivePickerEnvelope>),
        ),
      });

      const program = Effect.gen(function* () {
        const lc = yield* LiveConnection;
        const awaitStatus = (status: ConnectionStatus["status"]) =>
          lc.state.changes.pipe(
            Stream.filter((state) => state.connection.status === status),
            Stream.runHead,
            Effect.flatMap(
              Option.match({
                onNone: () => Effect.die("state stream ended early"),
                onSome: Effect.succeed,
              }),
            ),
          );

        // First arm: the listener install fails → fatal (not a silent wedge), and no
        // subscription was started (liveEvents is acquired before startLivePicker).
        yield* lc.configure({ settings: SETTINGS, runnerKey: "k1", enabled: true });
        const fatal = yield* awaitStatus("fatal");
        expect((fatal.connection as { message: string }).message).toContain("listener boom");

        // Reconnect: the supervisor is still alive, re-arms, the listener now installs,
        // a subscription starts, and a rows frame brings it online.
        yield* lc.reconnect;
        const started = yield* Queue.take(startCalls);
        yield* Queue.offer(events, {
          epoch: started.epoch,
          kind: "rows",
          rows: [ROW],
          snapshot: SNAPSHOT,
        } as LivePickerEnvelope);
        const online = yield* awaitStatus("online");
        expect(online.rows.map((r) => r.pane_id)).toEqual(["%1"]);
      });

      yield* program.pipe(
        Effect.provide(
          LiveConnection.DefaultWithoutDependencies.pipe(
            Layer.provide(Layer.merge(FlakyListenerTauri, StableBackoff)),
          ),
        ),
      );
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));

  it("treats an explicit-Start auto-start refusal as fatal, not noDaemon", () =>
    Effect.gen(function* () {
      const startCalls = yield* Queue.unbounded<{ epoch: number; autoStart: boolean }>();
      const events = yield* Queue.unbounded<LivePickerEnvelope>();
      const MockTauri = Layer.succeed(TauriIpc, {
        startLivePicker: ({ epoch, autoStart }) =>
          Queue.offer(startCalls, { epoch, autoStart }).pipe(Effect.asVoid),
        stopLivePicker: () => Effect.void,
        loadPickerRows: () => Effect.succeed<PickerRow[]>([]),
        pollDaemonStatus: () => Effect.succeed({ reachable: true }),
        liveEvents: Effect.succeed(events as Queue.Dequeue<LivePickerEnvelope>),
      });

      const program = Effect.gen(function* () {
        const lc = yield* LiveConnection;
        const awaitStatus = (status: ConnectionStatus["status"]) =>
          lc.state.changes.pipe(
            Stream.filter((state) => state.connection.status === status),
            Stream.runHead,
            Effect.flatMap(
              Option.match({
                onNone: () => Effect.die("state stream ended early"),
                onSome: Effect.succeed,
              }),
            ),
          );

        // Latch first, then the user explicitly clicks Start (autoStart true).
        yield* lc.configure({ settings: SETTINGS, runnerKey: "k1", enabled: true });
        yield* Queue.take(startCalls); // the latch attempt
        yield* lc.start;
        const started = yield* Queue.take(startCalls);
        expect(started.autoStart).toBe(true);

        // The daemon refuses OUR explicit start (macOS codesign/trust). It carries the
        // same "auto-start is disabled" text as a latch-miss, but because we asked to
        // start it must settle on fatal (surface the reason) — not loop back to Start.
        yield* Queue.offer(events, {
          epoch: started.epoch,
          kind: "fatal",
          message: "daemon auto-start is disabled: codesign failed",
          diagnostics: null,
        } as LivePickerEnvelope);
        const fatal = yield* awaitStatus("fatal");
        expect((fatal.connection as { message: string }).message).toContain("codesign");
      });

      yield* program.pipe(
        Effect.provide(
          LiveConnection.DefaultWithoutDependencies.pipe(
            Layer.provide(Layer.merge(MockTauri, StableBackoff)),
          ),
        ),
      );
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));

  it("keeps a post-Start daemon loss as noDaemon (latch poll), not a Start refusal", () =>
    Effect.gen(function* () {
      const startCalls = yield* Queue.unbounded<{ epoch: number; autoStart: boolean }>();
      const events = yield* Queue.unbounded<LivePickerEnvelope>();
      const MockTauri = Layer.succeed(TauriIpc, {
        startLivePicker: ({ epoch, autoStart }) =>
          Queue.offer(startCalls, { epoch, autoStart }).pipe(Effect.asVoid),
        stopLivePicker: () => Effect.void,
        loadPickerRows: () => Effect.succeed<PickerRow[]>([]),
        pollDaemonStatus: () => Effect.succeed({ reachable: true }),
        liveEvents: Effect.succeed(events as Queue.Dequeue<LivePickerEnvelope>),
      });

      const program = Effect.gen(function* () {
        const lc = yield* LiveConnection;
        const awaitStatus = (status: ConnectionStatus["status"]) =>
          lc.state.changes.pipe(
            Stream.filter((state) => state.connection.status === status),
            Stream.runHead,
            Effect.flatMap(
              Option.match({
                onNone: () => Effect.die("state stream ended early"),
                onSome: Effect.succeed,
              }),
            ),
          );

        // Latch, then the user clicks Start (autoStart true) and the daemon comes up.
        yield* lc.configure({ settings: SETTINGS, runnerKey: "k1", enabled: true });
        yield* Queue.take(startCalls); // the latch attempt
        yield* lc.start;
        const started = yield* Queue.take(startCalls);
        expect(started.autoStart).toBe(true);

        // The explicit Start SUCCEEDS — a rows frame brings it online.
        yield* Queue.offer(events, {
          epoch: started.epoch,
          kind: "rows",
          rows: [ROW],
          snapshot: SNAPSHOT,
        } as LivePickerEnvelope);
        yield* awaitStatus("online");

        // Then the daemon dies and the worker's OWN latch-only retry (same epoch,
        // auto_start already spent) finds none, reporting the same "auto-start is
        // disabled" text as a refusal. Because we already connected, this is a
        // latch-miss — it must stay noDaemon and keep slow-polling, NOT promote to
        // fatal the way a never-connected Start refusal does.
        yield* Queue.offer(events, {
          epoch: started.epoch,
          kind: "offline",
          message: "daemon auto-start is disabled: socket is missing",
          retrying: false,
          diagnostics: null,
        } as LivePickerEnvelope);
        const noDaemon = yield* awaitStatus("noDaemon");
        expect(noDaemon.connection.status).toBe("noDaemon");

        // The auto-latch poll re-arms latch-only — recovery never re-spawns a daemon.
        const reArm = yield* Queue.take(startCalls);
        expect(reArm.autoStart).toBe(false);
      });

      yield* program.pipe(
        Effect.provide(
          LiveConnection.DefaultWithoutDependencies.pipe(
            Layer.provide(Layer.merge(MockTauri, EagerBackoff)),
          ),
        ),
      );
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));

  it("cheap-polls daemon status and only re-arms a full subscribe once a daemon appears", () =>
    Effect.gen(function* () {
      // AUR-518: while no daemon is reachable, the service must NOT re-arm a full
      // subscribe each backoff tick (expensive over SSH). Instead it cheap-polls
      // `daemon status`; the full subscribe is re-armed only once the probe reports a
      // daemon. Here the probe says "no daemon" twice, then "daemon up" — exactly one
      // full re-arm should land, after the third poll, never per tick.
      const startCalls = yield* Queue.unbounded<{ epoch: number; autoStart: boolean }>();
      const events = yield* Queue.unbounded<LivePickerEnvelope>();
      const pollCount = yield* Ref.make(0);
      const MockTauri = Layer.succeed(TauriIpc, {
        startLivePicker: ({ epoch, autoStart }) =>
          Queue.offer(startCalls, { epoch, autoStart }).pipe(Effect.asVoid),
        stopLivePicker: () => Effect.void,
        loadPickerRows: () => Effect.succeed<PickerRow[]>([]),
        // Absent for the first two probes, reachable on the third.
        pollDaemonStatus: () =>
          Ref.updateAndGet(pollCount, (n) => n + 1).pipe(
            Effect.map((n) => ({ reachable: n >= 3 })),
          ),
        liveEvents: Effect.succeed(events as Queue.Dequeue<LivePickerEnvelope>),
      });

      const program = Effect.gen(function* () {
        const lc = yield* LiveConnection;
        const awaitStatus = (status: ConnectionStatus["status"]) =>
          lc.state.changes.pipe(
            Stream.filter((state) => state.connection.status === status),
            Stream.runHead,
            Effect.flatMap(
              Option.match({
                onNone: () => Effect.die("state stream ended early"),
                onSome: Effect.succeed,
              }),
            ),
          );

        // Latch (autoStart:false), then a NoDaemon latch-miss drops us into the poll.
        yield* lc.configure({ settings: SETTINGS, runnerKey: "k1", enabled: true });
        const first = yield* Queue.take(startCalls);
        expect(first.autoStart).toBe(false);
        yield* Queue.offer(events, {
          epoch: first.epoch,
          kind: "offline",
          message: "daemon auto-start is disabled: socket is missing",
          retrying: false,
          diagnostics: null,
        } as LivePickerEnvelope);
        yield* awaitStatus("noDaemon");

        // The only re-arm lands after the probe finally reports a daemon (3rd poll),
        // latch-only — not one full subscribe per backoff tick.
        const reArm = yield* Queue.take(startCalls);
        expect(reArm.autoStart).toBe(false);
        expect(reArm.epoch).toBeGreaterThan(first.epoch);
        expect(yield* Ref.get(pollCount)).toBe(3);
        expect(yield* Queue.size(startCalls)).toBe(0);
      });

      yield* program.pipe(
        Effect.provide(
          LiveConnection.DefaultWithoutDependencies.pipe(
            Layer.provide(Layer.merge(MockTauri, EagerBackoff)),
          ),
        ),
      );
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));

  it("falls back to re-arming a full subscribe when the daemon-status probe fails", () =>
    Effect.gen(function* () {
      // AUR-518: a probe that can't tell (SSH/timeout/incompatible → IpcError) must not
      // wedge the latch — it falls back to today's behavior and re-arms a full subscribe,
      // which then surfaces the real terminal.
      const startCalls = yield* Queue.unbounded<{ epoch: number; autoStart: boolean }>();
      const events = yield* Queue.unbounded<LivePickerEnvelope>();
      const MockTauri = Layer.succeed(TauriIpc, {
        startLivePicker: ({ epoch, autoStart }) =>
          Queue.offer(startCalls, { epoch, autoStart }).pipe(Effect.asVoid),
        stopLivePicker: () => Effect.void,
        loadPickerRows: () => Effect.succeed<PickerRow[]>([]),
        pollDaemonStatus: () =>
          Effect.fail(new IpcError({ op: "poll_daemon_status", message: "ssh: connection refused" })),
        liveEvents: Effect.succeed(events as Queue.Dequeue<LivePickerEnvelope>),
      });

      const program = Effect.gen(function* () {
        const lc = yield* LiveConnection;
        const awaitStatus = (status: ConnectionStatus["status"]) =>
          lc.state.changes.pipe(
            Stream.filter((state) => state.connection.status === status),
            Stream.runHead,
            Effect.flatMap(
              Option.match({
                onNone: () => Effect.die("state stream ended early"),
                onSome: Effect.succeed,
              }),
            ),
          );

        yield* lc.configure({ settings: SETTINGS, runnerKey: "k1", enabled: true });
        const first = yield* Queue.take(startCalls);
        yield* Queue.offer(events, {
          epoch: first.epoch,
          kind: "offline",
          message: "daemon auto-start is disabled: socket is missing",
          retrying: false,
          diagnostics: null,
        } as LivePickerEnvelope);
        yield* awaitStatus("noDaemon");

        // The failed probe escalates: a fresh latch-only re-arm still happens.
        const reArm = yield* Queue.take(startCalls);
        expect(reArm.autoStart).toBe(false);
        expect(reArm.epoch).toBeGreaterThan(first.epoch);
      });

      yield* program.pipe(
        Effect.provide(
          LiveConnection.DefaultWithoutDependencies.pipe(
            Layer.provide(Layer.merge(MockTauri, EagerBackoff)),
          ),
        ),
      );
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));

  it("stops a worker whose start is interrupted by a switch (no orphaned subscription)", () =>
    Effect.gen(function* () {
      const startedEpochs = yield* Queue.unbounded<number>();
      const stoppedEpochs = yield* Queue.unbounded<number>();
      const events = yield* Queue.unbounded<LivePickerEnvelope>();
      // Gate the (uninterruptible) start so the target can be switched WHILE the first
      // start is still in flight — the exact window the acquireUseRelease fix protects.
      const releaseStart = yield* Deferred.make<void>();

      const MockTauri = Layer.succeed(TauriIpc, {
        startLivePicker: ({ epoch }) =>
          Effect.zipRight(Queue.offer(startedEpochs, epoch), Deferred.await(releaseStart)),
        stopLivePicker: (epoch) => Queue.offer(stoppedEpochs, epoch).pipe(Effect.asVoid),
        loadPickerRows: () => Effect.succeed<PickerRow[]>([]),
        pollDaemonStatus: () => Effect.succeed({ reachable: true }),
        liveEvents: Effect.succeed(events as Queue.Dequeue<LivePickerEnvelope>),
      });

      const program = Effect.gen(function* () {
        const lc = yield* LiveConnection;
        // Arm k1: the first start begins and blocks (still "in flight").
        yield* lc.configure({ settings: SETTINGS, runnerKey: "k1", enabled: true });
        const inFlightEpoch = yield* Queue.take(startedEpochs);
        // Switch to a DISABLED target mid-start: the interrupt is masked until the
        // uninterruptible start finishes, and the new target installs no replacement
        // worker — so without cleanup the in-flight worker would orphan.
        yield* lc.configure({ settings: SETTINGS, runnerKey: "k2", enabled: false });
        // Let the start complete; the pending interrupt then runs release -> stop.
        yield* Deferred.succeed(releaseStart, undefined);
        const stoppedEpoch = yield* Queue.take(stoppedEpochs);
        expect(stoppedEpoch).toBe(inFlightEpoch);
      });

      yield* program.pipe(
        Effect.provide(
          LiveConnection.DefaultWithoutDependencies.pipe(
            Layer.provide(Layer.merge(MockTauri, StableBackoff)),
          ),
        ),
      );
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));

  it("stamps rows with their runner and keeps the prior runner's key during a switch", () =>
    Effect.gen(function* () {
      const startCalls = yield* Queue.unbounded<{ epoch: number }>();
      const events = yield* Queue.unbounded<LivePickerEnvelope>();
      const MockTauri = Layer.succeed(TauriIpc, {
        startLivePicker: ({ epoch }) => Queue.offer(startCalls, { epoch }).pipe(Effect.asVoid),
        stopLivePicker: () => Effect.void,
        loadPickerRows: () => Effect.succeed<PickerRow[]>([]),
        pollDaemonStatus: () => Effect.succeed({ reachable: true }),
        liveEvents: Effect.succeed(events as Queue.Dequeue<LivePickerEnvelope>),
      });

      const program = Effect.gen(function* () {
        const lc = yield* LiveConnection;
        const awaitStatus = (status: ConnectionStatus["status"]) =>
          lc.state.changes.pipe(
            Stream.filter((state) => state.connection.status === status),
            Stream.runHead,
            Effect.flatMap(
              Option.match({
                onNone: () => Effect.die("state stream ended early"),
                onSome: Effect.succeed,
              }),
            ),
          );

        // Source A (k1) latches and streams rows → the rows carry runner k1.
        yield* lc.configure({ settings: SETTINGS, runnerKey: "k1", enabled: true });
        const a = yield* Queue.take(startCalls);
        yield* Queue.offer(events, {
          epoch: a.epoch,
          kind: "rows",
          rows: [ROW],
          snapshot: SNAPSHOT,
        } as LivePickerEnvelope);
        const onlineA = yield* awaitStatus("online");
        expect(onlineA.rowsRunnerKey).toBe("k1");

        // Switch to source B (k2). The new subscription is connecting and the service
        // preserves A's rows (no same-runner flicker) — but they stay stamped k1, so
        // the dock's runner gate (rowsRunnerKey === active) rejects them instead of
        // rendering A's panes (and activating one against B's settings).
        yield* lc.configure({ settings: SETTINGS, runnerKey: "k2", enabled: true });
        const connectingB = yield* awaitStatus("connecting");
        expect(connectingB.rowsRunnerKey).toBe("k1");
        expect(connectingB.rows.map((r) => r.pane_id)).toEqual(["%1"]);

        // B then streams its own rows → now stamped k2 and trusted.
        const b = yield* Queue.take(startCalls);
        yield* Queue.offer(events, {
          epoch: b.epoch,
          kind: "rows",
          rows: [ROW],
          snapshot: SNAPSHOT,
        } as LivePickerEnvelope);
        const onlineB = yield* awaitStatus("online");
        expect(onlineB.rowsRunnerKey).toBe("k2");
      });

      yield* program.pipe(
        Effect.provide(
          LiveConnection.DefaultWithoutDependencies.pipe(
            Layer.provide(Layer.merge(MockTauri, StableBackoff)),
          ),
        ),
      );
    }).pipe(Effect.timeout(Duration.seconds(5)), Effect.runPromise));
});
