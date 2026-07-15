import { useEffect, useRef, useState } from "react";
import type { PointerEvent } from "react";
import { createPortal } from "react-dom";
import { MultiClientBadge } from "./MultiClientBadge";
import { SourceKindIcon } from "./SourceKindIcon";
import type { DesktopProfileConfig } from "../effect/profileModel";
import type { Orientation } from "../effect/prefs";

export type SourceMenuItem = {
  profile: DesktopProfileConfig;
  enabled: boolean;
  canToggle: boolean;
  isOwner: boolean;
};

type MenuDragRect = {
  left: number;
  top: number;
  width: number;
  height: number;
};

type MenuDropMarker = {
  targetId: string;
  left: number;
  top: number;
  width: number;
};

type MenuDragSession = {
  id: string;
  pointerId: number;
  item: SourceMenuItem;
  label: string;
  rowRect: MenuDragRect;
  offsetY: number;
  pointerY: number;
  marker: MenuDropMarker | null;
};

function GripIcon() {
  return (
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
  );
}

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
  attachedClientCount = 0,
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
  // Owner source's tmux client count; the badge surfaces only in the horizontal
  // bar (the vertical strip carries it per folder header instead).
  attachedClientCount?: number;
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
  // Ephemeral menu drag state: pointer coordinates, the grabbed row's original
  // rect, and the current insertion marker. Persistence only happens on drop.
  const [dragSession, setDragSession] = useState<MenuDragSession | null>(null);
  const [menuFrame, setMenuFrame] = useState<{
    bottom: number;
    left: number;
    width: number;
  } | null>(null);
  const dragSessionRef = useRef<MenuDragSession | null>(null);
  const sourceMenuRef = useRef<HTMLDivElement | null>(null);
  const sourceMenuPopupRef = useRef<HTMLDivElement | null>(null);
  const sourceRowRefs = useRef(new Map<string, HTMLDivElement>());

  function setMenuDragSession(next: MenuDragSession | null) {
    dragSessionRef.current = next;
    setDragSession(next);
  }

  function clearMenuDrag() {
    setMenuDragSession(null);
  }

  function closeSourceMenu() {
    setIsSourceMenuOpen(false);
    setMenuFrame(null);
    clearMenuDrag();
  }

  function rowRectFor(element: HTMLElement): MenuDragRect {
    const rect = element.getBoundingClientRect();
    return {
      left: rect.left,
      top: rect.top,
      width: rect.width,
      height: rect.height,
    };
  }

  function menuRowRects() {
    return sourceMenuItems
      .map(({ profile }) => {
        const element = sourceRowRefs.current.get(profile.id);
        return element ? { id: profile.id, rect: element.getBoundingClientRect() } : null;
      })
      .filter((row): row is { id: string; rect: DOMRect } => row !== null);
  }

  function hasUsableMenuRowRects() {
    const rows = menuRowRects();
    return rows.length > 0 && !rows.every(({ rect }) => rect.height === 0);
  }

  function markerForPointer(draggedId: string, pointerY: number): MenuDropMarker | null {
    const orderedIds = sourceMenuItems.map(({ profile }) => profile.id);
    const originalIndex = orderedIds.indexOf(draggedId);
    if (originalIndex === -1) {
      return null;
    }

    const rows = menuRowRects();
    if (rows.length === 0 || rows.every(({ rect }) => rect.height === 0)) {
      return null;
    }

    const insertionIndex = (() => {
      const index = rows.findIndex(({ rect }) => pointerY < rect.top + rect.height / 2);
      return index === -1 ? rows.length : index;
    })();
    const finalIndex = insertionIndex > originalIndex ? insertionIndex - 1 : insertionIndex;
    if (finalIndex === originalIndex || finalIndex < 0 || finalIndex >= orderedIds.length) {
      return null;
    }

    const menuRect = sourceMenuPopupRef.current?.getBoundingClientRect();
    const markerTop =
      insertionIndex >= rows.length
        ? rows[rows.length - 1].rect.bottom + 1
        : rows[insertionIndex].rect.top - 1;
    return {
      targetId: orderedIds[finalIndex],
      left: (menuRect?.left ?? rows[0].rect.left) + 8,
      top: markerTop,
      width: Math.max(0, (menuRect?.width ?? rows[0].rect.width) - 16),
    };
  }

  function fallbackTargetIdFromPoint(clientX: number, clientY: number, draggedId: string) {
    const target = document.elementFromPoint(clientX, clientY);
    const targetRow = target?.closest<HTMLElement>("[data-source-id]");
    const targetId = targetRow?.dataset.sourceId;
    return targetId && targetId !== draggedId ? targetId : null;
  }

  function startMenuDrag(event: PointerEvent<HTMLButtonElement>, item: SourceMenuItem) {
    if (event.button !== 0) {
      return;
    }
    const row = sourceRowRefs.current.get(item.profile.id);
    if (!row) {
      return;
    }
    const rect = rowRectFor(row);
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    setMenuDragSession({
      id: item.profile.id,
      pointerId: event.pointerId,
      item,
      label: labelFor(item.profile),
      rowRect: rect,
      offsetY: event.clientY - rect.top,
      pointerY: event.clientY,
      marker: null,
    });
  }

  function moveMenuDrag(event: PointerEvent<HTMLButtonElement>) {
    const session = dragSessionRef.current;
    if (!session || session.pointerId !== event.pointerId) {
      return;
    }
    event.preventDefault();
  setMenuDragSession({
    ...session,
    pointerY: event.clientY,
    marker: markerForPointer(session.id, event.clientY),
  });
  }

  function finishMenuDrag(event: PointerEvent<HTMLButtonElement>) {
    const session = dragSessionRef.current;
    if (!session || session.pointerId !== event.pointerId) {
      return;
    }
    event.preventDefault();
    const marker = markerForPointer(session.id, event.clientY);
    const targetId =
      marker?.targetId ??
      (hasUsableMenuRowRects()
        ? null
        : fallbackTargetIdFromPoint(event.clientX, event.clientY, session.id));
    if (targetId && targetId !== session.id) {
      reorderProfile({ id: session.id, targetId });
    }
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    clearMenuDrag();
  }

  function cancelMenuDrag(event: PointerEvent<HTMLButtonElement>) {
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    clearMenuDrag();
  }

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
      closeSourceMenu();
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
        closeSourceMenu();
      }
    }

    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.stopPropagation();
        event.preventDefault();
        if (dragSessionRef.current) {
          clearMenuDrag();
        } else {
          closeSourceMenu();
        }
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
            if (isSourceMenuOpen) {
              closeSourceMenu();
            } else {
              updateMenuFrame();
              setIsSourceMenuOpen(true);
            }
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
        {/* Horizontal-only, and only when the trigger names a real source (not the
            generic "Manage sources" state): the vertical strip shows this per
            folder header, and the bar displays just the owner source, so the
            owner's count belongs on its trigger. */}
        {orientation === "horizontal" && triggerShowsSource ? (
          <MultiClientBadge count={attachedClientCount} host={triggerTitle} />
        ) : null}
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
            <>
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
                {sourceMenuItems.map((item) => {
                  const { profile, enabled, canToggle, isOwner } = item;
                  const label = labelFor(profile);
                  return (
                    <div
                      className={`source-option source-row${isOwner ? " owner" : ""}${
                        enabled ? "" : " disabled"
                      }${dragSession?.id === profile.id ? " dragging" : ""}`}
                      data-source-id={profile.id}
                      key={profile.id}
                      ref={(node) => {
                        if (node) {
                          sourceRowRefs.current.set(profile.id, node);
                        } else {
                          sourceRowRefs.current.delete(profile.id);
                        }
                      }}
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
                          aria-label={`${enabled ? "Hide" : "Show"} ${label} in dock`}
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
                        <span className="source-option-name">{label}</span>
                      </span>
                      <button
                        className="source-grip"
                        type="button"
                        aria-label={`Drag ${label} to reorder`}
                        onPointerDown={(event) => startMenuDrag(event, item)}
                        onPointerMove={moveMenuDrag}
                        onPointerUp={finishMenuDrag}
                        onPointerCancel={cancelMenuDrag}
                      >
                        <GripIcon />
                      </button>
                    </div>
                  );
                })}
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
                    closeSourceMenu();
                    onOpenSettings();
                  }}
                >
                  <span className="source-check" aria-hidden="true">
                    {"⚙"}
                  </span>
                  <span className="source-option-label">Add or edit sources…</span>
                </button>
              </div>
              {dragSession?.marker ? (
                <div
                  className="source-drop-marker"
                  aria-hidden="true"
                  style={{
                    left: dragSession.marker.left,
                    top: dragSession.marker.top,
                    width: dragSession.marker.width,
                  }}
                />
              ) : null}
              {dragSession ? (
                <div
                  className={`source-option source-row source-drag-ghost${
                    dragSession.item.isOwner ? " owner" : ""
                  }${dragSession.item.enabled ? "" : " disabled"}`}
                  aria-hidden="true"
                  style={{
                    left: dragSession.rowRect.left,
                    top: dragSession.pointerY - dragSession.offsetY,
                    width: dragSession.rowRect.width,
                    height: dragSession.rowRect.height,
                  }}
                >
                  <span className="source-toggle" aria-hidden="true">
                    <input
                      type="checkbox"
                      checked={dragSession.item.enabled}
                      disabled
                      readOnly
                      tabIndex={-1}
                    />
                  </span>
                  <span className="source-option-mark" aria-hidden="true">
                    <SourceKindIcon kind={dragSession.item.profile.kind} />
                  </span>
                  <span className="source-option-text">
                    <span className="source-option-name">{dragSession.label}</span>
                  </span>
                  <span className="source-grip source-grip-ghost" aria-hidden="true">
                    <GripIcon />
                  </span>
                </div>
              ) : null}
            </>,
            document.body,
          )
        : null}
    </div>
  );
}
