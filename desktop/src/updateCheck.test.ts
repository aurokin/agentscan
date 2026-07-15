import { describe, expect, it, vi } from "vitest";

import {
  UPDATE_CHECK_CACHE_KEY,
  UPDATE_CHECK_INTERVAL_MS,
  fetchLatestReleaseVersion,
  isNewerVersion,
  parseReleaseTag,
  readUpdateCheckCache,
  resolveLatestVersion,
} from "./updateCheck";

const jsonResponse = (body: unknown, ok = true) =>
  ({
    ok,
    json: async () => body,
  }) as Response;

describe("parseReleaseTag", () => {
  it("extracts plain vX.Y.Z tags", () => {
    expect(parseReleaseTag("v0.8.2")).toBe("0.8.2");
    expect(parseReleaseTag(" v1.20.3 ")).toBe("1.20.3");
  });

  it("rejects anything that is not a plain release tag", () => {
    expect(parseReleaseTag("0.8.2")).toBeNull();
    expect(parseReleaseTag("v0.8")).toBeNull();
    expect(parseReleaseTag("v0.8.2-rc1")).toBeNull();
    expect(parseReleaseTag(null)).toBeNull();
    expect(parseReleaseTag(42)).toBeNull();
  });
});

describe("isNewerVersion", () => {
  it("compares numerically per component", () => {
    expect(isNewerVersion("0.8.3", "0.8.2")).toBe(true);
    expect(isNewerVersion("0.10.0", "0.9.9")).toBe(true);
    expect(isNewerVersion("1.0.0", "0.99.99")).toBe(true);
    expect(isNewerVersion("0.8.2", "0.8.2")).toBe(false);
    expect(isNewerVersion("0.8.1", "0.8.2")).toBe(false);
  });

  it("treats malformed versions as not newer", () => {
    expect(isNewerVersion("abc", "0.8.2")).toBe(false);
    expect(isNewerVersion("0.8.3", "dev")).toBe(false);
    expect(isNewerVersion("0.8", "0.8.2")).toBe(false);
  });
});

describe("readUpdateCheckCache", () => {
  it("round-trips a valid cache entry", () => {
    const stored = JSON.stringify({ checkedAtMs: 123, latestVersion: "0.8.3" });
    expect(readUpdateCheckCache(() => stored)).toEqual({
      checkedAtMs: 123,
      latestVersion: "0.8.3",
    });
  });

  it("reads corrupted or missing entries as absent", () => {
    expect(readUpdateCheckCache(() => null)).toBeNull();
    expect(readUpdateCheckCache(() => "not json")).toBeNull();
    expect(readUpdateCheckCache(() => JSON.stringify({ latestVersion: 3 }))).toBeNull();
  });
});

describe("fetchLatestReleaseVersion", () => {
  it("returns the parsed tag from a successful response", async () => {
    const fetchFn = vi.fn(async () => jsonResponse({ tag_name: "v0.9.0" }));
    await expect(fetchLatestReleaseVersion(fetchFn)).resolves.toBe("0.9.0");
    expect(fetchFn).toHaveBeenCalledWith(
      "https://api.github.com/repos/aurokin/agentscan/releases/latest",
      expect.objectContaining({ headers: expect.any(Object) }),
    );
  });

  it("is silent on http errors, bad bodies, and network failures", async () => {
    await expect(
      fetchLatestReleaseVersion(async () => jsonResponse({}, false)),
    ).resolves.toBeNull();
    await expect(
      fetchLatestReleaseVersion(async () => jsonResponse({ tag_name: "nightly" })),
    ).resolves.toBeNull();
    await expect(
      fetchLatestReleaseVersion(async () => {
        throw new Error("offline");
      }),
    ).resolves.toBeNull();
  });
});

describe("resolveLatestVersion", () => {
  const freshCache = (nowMs: number) =>
    JSON.stringify({ checkedAtMs: nowMs - 1000, latestVersion: "0.8.9" });

  it("serves a fresh cache without touching the network", async () => {
    const fetchFn = vi.fn();
    await expect(
      resolveLatestVersion({
        readStorage: () => freshCache(5_000_000),
        writeStorage: () => {},
        fetchFn: fetchFn as unknown as typeof fetch,
        nowMs: 5_000_000,
      }),
    ).resolves.toBe("0.8.9");
    expect(fetchFn).not.toHaveBeenCalled();
  });

  it("re-checks after the interval and rewrites the cache", async () => {
    const staleNow = 10_000 + UPDATE_CHECK_INTERVAL_MS + 1;
    const writes: Array<[string, string]> = [];
    await expect(
      resolveLatestVersion({
        readStorage: () =>
          JSON.stringify({ checkedAtMs: 10_000, latestVersion: "0.8.9" }),
        writeStorage: (key, value) => writes.push([key, value]),
        fetchFn: async () => jsonResponse({ tag_name: "v0.9.1" }),
        nowMs: staleNow,
      }),
    ).resolves.toBe("0.9.1");
    expect(writes).toEqual([
      [
        UPDATE_CHECK_CACHE_KEY,
        JSON.stringify({ checkedAtMs: staleNow, latestVersion: "0.9.1" }),
      ],
    ]);
  });

  it("falls back to a stale cache when the network is unavailable", async () => {
    await expect(
      resolveLatestVersion({
        readStorage: () =>
          JSON.stringify({ checkedAtMs: 0, latestVersion: "0.8.9" }),
        writeStorage: () => {},
        fetchFn: async () => {
          throw new Error("offline");
        },
        nowMs: UPDATE_CHECK_INTERVAL_MS * 2,
      }),
    ).resolves.toBe("0.8.9");
  });

  it("returns null with no cache and no network", async () => {
    await expect(
      resolveLatestVersion({
        readStorage: () => null,
        writeStorage: () => {},
        fetchFn: async () => {
          throw new Error("offline");
        },
        nowMs: 0,
      }),
    ).resolves.toBeNull();
  });
});
