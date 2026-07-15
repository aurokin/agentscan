import { providerLogo, type LogoTheme } from "../providerLogos";
import type { PickerRow } from "../effect/types";
import {
  paneSuffix,
  statusTone,
  type PickerActivation,
  type PickerGroup,
  type PickerState,
} from "../effect/pickerViewModel";
import { HOTKEY_MODIFIER_LABEL } from "../platform";

export function GroupedPicker({
  activation,
  connectionOffline = false,
  filterQuery,
  focusedPaneId,
  groups,
  keybindsOwned,
  logoTheme,
  selectedPaneId,
  sourceKey,
  state,
  totalRows,
  onActivate,
  onClearFilter,
  onSelect,
}: {
  activation: PickerActivation;
  // True when this source's connection is non-online (e.g. no daemon), so a
  // LiveStrip above is already stating why there are no rows. Suppresses the
  // resolved-empty placeholder, which would otherwise imply a successful empty scan.
  connectionOffline?: boolean;
  filterQuery: string;
  focusedPaneId: string | null;
  groups: PickerGroup[];
  // Whether this source owns the row keybinds (Ctrl+<key>). Non-owners render
  // their <kbd> labels dimmed, as information only.
  keybindsOwned: boolean;
  logoTheme: LogoTheme;
  // The app-level selection, already scoped to this source by the caller:
  // SourceFolders passes null to non-owner folders and the horizontal bar
  // renders only the owner's picker, so at most one rendered GroupedPicker
  // ever holds a non-null value. Pane ids collide across hosts — callers must
  // keep that scoping, because a non-null id is treated as this source's own
  // selection (aria-selected, aria-activedescendant).
  selectedPaneId: string | null;
  // This source's runnerKey; scopes the activation pulse (pane ids collide across hosts).
  sourceKey: string;
  state: PickerState;
  totalRows: number;
  onActivate: (row: PickerRow) => void;
  onClearFilter: () => void;
  onSelect: (row: PickerRow) => void;
}) {
  const rowCount = groups.reduce((total, group) => total + group.rows.length, 0);
  // DOM id for a row's option element, unique across sources (pane ids collide
  // across hosts) and safe as an id/aria-activedescendant reference. The escape
  // is injective — every char outside [A-Za-z0-9-] (including "_", the escape
  // introducer) becomes _hex_ — so distinct source keys like "ssh-a.b" and
  // "ssh-a-b" can never produce the same id.
  const idSafe = (value: string) =>
    value.replace(/[^A-Za-z0-9-]/g, (ch) => `_${ch.codePointAt(0)?.toString(16) ?? ""}_`);
  const optionId = (paneId: string) => `agent-option-${idSafe(sourceKey)}-${idSafe(paneId)}`;
  const selectedRowInThisSource =
    selectedPaneId !== null &&
    groups.some((group) => group.rows.some((row) => row.pane_id === selectedPaneId));

  if (state.status === "loading" && rowCount === 0) {
    return <p className="empty-note">Loading agents…</p>;
  }

  if (state.status === "failed") {
    return (
      <div className="error-state" role="alert">
        <h3>Unable to load agents</h3>
        <p>{state.message}</p>
      </div>
    );
  }

  if (totalRows > 0 && rowCount === 0 && filterQuery.trim()) {
    return (
      <div className="empty-filter-state">
        <p>No agents match “{filterQuery.trim()}”.</p>
        <button className="ghost-button" type="button" onClick={onClearFilter}>
          Clear search
        </button>
      </div>
    );
  }

  if (rowCount === 0) {
    // A non-online connection (no daemon, etc.) already shows a LiveStrip above
    // stating why there are no rows; a second "No agents here" would imply we
    // looked and found none. Let the strip own that message.
    if (connectionOffline) {
      return null;
    }
    return (
      <div className="empty-detected" role="status">
        <span className="empty-marker" aria-hidden="true" />
        <p>No agents here</p>
      </div>
    );
  }

  return (
    /* One listbox per source, spanning its project groups — the selection
       cursor moves across groups, so the composite widget is the whole picker,
       not each <ul>. aria-selected is only valid on selectable roles, hence
       option items. The listbox is a tab stop whose aria-activedescendant
       tracks the app-level selection; Arrow Up/Down and Enter already operate
       it through the window-level key handler (keybinds.ts). */
    <div
      className="picker-groups"
      role="listbox"
      // Only the keybind owner's picker is a tab stop: the window-level key
      // handler always drives the OWNER's selection, so focusing a non-owner
      // listbox would advertise keyboard control it does not have. Non-owner
      // pickers stay browsable to assistive tech without taking focus.
      tabIndex={keybindsOwned ? 0 : undefined}
      aria-label="Agent panes"
      aria-activedescendant={
        selectedRowInThisSource && selectedPaneId !== null ? optionId(selectedPaneId) : undefined
      }
    >
      {groups.map((group) => (
        <section className="picker-group" role="group" aria-label={group.project} key={group.key}>
          {/* Presentational: the group's accessible name rides the section's
              aria-label — a heading is not valid inside a listbox. */}
          <h2 className="group-header" role="presentation">
            {group.project}
          </h2>
          <ul className="agent-list" role="presentation">
            {group.rows.map((row) => {
              const isSelected = row.pane_id === selectedPaneId;
              // The single live pane the user is in. The selection cursor follows
              // it, so in the common case the two coincide and the selection ring
              // sits on the live pane. When they diverge (manual j/k/click away),
              // a faint "live" ring keeps the live pane discoverable. Derived from
              // the same resolved id as the cursor so the legacy `is_active`
              // fallback stays single-row and consistent.
              const isFocused = row.pane_id === focusedPaneId;
              const isFocusing =
                activation.status === "running" &&
                activation.sourceKey === sourceKey &&
                activation.paneId === row.pane_id;
              const logo = providerLogo(row.provider, logoTheme);
              return (
                <li
                  role="option"
                  id={optionId(row.pane_id)}
                  aria-selected={isSelected}
                  aria-current={isFocused ? "true" : undefined}
                  className={`agent-row${isSelected ? " selected" : ""}${
                    isFocused && !isSelected ? " live" : ""
                  }`}
                  key={`${row.key}-${row.pane_id}`}
                  onClick={() => {
                    // Single-click selects and switches the active tmux pane.
                    // Enter still activates the keyboard selection; double-click
                    // is gone (redundant under single-click activation).
                    onSelect(row);
                    onActivate(row);
                  }}
                  title={`${row.display_label} · ${row.provider ?? "unknown"} · ${row.location_tag}`}
                >
                  <span
                    className={`status-dot${isFocusing ? " pulsing" : ""}`}
                    data-tone={isFocusing ? "busy" : statusTone(row.status.kind)}
                    aria-hidden="true"
                  />
                  {logo ? (
                    <img className="provider-logo" src={logo} alt="" aria-hidden="true" />
                  ) : (
                    <span className="provider-logo provider-logo-empty" aria-hidden="true" />
                  )}
                  <span className="agent-label">{row.display_label}</span>
                  <span className="agent-suffix">{paneSuffix(row)}</span>
                  <kbd className={keybindsOwned ? undefined : "dimmed"}>
                    <span className="kbd-mod">{HOTKEY_MODIFIER_LABEL}</span>
                    {row.key}
                  </kbd>
                </li>
              );
            })}
          </ul>
        </section>
      ))}
    </div>
  );
}
