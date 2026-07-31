import { Context, Effect, Layer } from "effect";
import { invoke } from "@tauri-apps/api/core";
import { IpcError } from "./TauriIpc";
import { decodeLocalHostLabel } from "./wireSchema";

const messageOf = (error: unknown) => (error instanceof Error ? error.message : String(error));

// The local-hostname boundary: the single Tauri `invoke` both windows run to
// resolve the local machine's short hostname for the local source's label.
// Injected as a service (the PreflightIpc pattern) so the atom stays pure over
// it and tests can script it without a real Tauri host. `Effect.tryPromise`
// only constructs a description — nothing fires at layer build, preserving the
// side-effect-free runtime-build invariant (see shared.ts / atoms.ts).
export class HostIpc extends Context.Service<HostIpc>()("desktop/HostIpc", {
  make: Effect.succeed({
    localHostLabel: Effect.tryPromise({
      try: () => invoke<unknown>("local_host_label"),
      catch: (error) => new IpcError({ op: "local_host_label", message: messageOf(error) }),
    }).pipe(
      Effect.flatMap((input) =>
        Effect.fromResult(decodeLocalHostLabel(input)).pipe(
          Effect.mapError(
            (error) => new IpcError({ op: "local_host_label", message: error.message }),
          ),
        ),
      ),
    ),
  }),
}) {}

export const layer = Layer.effect(HostIpc, HostIpc.make);
