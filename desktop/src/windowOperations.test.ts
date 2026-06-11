// Transcript tests for the three queued-op bodies: every fake sink records
// into one ordered list, so the assertions pin the native/DOM interleavings
// the comments promise (blur-before-translucent, opaque-before-drop,
// unset-max-before-set-min, flip-attr-only-after-decorations-land) — the
// sequences a pure decision function could not.
import { describe, expect, it } from "vitest";
import { applyFrameless, applyGlass, applyWindowShape } from "./windowOperations";
import { sizePlanFor } from "./effect/windowChromeModel";

const recorder = () => {
  const transcript: string[] = [];
  return {
    transcript,
    log:
      (name: string) =>
      (...args: unknown[]) => {
        transcript.push(`${name}(${args.map((a) => JSON.stringify(a)).join(",")})`);
      },
  };
};

describe("applyWindowShape", () => {
  const fakeWindow = (transcript: string[], failMin = false) => ({
    setMinSize: (size: { width: number; height: number }) => {
      transcript.push(`min(${size.width}x${size.height})`);
      return failMin ? Promise.reject(new Error("boom")) : Promise.resolve();
    },
    setMaxSize: (size: { width: number; height: number } | null) => {
      transcript.push(size ? `max(${size.width}x${size.height})` : "max(null)");
      return Promise.resolve();
    },
  });

  it("unsets the cap before raising min, re-caps, then places — in one op", async () => {
    const { transcript } = recorder();
    await applyWindowShape({
      plan: sizePlanFor("horizontal", "horizontal", true),
      place: async () => {
        transcript.push("place");
      },
      getWindow: () => fakeWindow(transcript),
    });
    expect(transcript).toEqual(["max(null)", "min(220x56)", "max(10000x56)", "place"]);
  });

  it("keeps the trailing unconditional max(null) in auto and skips placement after the first", async () => {
    const { transcript } = recorder();
    await applyWindowShape({
      plan: sizePlanFor("auto", "vertical", true),
      place: async () => {
        transcript.push("place");
      },
      getWindow: () => fakeWindow(transcript),
    });
    expect(transcript).toEqual(["max(null)", "min(220x520)", "max(null)"]);
  });

  it("resolves on a native failure and never places past it", async () => {
    const { transcript } = recorder();
    await expect(
      applyWindowShape({
        plan: sizePlanFor("horizontal", "horizontal", true),
        place: async () => {
          transcript.push("place");
        },
        getWindow: () => fakeWindow(transcript, true),
      }),
    ).resolves.toBeUndefined();
    expect(transcript).not.toContain("place");
  });

  it("dereferences the place thunk at run time (the summonPlacementRef pattern)", async () => {
    const { transcript } = recorder();
    const ref = {
      current: async () => {
        transcript.push("stale");
      },
    };
    const run = applyWindowShape({
      plan: sizePlanFor("vertical", "vertical", true),
      place: () => ref.current(),
      getWindow: () => fakeWindow(transcript),
    });
    // An orientation flip lands while the op is queued/running.
    ref.current = async () => {
      transcript.push("live");
    };
    await run;
    expect(transcript).toContain("live");
    expect(transcript).not.toContain("stale");
  });
});

