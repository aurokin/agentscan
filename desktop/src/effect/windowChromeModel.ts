// The dock window's shape plan: which size constraints and placement the
// sizing effect should apply for an orientation preference. Pure and
// node-testable; the apply side (the queued native calls) lives in
// windowOperations.applyWindowShape, and DockApp wires the two together.

import {
  BAR_WINDOW_HEIGHT,
  WINDOW_MAX_UNBOUNDED,
  WINDOW_MAX_WIDTH_VERTICAL,
  WINDOW_MIN_HEIGHT_HORIZONTAL,
  WINDOW_MIN_HEIGHT_VERTICAL,
  WINDOW_MIN_WIDTH,
} from "../windowOperations";
import type { Orientation, OrientationPreference } from "./prefs";

// Plain width/height — the apply constructs Tauri's LogicalSize from these, so
// the plan stays free of host types.
export type WindowSize = { width: number; height: number };

export type WindowSizePlan = {
  minSize: WindowSize;
  // null = uncapped ("auto" clears the cap entirely).
  maxSize: WindowSize | null;
  shouldPlace: boolean;
};

// One race-free shape per preference ("auto" = free: no cap, no snap):
//
// - Pinned horizontal locks the bar to BAR_WINDOW_HEIGHT (min == max height)
//   so it can only be resized horizontally — the layout is tuned for that
//   exact height. Pinned vertical caps width into a strip. "auto" stays free
//   on both axes (just a min floor matched to the live shape).
// - shouldPlace: place on the first dock mount (so a saved layout opens
//   correctly) and on every pinned reshape, but not on a later "auto" drag —
//   which must not be fought.
export function sizePlanFor(
  orientationPref: OrientationPreference,
  effectiveOrientation: Orientation,
  didInitialPlace: boolean,
): WindowSizePlan {
  const sizingOrientation: Orientation =
    orientationPref === "auto" ? effectiveOrientation : orientationPref;
  return {
    minSize:
      orientationPref === "horizontal"
        ? { width: WINDOW_MIN_WIDTH, height: BAR_WINDOW_HEIGHT }
        : {
            width: WINDOW_MIN_WIDTH,
            height:
              sizingOrientation === "horizontal"
                ? WINDOW_MIN_HEIGHT_HORIZONTAL
                : WINDOW_MIN_HEIGHT_VERTICAL,
          },
    maxSize:
      orientationPref === "vertical"
        ? { width: WINDOW_MAX_WIDTH_VERTICAL, height: WINDOW_MAX_UNBOUNDED }
        : orientationPref === "horizontal"
          ? { width: WINDOW_MAX_UNBOUNDED, height: BAR_WINDOW_HEIGHT }
          : null,
    shouldPlace: orientationPref !== "auto" || !didInitialPlace,
  };
}
