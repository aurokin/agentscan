import { useEffect, useRef, useState } from "react";
import { SourceKindIcon } from "./SourceKindIcon";
import { HOTKEY_MODIFIER_LABEL } from "../platform";
import type { DesktopProfileConfig } from "../effect/profileModel";
import type { Orientation } from "../effect/prefs";

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
  liveSources,
  triggerProfile,
  triggerShowsSource,
  triggerTone,
  triggerTitle,
  orientation,
  labelFor,
  selectProfile,
  reorderProfile,
  onOpenSettings,
}: {
  liveSources: ReadonlyArray<{ profile: DesktopProfileConfig; isOwner: boolean }>;
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
  onOpenSettings: () => void;
}) {
  // Which agentscan we're listening to (local vs a remote over SSH). Open
  // state for the inline dropdown.
  const [isSourceMenuOpen, setIsSourceMenuOpen] = useState(false);
  // Mid-drag source id for the order menu (the counterpart of the settings
  // rail's draggedSourceId).
  const [draggedMenuSourceId, setDraggedMenuSourceId] = useState<string | null>(null);
  const sourceMenuRef = useRef<HTMLDivElement | null>(null);

  // A wide drag or pinning to horizontal can strand an already-open source menu in
  // the thin bar (where it clips). Close it whenever the layout goes horizontal.
  useEffect(() => {
    if (orientation === "horizontal") {
      setIsSourceMenuOpen(false);
    }
  }, [orientation]);

  // Dismiss the source dropdown on an outside click or Escape. The keydown is
  // captured so it closes the menu before the picker's global Escape handler
  // hides the whole window (capture beats bubble regardless of which component
  // registered first).
  useEffect(() => {
    if (!isSourceMenuOpen) {
      return;
    }

    function onPointerDown(event: MouseEvent) {
      if (sourceMenuRef.current && !sourceMenuRef.current.contains(event.target as Node)) {
        setIsSourceMenuOpen(false);
      }
    }

    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.stopPropagation();
        event.preventDefault();
        setIsSourceMenuOpen(false);
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
            setIsSourceMenuOpen((open) => !open);
          }
        }}
        title={
          triggerShowsSource
            ? triggerTitle
            : "Drag to reorder sources — the top open source owns row hotkeys"
        }
      >
        {triggerShowsSource ? (
          <span className="status-dot" data-tone={triggerTone} aria-hidden="true" />
        ) : null}
        <span className="source-label">
          {triggerShowsSource ? labelFor(triggerProfile) : "Manage sources"}
        </span>
        <span className={`source-caret${isSourceMenuOpen ? " open" : ""}`} aria-hidden="true">
          {"›"}
        </span>
      </button>
      {isSourceMenuOpen ? (
        // Pure ordering surface: drag rows to reorder sources. The topmost
        // OPEN folder owns the row hotkeys, so this is where the dock decides
        // which source answers them. Nothing else is duplicated here —
        // open/close lives on the folder headers, and enable/disable/add/
        // remove live in Settings.
        //
        // Deliberately NOT a quick-switch: the dock never changes the active
        // source. The old single-select footer existed because the dock could
        // show one source at a time; folders replace that gesture with the open
        // set. "Active" now only means the settings-edit selection + the single
        // preflight target, and it changes in Settings.
        <div className="source-menu" role="menu">
          {/* Draggable rows are safe inside the footer's frameless drag
              region: Tauri's data-tauri-drag-region handler only fires
              when the mousedown TARGET itself carries the attribute, so
              descendants start HTML5 drags, never window drags. */}
          {liveSources.map(({ profile, isOwner }) => (
            <div
              className={`source-option draggable${
                draggedMenuSourceId === profile.id ? " dragging" : ""
              }`}
              key={profile.id}
              role="menuitem"
              draggable
              onDragStart={(event) => {
                event.dataTransfer.effectAllowed = "move";
                setDraggedMenuSourceId(profile.id);
              }}
              onDragEnd={() => setDraggedMenuSourceId(null)}
              onDragOver={(event) => {
                // preventDefault marks this row as a valid drop target.
                if (draggedMenuSourceId && draggedMenuSourceId !== profile.id) {
                  event.preventDefault();
                  event.dataTransfer.dropEffect = "move";
                }
              }}
              onDrop={(event) => {
                event.preventDefault();
                if (draggedMenuSourceId && draggedMenuSourceId !== profile.id) {
                  reorderProfile({
                    id: draggedMenuSourceId,
                    targetId: profile.id,
                  });
                }
                setDraggedMenuSourceId(null);
              }}
            >
              <span className="source-option-mark" aria-hidden="true">
                <SourceKindIcon kind={profile.kind} />
              </span>
              <span className="source-option-text">
                <span className="source-option-name">{labelFor(profile)}</span>
              </span>
              {isOwner ? (
                <kbd className="folder-kbd" title="Row hotkeys target this source">
                  {HOTKEY_MODIFIER_LABEL.trim()}
                </kbd>
              ) : null}
              <svg
                className="source-grip"
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
              onOpenSettings();
            }}
          >
            <span className="source-check" aria-hidden="true">
              {"⚙"}
            </span>
            <span className="source-option-label">Add or edit sources…</span>
          </button>
        </div>
      ) : null}
    </div>
  );
}
