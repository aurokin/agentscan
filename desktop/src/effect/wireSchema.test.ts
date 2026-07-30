import { Result } from "effect";
import { describe, expect, it } from "vitest";
import {
  decodeAgentscanPreflight,
  decodeDaemonPollResult,
  decodeLivePickerEnvelope,
  decodeLivePickerEnvelopeHeader,
  decodeLiveSnapshotSummary,
  decodeLocalHostLabel,
  decodePickerRow,
  decodePickerRows,
  decodePrefsSync,
} from "./wireSchema";

const ROW = {
  key: "1",
  pane_id: "%1",
  provider: "claude",
  status: { kind: "idle" },
  display_label: "Claude",
  location_tag: "work:0.0",
  workspace: { id: "repo-1", label: "agentscan", source: "git_repo" },
  is_active: true,
  is_focused: false,
  attached_client_count: 2,
  last_focus_seq: 42,
};

const SNAPSHOT = {
  paneCount: 1,
  generatedAt: "2026-07-30T00:00:00Z",
  sourceKind: "tmux",
};

const PREFLIGHT = {
  binary: "agentscan",
  ok: true,
  version: "agentscan 0.11.2",
  error: null,
  suggestedBinaryPath: null,
  remoteHostLabel: "koopa",
};

function success<A, E>(result: Result.Result<A, E>): A {
  expect(Result.isSuccess(result)).toBe(true);
  if (Result.isFailure(result)) {
    throw new Error(String(result.failure));
  }
  return result.success;
}

const expectFailure = (result: Result.Result<unknown, unknown>) => {
  expect(Result.isFailure(result)).toBe(true);
};

describe("PickerRow wire schema", () => {
  it("accepts open status kinds and strips excess backend keys recursively", () => {
    const decoded = success(
      decodePickerRow({
        ...ROW,
        status: { kind: "provider-added-state", detail: "ignored" },
        workspace: { ...ROW.workspace, branch: "main" },
        location: { session_name: "work" },
        display: { provider_marker: "C" },
      }),
    );

    expect(decoded.status).toEqual({ kind: "provider-added-state" });
    expect(decoded.workspace).toEqual(ROW.workspace);
    expect(decoded).not.toHaveProperty("location");
    expect(decoded).not.toHaveProperty("display");
  });

  it("preserves optional fields as absent", () => {
    const {
      workspace: _workspace,
      is_focused: _focus,
      attached_client_count: _clients,
      last_focus_seq: _seq,
      ...withoutOptionalFields
    } = ROW;
    const decoded = success(decodePickerRow(withoutOptionalFields));

    expect(decoded).not.toHaveProperty("workspace");
    expect(decoded).not.toHaveProperty("is_focused");
    expect(decoded).not.toHaveProperty("attached_client_count");
    expect(decoded).not.toHaveProperty("last_focus_seq");
  });

  it("accepts partial workspace metadata and a null provider", () => {
    const decoded = success(decodePickerRow({ ...ROW, provider: null, workspace: { label: "repo" } }));
    expect(decoded.provider).toBeNull();
    expect(decoded.workspace).toEqual({ label: "repo" });
  });

  it("retains present focus compatibility fields", () => {
    const decoded = success(decodePickerRow(ROW));
    expect(decoded.is_focused).toBe(false);
    expect(decoded.last_focus_seq).toBe(42);
  });

  it("rejects undefined rather than treating it as an absent compatibility field", () => {
    expectFailure(decodePickerRow({ ...ROW, is_focused: undefined }));
    expectFailure(decodePickerRow({ ...ROW, last_focus_seq: undefined }));
  });

  it("rejects malformed rows and row arrays", () => {
    expectFailure(decodePickerRow({ ...ROW, pane_id: 1 }));
    expectFailure(decodePickerRow({ ...ROW, status: { kind: 1 } }));
    expectFailure(decodePickerRows([ROW, { ...ROW, is_active: "yes" }]));
    expect(success(decodePickerRows([ROW]))).toHaveLength(1);
  });
});

