import { describe, expect, it } from "vitest";

import { providerLogo } from "./providerLogos";

describe("providerLogo", () => {
  it("resolves Aider logos for both themes", () => {
    const light = providerLogo("aider", "light");
    const dark = providerLogo("aider", "dark");

    expect(light).toEqual(expect.any(String));
    expect(dark).toEqual(expect.any(String));
    expect(light?.length).toBeGreaterThan(0);
    expect(dark?.length).toBeGreaterThan(0);
    expect(light).not.toBe(dark);
  });
});
