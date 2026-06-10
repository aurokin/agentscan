// Source-kind mark: Lucide "house" / "server" outlines (ISC), inlined so the
// mark renders crisply at small sizes instead of leaning on font glyph
// coverage. Each context sizes it via font-size (the icon is 1em square).
export function SourceKindIcon({ kind }: { kind: "local" | "ssh" }) {
  return (
    <svg
      className="source-kind-icon"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {kind === "ssh" ? (
        <>
          <rect width="20" height="8" x="2" y="2" rx="2" ry="2" />
          <rect width="20" height="8" x="2" y="14" rx="2" ry="2" />
          <path d="M6 6h.01" />
          <path d="M6 18h.01" />
        </>
      ) : (
        <>
          <path d="M15 21v-8a1 1 0 0 0-1-1h-4a1 1 0 0 0-1 1v8" />
          <path d="M3 10a2 2 0 0 1 .709-1.528l7-5.999a2 2 0 0 1 2.582 0l7 5.999A2 2 0 0 1 21 10v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
        </>
      )}
    </svg>
  );
}
