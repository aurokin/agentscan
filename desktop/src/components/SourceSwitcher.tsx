import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { SourceKindIcon } from "./SourceKindIcon";
import type { DesktopProfileConfig } from "../effect/profileModel";
import type { Orientation } from "../effect/prefs";

export type SourceMenuItem = {
  profile: DesktopProfileConfig;
  enabled: boolean;
  canToggle: boolean;
  isOwner: boolean;
};

// The footer's source trigger + the inline order menu. The menu's open/drag
// state lives HERE (the one deliberate state move of the component cuts): it
// is purely presentational, and every dismiss path references only this
// component's scope. One behavior delta from the move, both deliberate:
// unmounting (the boot takeover) now drops an open menu, so it reopens closed
// when the footer returns — previously the dock-held state survived and the
// menu popped back open. And the dock's openSettings no longer force-closes
// the menu, so a settings gear triggered purely by keyboard (Enter/Space — no
// mousedown for the outside-dismiss to see) leaves the menu open behind the
// settings window; an obscure pointer-free path, accepted.
export function SourceSwitcher({
  sourceMenuItems,
  triggerProfile,
  triggerShowsSource,
  triggerTone,
  triggerTitle,
  orientation,
  labelFor,
  selectProfile,
  reorderProfile,
  setProfileEnabled,
  onOpenSettings,
}: {
  sourceMenuItems: ReadonlyArray<SourceMenuItem>;
  triggerProfile: DesktopProfileConfig;
  triggerShowsSource: boolean;
  triggerTone: string;
  triggerTitle: string;
  orientation: Orientation;
  // Passed down, never recreated here: it closes over the dock's hostname/
  // preflight label sources.
  labelFor: (profile: DesktopProfileConfig) => string;
  // Promise mode: the horizontal deep-link awaits the commit before opening
  // settings so the window can't load the old selection.
  selectProfile: (id: string) => Promise<unknown>;
  reorderProfile: (input: { id: string; targetId: string }) => void;
  setProfileEnabled: (input: { id: string; enabled: boolean }) => void;
  onOpenSettings: () => void;
}) {
  // Which agentscan we're listening to (local vs a remote over SSH). Open
  // state for the inline dropdown.
  const [isSourceMenuOpen, setIsSourceMenuOpen] = useState(false);
  // Mid-drag source id for the order menu (the counterpart of the settings
  // rail's draggedSourceId).
  const [draggedMenuSourceId, setDraggedMenuSourceId] = useState<string | null>(null);
  const [menuFrame, setMenuFrame] = useState<{
    bottom: number;
    left: number;
    width: number;
  } | null>(null);
  const draggedMenuSourceIdRef = useRef<string | null>(null);
  const sourceMenuRef = useRef<HTMLDivElement | null>(null);
  const sourceMenuPopupRef = useRef<HTMLDivElement | null>(null);

  function updateMenuFrame() {
    const rect = sourceMenuRef.current?.getBoundingClientRect();
    if (!rect) {
      return;
    }
    const width = Math.min(Math.max(rect.width, 340), window.innerWidth - 20);
    setMenuFrame({
      bottom: window.innerHeight - rect.top + 6,
      left: Math.min(Math.max(10, rect.left), Math.max(10, window.innerWidth - width - 10)),
      width,
    });
  }

  // A wide drag or pinning to horizontal can strand an already-open source menu in
  // the thin bar (where it clips). Close it whenever the layout goes horizontal.
  useEffect(() => {
    if (orientation === "horizontal") {
      setIsSourceMenuOpen(false);
      setMenuFrame(null);
    }
  }, [orientation]);

  useEffect(() => {
    if (!isSourceMenuOpen) {
      return;
    }
    updateMenuFrame();
    window.addEventListener("resize", updateMenuFrame);
    return () => window.removeEventListener("resize", updateMenuFrame);
  }, [isSourceMenuOpen]);

  // Dismiss the source dropdown on an outside click or Escape. The keydown is
  // captured so it closes the menu before the picker's global Escape handler
  // hides the whole window (capture beats bubble regardless of which component
  // registered first).
  useEffect(() => {
    if (!isSourceMenuOpen) {
      return;
    }

    function onPointerDown(event: MouseEvent) {
      const target = event.target as Node;
      const isInsideTrigger = sourceMenuRef.current?.contains(target) ?? false;
      const isInsideMenu = sourceMenuPopupRef.current?.contains(target) ?? false;
      if (!isInsideTrigger && !isInsideMenu) {
        setIsSourceMenuOpen(false);
        setMenuFrame(null);
      }
    }

    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.stopPropagation();
        event.preventDefault();
        setIsSourceMenuOpen(false);
        setMenuFrame(null);
      }
    }

    window.addEventListener("mousedown", onPointerDown);
    window.addEventListener("keydown", onKeyDown, true);
    return () => {
      window.removeEventListener("mousedown", onPointerDown);
      window.removeEventListener("keydown", onKeyDown, true);
    };
  }, [isSourceMenuOpen]);

  return (
    <div className="source-switcher" ref={sourceMenuRef}>
      <button
        className="source-trigger"
        type="button"
        aria-haspopup="menu"
        aria-expanded={isSourceMenuOpen}
        onClick={() => {
          // The inline menu opens upward and would clip inside the thin
          // horizontal bar, so there the trigger re-docks into settings (which
          // owns the full Sources list) instead of popping a cramped menu.
          // Pre-select the source this trigger advertises (the owner can differ
          // from the settings-selected active profile): landing in Settings on a
          // different source than the label promised would manage the wrong one.
          // This is a deep-link INTO the settings selection, not a dock-side
          // quick-switch — the order menu below still never selects. The
          // retarget is deliberate and cheap: the probe moves to the source the
          // bar is DISPLAYING (open, so an online channel stays armed), no
          // subscription churns, and a user after the previous selection is one
          // card-click away. Await the commit so the window can't load the old
          // selection; open regardless of the outcome (Settings is the goal).
          // A DIRTY settings window deliberately wins over this deep-link: it
          // skips inbound syncs to protect unsaved edits, and its Apply/Delete
          // target the window's own ref (which mirrors the form), so no action
          // can hit a different source than the one its form displays. The
          // label/form mismatch resolves on the form's apply or reset.
          if (orientation === "horizontal") {
            void selectProfile(triggerProfile.id)
              .catch(() => {})
              .then(() => onOpenSettings());
          } else {
            // Vertical: only the order menu toggles — no selection happens
            // on this branch (the deep-link above is horizontal-exclusive,
            // where the trigger's label names exactly one source).
            setIsSourceMenuOpen((open) => {
              if (open) {
                setMenuFrame(null);
                return false;
              }
              updateMenuFrame();
              return true;
            });
          }
        }}
        title={
          triggerShowsSource
            ? triggerTitle
            : "Show, hide, or reorder sources — the top open source owns row hotkeys"
        }
      >
        {triggerShowsSource ? (
          <span className="status-dot" data-tone={triggerTone} aria-hidden="true" />
        ) : null}
        <span className="source-label">
          {triggerShowsSource ? labelFor(triggerProfile) : "Manage sources"}
        </span>
      </button>
      {isSourceMenuOpen
        ? createPortal(
            // Ordering + visibility surface: drag rows to reorder sources. SSH
            // checkboxes enable/disable a source without deleting its
            // configuration. The topmost open enabled source owns row hotkeys.
            //
            // Deliberately NOT a quick-switch: the dock never changes the
            // active source. The old single-select footer existed because the
            // dock could show one source at a time; folders replace that
            // gesture with the open set. "Active" now only means the
            // settings-edit selection + the single preflight target, and it
            // changes in Settings.
            <div
              className="source-menu"
              ref={sourceMenuPopupRef}
              role="menu"
              style={
                menuFrame
                  ? {
                      bottom: menuFrame.bottom,
                      left: menuFrame.left,
                      width: menuFrame.width,
                    }
                  : undefined
              }
            >
              {/* Portaled out of the footer drag region so native window drags
                  cannot swallow menu row interactions. */}
          {sourceMenuItems.map(({ profile, enabled, canToggle, isOwner }) => (
            <div
              className={`source-option source-row${isOwner ? " owner" : ""}${
                enabled ? "" : " disabled"
              }${
                draggedMenuSourceId === profile.id ? " dragging" : ""
              }`}
              data-source-id={profile.id}
              key={profile.id}
              role="menuitem"
            >
              <label
                className="source-toggle"
                title={canToggle ? undefined : "Local source is always shown"}
                onMouseDown={(event) => event.stopPropagation()}
                onClick={(event) => event.stopPropagation()}
              >
                <input
                  type="checkbox"
                  checked={enabled}
                  disabled={!canToggle}
                  aria-label={`${enabled ? "Hide" : "Show"} ${labelFor(profile)} in dock`}
                  onChange={(event) =>
                    setProfileEnabled({
                      id: profile.id,
                      enabled: event.currentTarget.checked,
                    })
                  }
                />
              </label>
              <span className="source-option-mark" aria-hidden="true">
                <SourceKindIcon kind={profile.kind} />
              </span>
              <span className="source-option-text">
                <span className="source-option-name">{labelFor(profile)}</span>
              </span>
              <button
                className="source-grip"
                type="button"
                aria-label={`Drag ${labelFor(profile)} to reorder`}
                onPointerDown={(event) => {
                  if (event.button !== 0) {
                    return;
                  }
                  event.preventDefault();
                  event.currentTarget.setPointerCapture(event.pointerId);
                  draggedMenuSourceIdRef.current = profile.id;
                  setDraggedMenuSourceId(profile.id);
                }}
                onPointerUp={(event) => {
                  if (draggedMenuSourceIdRef.current !== profile.id) {
                    return;
                  }
                  const target = document.elementFromPoint(event.clientX, event.clientY);
                  const targetRow = target?.closest<HTMLElement>("[data-source-id]");
                  const targetId = targetRow?.dataset.sourceId;
                  if (targetId && targetId !== profile.id) {
                    reorderProfile({ id: profile.id, targetId });
                  }
                  if (event.currentTarget.hasPointerCapture(event.pointerId)) {
                    event.currentTarget.releasePointerCapture(event.pointerId);
                  }
                  draggedMenuSourceIdRef.current = null;
                  setDraggedMenuSourceId(null);
                }}
                onPointerCancel={(event) => {
                  if (event.currentTarget.hasPointerCapture(event.pointerId)) {
                    event.currentTarget.releasePointerCapture(event.pointerId);
                  }
                  draggedMenuSourceIdRef.current = null;
                  setDraggedMenuSourceId(null);
                }}
              >
                <svg
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="2"
                  strokeLinecap="round"
                  aria-hidden="true"
                >
                  <circle cx="9" cy="5" r="1" />
                  <circle cx="9" cy="12" r="1" />
                  <circle cx="9" cy="19" r="1" />
                  <circle cx="15" cy="5" r="1" />
                  <circle cx="15" cy="12" r="1" />
                  <circle cx="15" cy="19" r="1" />
                </svg>
              </button>
            </div>
          ))}
          <div className="source-menu-divider" role="separator" />
          <button
            className="source-option manage"
            role="menuitem"
            type="button"
            onClick={() => {
              // Deliberately no selectProfile deep-link here, unlike the
              // horizontal trigger above: that button names exactly one
              // source, so it must land Settings on it. This item is plural
              // and source-agnostic — it preserves the settings window's own
              // edit selection rather than warping it (and the preflight
              // probe) to whichever owner the footer happens to advertise.
              // Settings shows its selection unambiguously (highlighted rail
              // card + the form's fields), so Apply/Delete can't silently
              // target a source other than the one displayed.
              setIsSourceMenuOpen(false);
              setMenuFrame(null);
              onOpenSettings();
            }}
          >
            <span className="source-check" aria-hidden="true">
              {"⚙"}
            </span>
            <span className="source-option-label">Add or edit sources…</span>
          </button>
        </div>,
          document.body,
        )
        : null}
    </div>
  );
}
