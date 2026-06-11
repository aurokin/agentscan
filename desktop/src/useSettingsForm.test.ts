// @vitest-environment jsdom
//
// The hook is atom-free (its atom-bound collaborators arrive as plain
// function arguments), so no Tauri mocks are needed. The contract worth a
// render-level test — pinned nowhere else — is the draft-reset keying: the
// effect fires on (activeProfile.id, runnerKey) VALUES, never on object
// identity, because every service commit re-reads storage with all-new
// identities and must not clobber unsaved edits on reorder/open-toggle
// commits. (There is no eslint gate in this package; this test is what
// protects that dep array from a well-meaning "fix" to [activeProfile].)
import { act } from "react";
import { expect, it, vi } from "vitest";
import { renderHook } from "@testing-library/react";
import { useSettingsForm } from "./useSettingsForm";
import type { SshProfileConfig } from "./effect/profileModel";

(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;

const profile = (overrides: Partial<SshProfileConfig> = {}): SshProfileConfig => ({
  id: "ssh-1",
  kind: "ssh",
  host: "box",
  clientTty: "",
  runner: { binaryPath: "", env: [] },
  enabled: true,
  ...overrides,
});

const runnerKeyOf = (p: SshProfileConfig) => `${p.host}|${p.runner.binaryPath}`;

const setup = () => {
  const reloadProfiles = vi.fn();
  const appendDebugEntry = vi.fn();
  const applyRunnerSettingsSet = vi.fn(async () => "applied" as const);
  const initial = profile();
  const view = renderHook(
    ({ activeProfile }: { activeProfile: SshProfileConfig }) =>
      useSettingsForm({
        initialProfile: initial,
        activeProfile,
        profiles: [activeProfile],
        runnerKey: runnerKeyOf(activeProfile),
        labelFor: (p) => p.id,
        appendDebugEntry,
        applyRunnerSettingsSet,
        reloadProfiles,
      }),
    { initialProps: { activeProfile: initial } },
  );
  return { ...view, reloadProfiles, appendDebugEntry, applyRunnerSettingsSet };
};

it("preserves edited drafts across a same-target commit with fresh object identity", () => {
  const { result, rerender } = setup();
  act(() => {
    result.current.setSshHostDraft("elsewhere");
  });
  expect(result.current.isSettingsDirty).toBe(true);

  // A reorder/open-toggle commit: same id + runnerKey, all-new identity.
  rerender({ activeProfile: profile() });
  expect(result.current.sshHostDraft).toBe("elsewhere");
  expect(result.current.isSettingsDirty).toBe(true);
});

it("resets the drafts when the target's runnerKey changes", () => {
  const { result, rerender } = setup();
  act(() => {
    result.current.setSshHostDraft("scratch");
  });

  // An in-place committed edit moves the runnerKey: the form follows it.
  rerender({ activeProfile: profile({ host: "moved" }) });
  expect(result.current.sshHostDraft).toBe("moved");
  expect(result.current.isSettingsDirty).toBe(false);
});

it("reconciles from storage only while clean, and mirrors dirty into the ref render-synchronously", () => {
  const { result, reloadProfiles } = setup();
  // The clean-reconcile effect fires on mount (clean).
  const cleanCalls = reloadProfiles.mock.calls.length;
  expect(cleanCalls).toBeGreaterThan(0);

  act(() => {
    result.current.setSshHostDraft("editing");
  });
  // Dirty: no further reload, and the ref already reads true.
  expect(reloadProfiles.mock.calls.length).toBe(cleanCalls);
  expect(result.current.isSettingsDirtyRef.current).toBe(true);

  act(() => {
    result.current.resetProfileSettings();
  });
  // Clean again: the dirty->clean transition reconciles once more.
  expect(reloadProfiles.mock.calls.length).toBeGreaterThan(cleanCalls);
  expect(result.current.isSettingsDirtyRef.current).toBe(false);
});
