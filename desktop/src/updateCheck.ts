// Startup update check against GitHub Releases (AUR-581 decision: a
// display-only version hint, not an auto-updater). Failures are silent — the
// check must never block or degrade the app when offline — and results are
// cached for a day so most launches render the hint without a network call.
// The webview CSP allowlists exactly this origin (`https://api.github.com`)
// in `connect-src` for this check; widen it further only with a matching
// documented decision.

export const UPDATE_CHECK_CACHE_KEY = "agentscan.updateCheck.v1";
export const UPDATE_CHECK_INTERVAL_MS = 24 * 60 * 60 * 1000;
const LATEST_RELEASE_URL =
  "https://api.github.com/repos/aurokin/agentscan/releases/latest";

export type UpdateCheckCache = {
  readonly checkedAtMs: number;
  readonly latestVersion: string;
};

// "v0.8.3" -> "0.8.3"; anything that is not a plain vX.Y.Z tag reads as no
// release so unusual tags can never produce a bogus hint.
export function parseReleaseTag(tag: unknown): string | null {
  if (typeof tag !== "string") {
    return null;
  }
  const match = /^v(\d+\.\d+\.\d+)$/.exec(tag.trim());
  return match ? match[1] : null;
}

export function isNewerVersion(candidate: string, current: string): boolean {
  const parts = (version: string) => version.split(".").map(Number);
  const candidateParts = parts(candidate);
  const currentParts = parts(current);
  if (
    candidateParts.length !== 3 ||
    currentParts.length !== 3 ||
    [...candidateParts, ...currentParts].some(Number.isNaN)
  ) {
    return false;
  }
  for (let index = 0; index < 3; index += 1) {
    if (candidateParts[index] !== currentParts[index]) {
      return candidateParts[index] > currentParts[index];
    }
  }
  return false;
}

export function readUpdateCheckCache(
  read: (key: string) => string | null,
): UpdateCheckCache | null {
  const raw = read(UPDATE_CHECK_CACHE_KEY);
  if (raw === null) {
    return null;
  }
  try {
    const parsed = JSON.parse(raw) as Partial<UpdateCheckCache>;
    if (
      typeof parsed.checkedAtMs === "number" &&
      typeof parsed.latestVersion === "string"
    ) {
      return {
        checkedAtMs: parsed.checkedAtMs,
        latestVersion: parsed.latestVersion,
      };
    }
  } catch {
    // A corrupted cache entry reads as absent and is overwritten on the next
    // successful check.
  }
  return null;
}

export async function fetchLatestReleaseVersion(
  fetchFn: typeof fetch,
): Promise<string | null> {
  try {
    const response = await fetchFn(LATEST_RELEASE_URL, {
      headers: { Accept: "application/vnd.github+json" },
    });
    if (!response.ok) {
      return null;
    }
    const body: unknown = await response.json();
    const tag =
      typeof body === "object" && body !== null
        ? (body as { tag_name?: unknown }).tag_name
        : null;
    return parseReleaseTag(tag);
  } catch {
    return null;
  }
}

// Latest known release version: the cache when fresh, otherwise a live check
// (falling back to a stale cache when the network is unavailable).
export async function resolveLatestVersion(options: {
  readonly readStorage: (key: string) => string | null;
  readonly writeStorage: (key: string, value: string) => void;
  readonly fetchFn: typeof fetch;
  readonly nowMs: number;
}): Promise<string | null> {
  const cached = readUpdateCheckCache(options.readStorage);
  // A future-dated entry (clock stepped backward since the last check) must
  // read as stale, or a negative elapsed time would keep it "fresh" until the
  // clock catches back up — suppressing checks far beyond the day interval.
  const cacheAgeMs = cached === null ? -1 : options.nowMs - cached.checkedAtMs;
  if (cached !== null && cacheAgeMs >= 0 && cacheAgeMs < UPDATE_CHECK_INTERVAL_MS) {
    return cached.latestVersion;
  }

  const latest = await fetchLatestReleaseVersion(options.fetchFn);
  if (latest !== null) {
    try {
      const cache: UpdateCheckCache = {
        checkedAtMs: options.nowMs,
        latestVersion: latest,
      };
      options.writeStorage(UPDATE_CHECK_CACHE_KEY, JSON.stringify(cache));
    } catch {
      // Storage being unavailable only costs a re-check next launch.
    }
  }
  return latest ?? cached?.latestVersion ?? null;
}
