import { Effect } from "effect";
import { invoke } from "@tauri-apps/api/core";
import { IpcError } from "./TauriIpc";

const messageOf = (error: unknown) => (error instanceof Error ? error.message : String(error));

// The local-hostname boundary: the single Tauri `invoke` both windows run to
// resolve the local machine's short hostname for the local source's label.
// Injected as a service (the PreflightIpc pattern) so the atom stays pure over
// it and tests can script it without a real Tauri host. `Effect.tryPromise`
// only constructs a description — nothing fires at layer build, preserving the
// side-effect-free runtime-build invariant (see shared.ts / atoms.ts).
export class HostIpc extends Effect.Service<HostIpc>()("desktop/HostIpc", {
  succeed: {
    localHostLabel: Effect.tryPromise({
      try: () => invoke<string>("local_host_label"),
      catch: (error) => new IpcError({ op: "local_host_label", message: messageOf(error) }),
    }),
  },
}) {}
