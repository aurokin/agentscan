import { describe, expect, it } from "vitest";

import { providerLogo } from "./providerLogos";

describe("providerLogo", () => {
  it("resolves the same Amp logo for both themes", () => {
    const light = providerLogo("amp", "light");
    const dark = providerLogo("amp", "dark");

    expect(light).toEqual(expect.any(String));
    expect(dark).toEqual(expect.any(String));
    expect(light?.length).toBeGreaterThan(0);
    expect(dark?.length).toBeGreaterThan(0);
    expect(light).toBe(dark);
  });

  it("resolves Aider logos for both themes", () => {
    const light = providerLogo("aider", "light");
    const dark = providerLogo("aider", "dark");

    expect(light).toEqual(expect.any(String));
    expect(dark).toEqual(expect.any(String));
    expect(light?.length).toBeGreaterThan(0);
    expect(dark?.length).toBeGreaterThan(0);
    expect(light).not.toBe(dark);
  });

  it("resolves Prime Agent logos for both themes", () => {
    const light = providerLogo("prime", "light");
    const dark = providerLogo("prime", "dark");

    expect(light).toEqual(expect.any(String));
    expect(dark).toEqual(expect.any(String));
    expect(light?.length).toBeGreaterThan(0);
    expect(dark?.length).toBeGreaterThan(0);
    expect(light).not.toBe(dark);
  });

  it("resolves Kimi Code logos for both themes", () => {
    const light = providerLogo("kimi_code", "light");
    const dark = providerLogo("kimi_code", "dark");

    expect(light).toEqual(expect.any(String));
    expect(dark).toEqual(expect.any(String));
    expect(light?.length).toBeGreaterThan(0);
    expect(dark?.length).toBeGreaterThan(0);
    expect(light).not.toBe(dark);
  });
});
