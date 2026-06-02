import { Data, Effect, Queue, Runtime, Scope } from "effect";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { DesktopRunnerSettings, LivePickerEnvelope, PickerRow } from "./types";

// Keep in sync with LIVE_PICKER_EVENT in src-tauri/src/lib.rs.
const LIVE_PICKER_EVENT = "agentscan-live-picker";

const messageOf = (error: unknown) =>
  error instanceof Error ? error.message : String(error);

// A failed Tauri command. Carries the op so the lifecycle can classify it.
export class IpcError extends Data.TaggedError("IpcError")<{
  readonly op: string;
  readonly message: string;
}> {}

const invokeEffect = <A>(op: string, args: Record<string, unknown>) =>
  Effect.tryPromise({
    try: () => invoke<A>(op, args),
    catch: (error) => new IpcError({ op, message: messageOf(error) }),
  });

// The IPC boundary. Every Tauri `invoke`/`listen` the live lifecycle needs is
// wrapped here as an Effect/scoped resource, so LiveConnection is pure logic over
// this interface and a test can swap in a scripted implementation.
export class TauriIpc extends Effect.Service<TauriIpc>()("desktop/TauriIpc", {
  succeed: {
    // Install the Rust live-picker worker. `autoStart` is the latch policy: false
    // (reconnect/launch) subscribes with `--no-auto-start` and only attaches to a
    // running daemon; true (explicit "Start agentscan") lets it spawn one.
    //
    // The JS key `autoStart` (camelCase) maps to the Rust command's `auto_start`
    // (snake_case): Tauri v2 converts command-argument names by convention, exactly
    // as the existing, shipped focus_picker_row does (JS `paneId` -> Rust `pane_id`,
    // see App.tsx + lib.rs). Do NOT rename this to `auto_start` — that would make
    // Tauri look for a camelCase `autoStart` Rust param and break the invoke.
    startLivePicker: (input: {
      settings: DesktopRunnerSettings;
      epoch: number;
      autoStart: boolean;
    }) =>
      invokeEffect<void>("start_live_picker", {
        settings: input.settings,
        epoch: input.epoch,
        autoStart: input.autoStart,
      }),

    stopLivePicker: (epoch: number) => invokeEffect<void>("stop_live_picker", { epoch }),

    loadPickerRows: (settings: DesktopRunnerSettings) =>
      invokeEffect<PickerRow[]>("load_picker_rows", { settings }),

    // A scoped queue of live envelopes. Awaiting this registers the Tauri listener
    // BEFORE the caller starts a subscription (so no early frame is missed), and
    // unregisters it when the scope closes.
    liveEvents: Effect.gen(function* () {
      const queue = yield* Queue.unbounded<LivePickerEnvelope>();
      const runFork = Runtime.runFork(yield* Effect.runtime<never>());
      yield* Effect.acquireRelease(
        // tryPromise (not promise): a rejected `listen` is a typed IpcError the
        // LiveConnection supervisor can surface as a fatal connection state, not a
        // defect that would tear the supervisor fiber down with no UI feedback.
        Effect.tryPromise({
          try: () =>
            listen<LivePickerEnvelope>(LIVE_PICKER_EVENT, (event) => {
              runFork(Queue.offer(queue, event.payload));
            }),
          catch: (error) =>
            new IpcError({ op: `listen:${LIVE_PICKER_EVENT}`, message: messageOf(error) }),
        }),
        (unlisten: UnlistenFn) => Effect.sync(() => unlisten()),
      );
      return queue as Queue.Dequeue<LivePickerEnvelope>;
    }),
  },
}) {}
