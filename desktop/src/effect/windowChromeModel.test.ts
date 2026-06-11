import { describe, expect, it } from "vitest";
import { sizePlanFor } from "./windowChromeModel";
import { BAR_WINDOW_HEIGHT } from "../windowOperations";

describe("sizePlanFor", () => {
  it("locks the pinned horizontal bar: min and max height both BAR_WINDOW_HEIGHT", () => {
    const plan = sizePlanFor("horizontal", "vertical", true);
    expect(plan.minSize).toEqual({ width: 220, height: BAR_WINDOW_HEIGHT });
    expect(plan.maxSize).toEqual({ width: 10000, height: BAR_WINDOW_HEIGHT });
    expect(plan.minSize.height).toBe(plan.maxSize?.height);
  });

  it("caps pinned vertical into a strip with free height", () => {
    const plan = sizePlanFor("vertical", "horizontal", true);
    expect(plan.minSize).toEqual({ width: 220, height: 520 });
    expect(plan.maxSize).toEqual({ width: 520, height: 10000 });
  });

  it("leaves auto uncapped with the floor matched to the live shape", () => {
    expect(sizePlanFor("auto", "vertical", true)).toEqual({
      minSize: { width: 220, height: 520 },
      maxSize: null,
      shouldPlace: false,
    });
    expect(sizePlanFor("auto", "horizontal", true).minSize).toEqual({
      width: 220,
      height: 44,
    });
  });

  it("places on every pinned reshape but only the FIRST auto mount", () => {
    expect(sizePlanFor("horizontal", "horizontal", true).shouldPlace).toBe(true);
    expect(sizePlanFor("vertical", "vertical", true).shouldPlace).toBe(true);
    expect(sizePlanFor("auto", "vertical", false).shouldPlace).toBe(true);
    expect(sizePlanFor("auto", "vertical", true).shouldPlace).toBe(false);
  });
});
