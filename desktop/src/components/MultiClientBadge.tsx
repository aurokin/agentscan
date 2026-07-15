// Per-source "multiple viewers" mark: shown on a source whose tmux server has
// more than one client attached. This isn't an error — two terminals on one
// server is normal — only a heads-up that that source's live-pane highlight is
// best-effort (it follows whichever client moved most recently), so it's a muted
// people glyph rather than a warning, with the count and the caveat in the
// tooltip. Renders nothing at <= 1 client. The same mark sits on each folder
// header (vertical) and the footer source trigger (horizontal), so the signal
// names the specific host instead of a global banner that can't say which one.
//
// a11y: the mark lives inside a name-from-content <button>, so its accessible
// name is appended to the button's. We keep it terse and lead with a separator
// (", N viewers") so the composed name reads "host, N viewers" rather than gluing
// the count onto the host ("host2 viewers") — the accessible-name algorithm
// concatenates descendant text without inserting whitespace and trims a leading
// space, so a comma is what survives. The full explanation rides the hover
// `title`. We deliberately do not announce via a live region — it's a visual
// heads-up about a visual affordance (the pane highlight), reachable on focus
// through the button name.
export function MultiClientBadge({ count, host }: { count: number; host?: string }) {
  if (count <= 1) {
    return null;
  }
  return (
    <span
      className="client-badge"
      role="img"
      aria-label={`, ${count} viewers`}
      title={`${count} clients attached to ${host ?? "this server"} — the live-pane highlight follows the most recent one`}
    >
      <svg
        className="client-badge-icon"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
      >
        {/* Lucide "users" (ISC), inlined to match SourceKindIcon. */}
        <path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2" />
        <circle cx="9" cy="7" r="4" />
        <path d="M22 21v-2a4 4 0 0 0-3-3.87" />
        <path d="M16 3.13a4 4 0 0 1 0 7.75" />
      </svg>
    </span>
  );
}
