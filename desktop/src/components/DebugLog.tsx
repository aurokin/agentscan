import type { DebugEntry } from "../effect/DebugLog";

export function DebugLog({ entries }: { entries: ReadonlyArray<DebugEntry> }) {
  if (entries.length === 0) {
    return <p className="muted">No debug events yet.</p>;
  }

  return (
    <ol className="debug-list">
      {entries.map((entry) => (
        <li key={entry.id}>
          <time>{entry.time}</time>
          <span>{entry.kind}</span>
          <strong>{entry.label}</strong>
          <small>{entry.detail}</small>
        </li>
      ))}
    </ol>
  );
}
