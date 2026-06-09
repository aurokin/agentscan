// Shared types for the live-connection slice. These mirror the Rust contracts in
// src-tauri/src/lib.rs (PickerRow, LivePickerEvent/Envelope) and the runner
// settings sent to the commands. App.tsx imports the picker/runner shapes from
// here so there is a single source of truth across the IPC boundary.

export type EnvironmentVariable = { name: string; value: string };

// Runner settings forwarded verbatim to the Rust commands (tagged union matching
// DesktopRunnerSettings on the backend). The desktop layer treats it as opaque —
// App.tsx builds it from the active profile.
export type DesktopRunnerSettings =
  | { kind: "local"; binaryPath: string; env: EnvironmentVariable[] }
  | {
      kind: "ssh";
      host: string;
      clientTty: string | null;
      binaryPath: string;
      env: EnvironmentVariable[];
    };

export type PickerRow = {
  key: string;
  pane_id: string;
  provider: string | null;
  status: { kind: string };
  display_label: string;
  location_tag: string;
  workspace?: { id?: string; label?: string; source?: string };
  is_active: boolean;
  is_focused?: boolean;
  attached_client_count?: number;
};

export type LiveSnapshotSummary = {
  paneCount: number;
  generatedAt: string | null;
  sourceKind: string | null;
};

// The raw event the Rust worker emits over the Tauri event bus, stamped with the
// epoch of the subscription that produced it (see LivePickerEnvelope on the
// backend). The frontend folds these into ConnectionStatus + rows.
export type LivePickerEvent =
  | { kind: "connecting"; message: string }
  | { kind: "rows"; rows: PickerRow[]; snapshot: LiveSnapshotSummary }
  | { kind: "offline"; message: string; retrying: boolean; diagnostics: unknown | null }
  | { kind: "shutdown"; message: string }
  | { kind: "fatal"; message: string; diagnostics: unknown | null };

export type LivePickerEnvelope = LivePickerEvent & { epoch: number };

// The connection status the dock renders. `noDaemon` is new to the Effect slice:
// the dock latches onto an existing daemon and never starts one itself, so when
// none is reachable it offers an explicit "Start agentscan" action instead of
// wedging on the old "shutdown" terminal state.
export type ConnectionStatus =
  | { status: "connecting"; message: string }
  | { status: "online"; message: string; snapshot: LiveSnapshotSummary }
  | { status: "reconnecting"; message: string }
  | { status: "noDaemon"; message: string }
  | { status: "fatal"; message: string };

// The full live state owned by the LiveConnection service: the connection status
// plus the latest picker rows (rows arrive on the same live stream).
export type LiveState = {
  connection: ConnectionStatus;
  rows: PickerRow[];
  // The runnerKey of the subscription that produced `rows`. The dock gates rendering
  // on this matching the active runner: after a source switch the service preserves
  // the previous runner's rows during the new subscription's connecting window (to
  // avoid a same-runner reconnect flicker), and those stale rows must not be shown or
  // activated against the new runner's settings. null when there are no rows.
  rowsRunnerKey: string | null;
};
