import { Schema } from "effect";
import type { PrefsSync } from "./prefs";
import type { AgentscanPreflight } from "./profileModel";
import type {
  DaemonPollResult,
  LivePickerEnvelope,
  LiveSnapshotSummary,
  PickerRow,
} from "./types";

const NullableStringSchema = Schema.NullOr(Schema.String);

const WorkspaceSchema = Schema.Struct({
  id: Schema.optionalKey(Schema.String),
  label: Schema.optionalKey(Schema.String),
  source: Schema.optionalKey(Schema.String),
});

export const PickerRowWireSchema = Schema.Struct({
  key: Schema.String,
  pane_id: Schema.String,
  provider: NullableStringSchema,
  status: Schema.Struct({ kind: Schema.String }),
  display_label: Schema.String,
  location_tag: Schema.String,
  workspace: Schema.optionalKey(WorkspaceSchema),
  is_active: Schema.Boolean,
  is_focused: Schema.optionalKey(Schema.Boolean),
  attached_client_count: Schema.optionalKey(Schema.Number),
  last_focus_seq: Schema.optionalKey(Schema.Number),
});

const PickerRowsWireSchema = Schema.mutable(Schema.Array(PickerRowWireSchema));

export const LiveSnapshotSummaryWireSchema = Schema.Struct({
  paneCount: Schema.Number,
  generatedAt: NullableStringSchema,
  sourceKind: NullableStringSchema,
});

const LiveEnvelopeFields = {
  sourceKey: Schema.String,
  epoch: Schema.Number,
};

// The shared Tauri event channel has one listener per configured source. Decode
// this small routing header first so only the owning listener traverses and
// rebuilds a potentially large rows payload.
export const LivePickerEnvelopeHeaderWireSchema = Schema.Struct(LiveEnvelopeFields);

export const LivePickerEnvelopeWireSchema = Schema.Union([
  Schema.Struct({
    ...LiveEnvelopeFields,
    kind: Schema.Literal("connecting"),
    message: Schema.String,
  }),
  Schema.Struct({
    ...LiveEnvelopeFields,
    kind: Schema.Literal("rows"),
    rows: PickerRowsWireSchema,
    snapshot: LiveSnapshotSummaryWireSchema,
  }),
  Schema.Struct({
    ...LiveEnvelopeFields,
    kind: Schema.Literal("offline"),
    message: Schema.String,
    retrying: Schema.Boolean,
    diagnostics: Schema.NullOr(Schema.Unknown),
  }),
  Schema.Struct({
    ...LiveEnvelopeFields,
    kind: Schema.Literal("shutdown"),
    message: Schema.String,
  }),
  Schema.Struct({
    ...LiveEnvelopeFields,
    kind: Schema.Literal("fatal"),
    message: Schema.String,
    diagnostics: Schema.NullOr(Schema.Unknown),
  }),
]);

export const AgentscanPreflightWireSchema = Schema.Struct({
  binary: Schema.String,
  ok: Schema.Boolean,
  version: NullableStringSchema,
  error: NullableStringSchema,
  suggestedBinaryPath: NullableStringSchema,
  remoteHostLabel: NullableStringSchema,
});

export const DaemonPollResultWireSchema = Schema.Struct({ reachable: Schema.Boolean });
export const LocalHostLabelWireSchema = Schema.String;

const ThemePreferenceSchema = Schema.Literals(["dark", "light", "system"]);
const OrientationPreferenceSchema = Schema.Literals(["auto", "vertical", "horizontal"]);
const PendingPreflightStatusSchema = Schema.Literals(["loading", "failed"]);

export const PrefsSyncWireSchema = Schema.Union([
  Schema.Struct({ kind: Schema.Literal("theme"), theme: ThemePreferenceSchema }),
  Schema.Struct({
    kind: Schema.Literal("orientation"),
    orientation: OrientationPreferenceSchema,
  }),
  Schema.Struct({ kind: Schema.Literal("glass"), enabled: Schema.Boolean, alpha: Schema.Number }),
  Schema.Struct({ kind: Schema.Literal("frameless"), enabled: Schema.Boolean }),
  Schema.Struct({ kind: Schema.Literal("notifyOnIdle"), enabled: Schema.Boolean }),
  Schema.Struct({ kind: Schema.Literal("profiles") }),
  Schema.Struct({
    kind: Schema.Literal("preflight"),
    status: Schema.Literal("ready"),
    runnerKey: Schema.String,
    preflight: AgentscanPreflightWireSchema,
  }),
  Schema.Struct({
    kind: Schema.Literal("preflight"),
    status: PendingPreflightStatusSchema,
    runnerKey: Schema.String,
    preflight: Schema.Null,
  }),
  Schema.Struct({ kind: Schema.Literal("preflight-request") }),
]);

export type DecodedPickerRow = typeof PickerRowWireSchema.Type;
export type DecodedLiveSnapshotSummary = typeof LiveSnapshotSummaryWireSchema.Type;
export type DecodedLivePickerEnvelope = typeof LivePickerEnvelopeWireSchema.Type;
export type DecodedAgentscanPreflight = typeof AgentscanPreflightWireSchema.Type;
export type DecodedDaemonPollResult = typeof DaemonPollResultWireSchema.Type;
export type DecodedLocalHostLabel = typeof LocalHostLabelWireSchema.Type;
export type DecodedPrefsSync = typeof PrefsSyncWireSchema.Type;

export const decodePickerRow = Schema.decodeUnknownResult(PickerRowWireSchema);
export const decodePickerRows = Schema.decodeUnknownResult(PickerRowsWireSchema);
export const decodeLiveSnapshotSummary = Schema.decodeUnknownResult(LiveSnapshotSummaryWireSchema);
export const decodeLivePickerEnvelopeHeader = Schema.decodeUnknownResult(
  LivePickerEnvelopeHeaderWireSchema,
);
export const decodeLivePickerEnvelope = Schema.decodeUnknownResult(LivePickerEnvelopeWireSchema);
export const decodeAgentscanPreflight = Schema.decodeUnknownResult(AgentscanPreflightWireSchema);
export const decodeDaemonPollResult = Schema.decodeUnknownResult(DaemonPollResultWireSchema);
export const decodeLocalHostLabel = Schema.decodeUnknownResult(LocalHostLabelWireSchema);
export const decodePrefsSync = Schema.decodeUnknownResult(PrefsSyncWireSchema);

type Assert<T extends true> = T;
type SameContract<A, B> = [A] extends [B] ? ([B] extends [A] ? true : false) : false;

// Exporting one type-only tuple makes every assertion an intentional declaration
// while still failing compilation if a wire schema drifts from its domain type.
export type WireContractChecks = readonly [
  Assert<SameContract<DecodedPickerRow, PickerRow>>,
  Assert<SameContract<DecodedLiveSnapshotSummary, LiveSnapshotSummary>>,
  Assert<SameContract<DecodedLivePickerEnvelope, LivePickerEnvelope>>,
  Assert<SameContract<DecodedAgentscanPreflight, AgentscanPreflight>>,
  Assert<SameContract<DecodedDaemonPollResult, DaemonPollResult>>,
  Assert<SameContract<DecodedLocalHostLabel, string>>,
  Assert<SameContract<DecodedPrefsSync, PrefsSync>>,
];