describe("live picker wire schemas", () => {
  it("decodes only the routing fields from an envelope header", () => {
    expect(
      success(
        decodeLivePickerEnvelopeHeader({
          sourceKey: "local",
          epoch: 7,
          kind: "rows",
          rows: "not traversed by the header decoder",
        }),
      ),
    ).toEqual({ sourceKey: "local", epoch: 7 });
    expectFailure(decodeLivePickerEnvelopeHeader({ sourceKey: 1, epoch: 7 }));
  });

  it("validates snapshot summaries", () => {
    expect(success(decodeLiveSnapshotSummary({ ...SNAPSHOT, backendDetail: true }))).toEqual(
      SNAPSHOT,
    );
    expectFailure(decodeLiveSnapshotSummary({ ...SNAPSHOT, paneCount: "1" }));
    expectFailure(decodeLiveSnapshotSummary({ ...SNAPSHOT, generatedAt: 1 }));
  });

  it.each([
    ["connecting", { kind: "connecting", message: "Connecting" }],
    ["rows", { kind: "rows", rows: [ROW], snapshot: SNAPSHOT }],
    [
      "offline",
      { kind: "offline", message: "Unavailable", retrying: true, diagnostics: { code: 1 } },
    ],
    ["shutdown", { kind: "shutdown", message: "Stopped" }],
    ["fatal", { kind: "fatal", message: "Incompatible", diagnostics: null }],
  ])("decodes the %s event arm", (kind, event) => {
    const decoded = success(
      decodeLivePickerEnvelope({ sourceKey: "local", epoch: 7, ignored: true, ...event }),
    );
    expect(decoded.kind).toBe(kind);
    expect(decoded).not.toHaveProperty("ignored");
  });

  it("rejects malformed known events", () => {
    expectFailure(
      decodeLivePickerEnvelope({
        sourceKey: "local",
        epoch: 1,
        kind: "rows",
        rows: [{ ...ROW, key: false }],
        snapshot: SNAPSHOT,
      }),
    );
    expectFailure(
      decodeLivePickerEnvelope({
        sourceKey: "local",
        epoch: "1",
        kind: "connecting",
        message: "Connecting",
      }),
    );
  });

  it("rejects unknown event kinds so listeners can drop them", () => {
    expectFailure(
      decodeLivePickerEnvelope({
        sourceKey: "local",
        epoch: 1,
        kind: "future-event",
        message: "Newer backend",
      }),
    );
  });
});

describe("command response wire schemas", () => {
  it("validates and strips preflight responses", () => {
    expect(success(decodeAgentscanPreflight({ ...PREFLIGHT, ignored: "backend detail" }))).toEqual(
      PREFLIGHT,
    );

    for (const malformed of [
      { ...PREFLIGHT, binary: null },
      { ...PREFLIGHT, ok: "yes" },
      { ...PREFLIGHT, version: 1 },
      { ...PREFLIGHT, suggestedBinaryPath: false },
      { ...PREFLIGHT, remoteHostLabel: 12 },
    ]) {
      expectFailure(decodeAgentscanPreflight(malformed));
    }
  });

  it("validates daemon poll and local host label responses", () => {
    expect(success(decodeDaemonPollResult({ reachable: true, detail: "ignored" }))).toEqual({
      reachable: true,
    });
    expect(success(decodeLocalHostLabel("koopa"))).toBe("koopa");
    expectFailure(decodeDaemonPollResult({ reachable: "true" }));
    expectFailure(decodeLocalHostLabel({ label: "koopa" }));
  });
});

describe("PrefsSync wire schema", () => {
  it.each([
    ["theme", { kind: "theme", theme: "system" }],
    ["orientation", { kind: "orientation", orientation: "horizontal" }],
    ["glass", { kind: "glass", enabled: true, alpha: 0.8 }],
    ["frameless", { kind: "frameless", enabled: false }],
    ["notifyOnIdle", { kind: "notifyOnIdle", enabled: true }],
    ["profiles", { kind: "profiles" }],
    [
      "preflight",
      { kind: "preflight", status: "ready", runnerKey: "local", preflight: PREFLIGHT },
    ],
    ["preflight-request", { kind: "preflight-request" }],
  ])("decodes the %s arm", (kind, payload) => {
    const decoded = success(decodePrefsSync({ ...payload, ignored: true }));
    expect(decoded.kind).toBe(kind);
    expect(decoded).not.toHaveProperty("ignored");
  });

  it("enforces the preflight status/payload invariant", () => {
    for (const status of ["loading", "failed"] as const) {
      expect(
        success(
          decodePrefsSync({ kind: "preflight", status, runnerKey: "local", preflight: null }),
        ),
      ).toMatchObject({ status, preflight: null });
      expectFailure(
        decodePrefsSync({ kind: "preflight", status, runnerKey: "local", preflight: PREFLIGHT }),
      );
    }
    expectFailure(
      decodePrefsSync({ kind: "preflight", status: "ready", runnerKey: "local", preflight: null }),
    );
  });

  it("rejects unknown kinds, invalid preference literals, and malformed payloads", () => {
    expectFailure(decodePrefsSync({ kind: "future-pref", enabled: true }));
    expectFailure(decodePrefsSync({ kind: "theme", theme: "sepia" }));
    expectFailure(decodePrefsSync({ kind: "orientation", orientation: "diagonal" }));
    expectFailure(decodePrefsSync({ kind: "glass", enabled: true, alpha: "0.8" }));
    expectFailure(
      decodePrefsSync({
        kind: "preflight",
        status: "ready",
        runnerKey: "local",
        preflight: { ...PREFLIGHT, ok: 1 },
      }),
    );
  });
});
