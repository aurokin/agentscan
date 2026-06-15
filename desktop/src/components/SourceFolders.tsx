import { GroupedPicker } from "./GroupedPicker";
import { LiveStrip } from "./LiveStrip";
import { SourceKindIcon } from "./SourceKindIcon";
import { HOTKEY_MODIFIER_LABEL } from "../platform";
import {
  connectionTone,
  type PickerActivation,
  type SourceView,
} from "../effect/pickerViewModel";
import type { LogoTheme } from "../providerLogos";
import type { DesktopProfileConfig } from "../effect/profileModel";
import type { PickerRow } from "../effect/types";

// The structural slice of the dock's liveSources each folder needs; the dock's
// fuller source shape satisfies it.
type FolderSource = {
  profile: DesktopProfileConfig;
  runnerKey: string;
  isOpen: boolean;
  isOwner: boolean;
};

// The vertical strip's list of host folders: one collapsible section per
// enabled source, in the user's order. Open = live subscription + that
// source's workspace-grouped rows; closed = header only, no subscription.
// All state stays in the dock; this renders the per-folder composition
// (header dot/label/kbd, the preflight-error strips, LiveStrip, GroupedPicker).
export function SourceFolders({
  sourceViews,
  activation,
  pickerFilter,
  selectedPaneId,
  resolvedTheme,
  runnerKey,
  preflightError,
  activeProfile,
  labelFor,
  onOpenSettings,
  onToggleFolder,
  onActivate,
  onSelect,
  onStart,
  onReconnect,
  onClearFilter,
}: {
  sourceViews: SourceView<FolderSource>[];
  activation: PickerActivation;
  pickerFilter: string;
  selectedPaneId: string | null;
  resolvedTheme: LogoTheme;
  runnerKey: string;
  preflightError: string | null;
  activeProfile: DesktopProfileConfig;
  // Passed down, never recreated here: it closes over the dock's hostname/
  // preflight label sources.
  labelFor: (profile: DesktopProfileConfig) => string;
  onOpenSettings: () => void;
  onToggleFolder: (profileId: string) => void;
  onActivate: (row: PickerRow, profile: DesktopProfileConfig) => void;
  onSelect: (row: PickerRow) => void;
  onStart: (runnerKey: string) => void;
  onReconnect: (runnerKey: string) => void;
  onClearFilter: () => void;
}) {
  return (
    <div className="source-folders">
      {preflightError !== null && !sourceViews.some((view) => view.runnerKey === runnerKey) ? (
        // The active source can be folder-INeligible (e.g. a just-added remote
        // with no host yet): it renders no folder, and with another folder open
        // the boot screen is suppressed, so without this strip its failure has
        // no surface at all. Same recovery shape as the in-folder strip.
        <div className="live-strip error" aria-live="polite">
          <span className="status-dot" data-tone="error" />
          <span className="live-label">{labelFor(activeProfile)}</span>
          <span className="live-message">{preflightError}</span>
          <button className="live-action" type="button" onClick={onOpenSettings}>
            Open settings
          </button>
        </div>
      ) : null}
      {sourceViews.map((view) => {
        // The active source's resolved-failing preflight surfaces in its own
        // folder: its live target is gated off on a failed probe, so the keyed
        // connection (a perpetual "Waiting for a source") would lie about what
        // broke. Non-active sources are never probed; theirs stays null.
        const folderPreflightError = view.runnerKey === runnerKey ? preflightError : null;
        return (
          <section className="source-folder" key={view.profile.id}>
            <button
              className="folder-header"
              type="button"
              aria-expanded={view.isOpen}
              onClick={() => onToggleFolder(view.profile.id)}
              title={
                folderPreflightError ??
                (view.isOpen ? view.live.connection.message : "Closed — no live subscription")
              }
            >
              <span
                className={`status-dot${
                  folderPreflightError === null &&
                  view.isOpen &&
                  (view.live.connection.status === "connecting" ||
                    view.live.connection.status === "reconnecting")
                    ? " pulsing"
                    : ""
                }`}
                data-tone={
                  folderPreflightError !== null
                    ? "error"
                    : view.isOpen
                      ? connectionTone(view.live.connection)
                      : "unknown"
                }
                aria-hidden="true"
              />
              <span className="folder-mark" aria-hidden="true">
                <SourceKindIcon kind={view.profile.kind} />
              </span>
              <span className="folder-label">{labelFor(view.profile)}</span>
              {view.isOwner ? (
                <kbd className="folder-kbd" title="Row hotkeys target this source">
                  {HOTKEY_MODIFIER_LABEL.trim()}
                </kbd>
              ) : null}
              <span className={`folder-caret${view.isOpen ? " open" : ""}`} aria-hidden="true">
                {"›"}
              </span>
            </button>
            {view.isOpen ? (
              <div className="folder-body">
                {folderPreflightError !== null ? (
                  // The gated-off target's keyed state (connecting, no rows) would
                  // render a perpetual loading skeleton under this, so the strip
                  // replaces the picker body too. Mirrors the boot screen's
                  // recovery path; LiveStrip's Start/Reconnect can't fix a
                  // preflight failure, so it doesn't render here.
                  <div className="live-strip error" aria-live="polite">
                    <span className="status-dot" data-tone="error" />
                    <span className="live-label">Unavailable</span>
                    <span className="live-message">{folderPreflightError}</span>
                    <button className="live-action" type="button" onClick={onOpenSettings}>
                      Open settings
                    </button>
                  </div>
                ) : (
                  <>
                    {/* This source's own failed activation; source-less
                        failures use the global surface above the folders. */}
                    {activation.status === "failed" &&
                    activation.sourceKey === view.runnerKey ? (
                      <div className="inline-error" role="alert">
                        {activation.message}
                      </div>
                    ) : null}
                    {view.live.connection.status !== "online" ? (
                      <LiveStrip
                        status={view.live.connection}
                        onStart={() => onStart(view.runnerKey)}
                        onReconnect={() => onReconnect(view.runnerKey)}
                      />
                    ) : null}
                    <GroupedPicker
                      activation={activation}
                      connectionOffline={view.live.connection.status !== "online"}
                      filterQuery={pickerFilter}
                      focusedPaneId={view.focusedPaneId}
                      groups={view.groups}
                      keybindsOwned={view.isOwner}
                      logoTheme={resolvedTheme}
                      selectedPaneId={view.isOwner ? selectedPaneId : null}
                      sourceKey={view.runnerKey}
                      state={view.state}
                      totalRows={view.allRows.length}
                      onActivate={(row) => onActivate(row, view.profile)}
                      onClearFilter={onClearFilter}
                      onSelect={(row) => {
                        // Selection (the keyboard cursor) is owner-scoped; clicks on
                        // other folders activate without moving it.
                        if (view.isOwner) {
                          onSelect(row);
                        }
                      }}
                    />
                  </>
                )}
              </div>
            ) : null}
          </section>
        );
      })}
    </div>
  );
}