describe("applyGlass", () => {
  const base = (transcript: string[], log: ReturnType<typeof recorder>["log"]) => ({
    enabled: true,
    radius: null as number | null,
    isCancelled: () => false,
    currentClear: () => 0.5,
    setAttr: log("attr") as (value: "on" | "off") => void,
    setClear: log("clear") as (clear: number) => void,
    onError: log("error") as (error: unknown) => void,
    invokeIpc: (cmd: string, args: Record<string, unknown>) => {
      transcript.push(`invoke(${cmd},${JSON.stringify(args)})`);
      return Promise.resolve();
    },
  });

  it("enables blur-first: invoke lands before the surface goes translucent", async () => {
    const { transcript, log } = recorder();
    await applyGlass({ ...base(transcript, log), radius: 12 });
    expect(transcript).toEqual([
      'invoke(set_window_glass,{"enabled":true,"radius":12})',
      'attr("on")',
      "clear(0.5)",
    ]);
  });

  it("disables opaque-first: DOM goes off before the blur drops", async () => {
    const { transcript, log } = recorder();
    await applyGlass({ ...base(transcript, log), enabled: false });
    expect(transcript).toEqual([
      'attr("off")',
      "clear(0)",
      'invoke(set_window_glass,{"enabled":false,"radius":null})',
    ]);
  });

  it("reads currentClear at run time, after the native call resolves", async () => {
    const { transcript, log } = recorder();
    let alpha = 0.2;
    let resolveInvoke!: () => void;
    const run = applyGlass({
      ...base(transcript, log),
      currentClear: () => alpha,
      invokeIpc: () =>
        new Promise<void>((resolve) => {
          resolveInvoke = resolve;
        }),
    });
    // The slider moves while the op awaits the native call.
    alpha = 0.8;
    resolveInvoke();
    await run;
    expect(transcript).toContain("clear(0.8)");
  });

  it("skips everything when cancelled before running", async () => {
    const { transcript, log } = recorder();
    await applyGlass({ ...base(transcript, log), isCancelled: () => true });
    expect(transcript).toEqual([]);
  });

  it("skips the DOM (but not the invoke) when superseded mid-invoke", async () => {
    const { transcript, log } = recorder();
    let cancelled = false;
    await applyGlass({
      ...base(transcript, log),
      isCancelled: () => cancelled,
      invokeIpc: (cmd, args) => {
        transcript.push(`invoke(${cmd},${JSON.stringify(args)})`);
        cancelled = true;
        return Promise.resolve();
      },
    });
    expect(transcript).toEqual(['invoke(set_window_glass,{"enabled":true,"radius":null})']);
  });

  it("falls back to opaque and reports on failure — resolving, even when cancelled", async () => {
    const { transcript, log } = recorder();
    let cancelled = false;
    const failure = new Error("native boom");
    await expect(
      applyGlass({
        ...base(transcript, log),
        isCancelled: () => cancelled,
        invokeIpc: () => {
          cancelled = true;
          return Promise.reject(failure);
        },
      }),
    ).resolves.toBeUndefined();
    // The catch runs despite the cancellation: the queue's next op converges
    // the state, but a failed toggle must never leave a translucent surface.
    expect(transcript).toEqual(['attr("off")', "clear(0)", "error({})"]);
  });
});

describe("applyFrameless", () => {
  const base = (transcript: string[], log: ReturnType<typeof recorder>["log"]) => ({
    enabled: true,
    isCancelled: () => false,
    setAttr: log("attr") as (value: "on" | "off") => void,
    setApplied: log("applied") as (applied: boolean) => void,
    onError: log("error") as (error: unknown) => void,
    invokeIpc: (cmd: string, args: Record<string, unknown>) => {
      transcript.push(`invoke(${cmd},${JSON.stringify(args)})`);
      return Promise.resolve();
    },
  });

  it("flips the attribute and applied state only after decorations land", async () => {
    const { transcript, log } = recorder();
    await applyFrameless(base(transcript, log));
    expect(transcript).toEqual([
      'invoke(set_window_decorations,{"decorations":false})',
      'attr("on")',
      "applied(true)",
    ]);
  });

  it("restores decorations on disable", async () => {
    const { transcript, log } = recorder();
    await applyFrameless({ ...base(transcript, log), enabled: false });
    expect(transcript).toEqual([
      'invoke(set_window_decorations,{"decorations":true})',
      'attr("off")',
      "applied(false)",
    ]);
  });

  it("skips everything when cancelled before running", async () => {
    const { transcript, log } = recorder();
    await applyFrameless({ ...base(transcript, log), isCancelled: () => true });
    expect(transcript).toEqual([]);
  });

  it("skips the DOM/applied flip when superseded mid-invoke", async () => {
    const { transcript, log } = recorder();
    let cancelled = false;
    await applyFrameless({
      ...base(transcript, log),
      invokeIpc: (cmd, args) => {
        transcript.push(`invoke(${cmd},${JSON.stringify(args)})`);
        cancelled = true;
        return Promise.resolve();
      },
      isCancelled: () => cancelled,
    });
    expect(transcript).toEqual(['invoke(set_window_decorations,{"decorations":false})']);
  });

  it("hides the custom chrome and reports on failure — resolving, even when cancelled", async () => {
    const { transcript, log } = recorder();
    let cancelled = false;
    await expect(
      applyFrameless({
        ...base(transcript, log),
        isCancelled: () => cancelled,
        invokeIpc: () => {
          cancelled = true;
          return Promise.reject(new Error("native boom"));
        },
      }),
    ).resolves.toBeUndefined();
    expect(transcript).toEqual(['attr("off")', "applied(false)", "error({})"]);
  });
});
