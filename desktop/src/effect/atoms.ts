import { Atom } from "@effect-atom/atom-react";
import { Effect } from "effect";
import { LiveConnection, type ConfigureInput } from "./LiveConnection";

// One runtime for the whole dock window, wired from the LiveConnection layer (which
// pulls in TauriIpc + config via its dependencies). The settings window never reads
// these atoms, so its supervisor never starts.
const runtime = Atom.runtime(LiveConnection.Default);

// The live state the dock observes: Result<LiveState> (connection status + rows).
// keepAlive so the supervised connection fiber persists across re-renders/StrictMode
// remounts for the dock session rather than tearing down when momentarily unmounted.
export const liveStateAtom = Atom.keepAlive(
  runtime.subscriptionRef(Effect.map(LiveConnection, (lc) => lc.state)),
);

// The only path that may spawn a daemon — the "Start agentscan" affordance.
export const startAtom = runtime.fn(
  Effect.fnUntraced(function* () {
    const lc = yield* LiveConnection;
    yield* lc.start;
  }),
);

// Re-arm the live connection now (latch only). Backs the Refresh button and the
// fatal-state Reconnect action.
export const reconnectAtom = runtime.fn(
  Effect.fnUntraced(function* () {
    const lc = yield* LiveConnection;
    yield* lc.reconnect;
  }),
);

// Re-target the connection when the active profile/preflight changes.
export const configureAtom = runtime.fn(
  Effect.fnUntraced(function* (input: ConfigureInput) {
    const lc = yield* LiveConnection;
    yield* lc.configure(input);
  }),
);
