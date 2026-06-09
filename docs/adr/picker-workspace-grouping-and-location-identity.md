# ADR: Picker Workspace Grouping And Location Identity

Status: accepted
Date: 2026-06-08

## Context

The picker currently tracks and presents panes primarily by tmux location. For
workflows where tmux session names intentionally match projects, this works well:
the session name is both a tmux address component and a useful human grouping
label.

That assumption does not hold for every workflow. Some users organize tmux
sessions by host, activity, date, client, or arbitrary names. For those users, a
git repository name or working-directory basename may be a better way to answer
"which work is this agent doing?" than the tmux session name.

At the same time, users still need the exact tmux address. `session:window.pane`
is the stable human answer to "where is this pane?" and remains important for
debugging, recovery, terminal use, and cross-checking desktop picker actions.

Picker hotkeys are also part of the shared contract. They are assigned by
`agentscan`, consumed by the terminal TUI and desktop app, and used by tmux binds
and automation. The UI must not invent a different order or assignment model than
the backend.

## Decision

Separate workspace context from tmux location identity.

The picker model should preserve these concepts as distinct meanings:

- `pane_id`: the machine focus target.
- tmux location: the structured tmux address.
- location tag: the human tmux address, always represented as
  `session:window.pane`.
- display label: the agent task, conversation, or provider-derived label.
- workspace context: the human grouping label for "what work is this?"

Session grouping remains the default behavior. It preserves the current mental
model, row order, and hotkey assignment for existing users.

A single user-facing grouping choice should control both grouping and picker
order:

- `session`: group by tmux session and order by tmux location.
- `git-repo`: group by repository context and order by group, then tmux location.
- `cwd`: group by working-directory context and order by group, then tmux
  location.

There should not be a separate picker ordering setting unless real usage shows a
need for it. Choosing a non-session grouping mode means choosing project-first
picker ordering, and hotkey assignment follows that picker order.

The terminal TUI and desktop app may render the same row differently, but they
must preserve the same backend picker model:

- `agentscan` owns picker row order and hotkey assignment.
- clients render the returned keys rather than assigning their own.
- clients may visually group rows, but must not reorder rows in a way that makes
  displayed keys disagree with backend activation.

The tmux location tag must remain available and visible enough that users can
always recover the exact `session:window.pane` target, regardless of grouping
mode.

When richer workspace context is unavailable or ambiguous, the picker should
degrade to an honest fallback label rather than inventing a stronger project
identity.

## Consequences

- Existing session-name workflows keep their current behavior by default.
- Users whose tmux sessions do not map to projects can switch the picker to a
  repo- or folder-oriented model without losing tmux address visibility.
- Hotkey assignment remains a backend contract shared by CLI, TUI, tmux binds,
  automation, and desktop surfaces.
- Desktop grouping should become an explicit rendering of picker workspace
  context, not a client-side parse of `location_tag`.
- Non-session grouping intentionally changes hotkey order because grouping and
  ordering are one decision.

## Non-Goals

- Do not replace or overload tmux session identity with project identity.
- Do not hide `session:window.pane` in project-oriented displays.
- Do not add separate `group_by` and `order_by` user settings before there is a
  demonstrated need.
- Do not assign duplicate hotkeys within visual groups.
- Do not let desktop or TUI clients create a private sort order that disagrees
  with backend picker activation.
- Do not require git repository detection or wrapper metadata for baseline pane
  discovery.
- Do not infer rich project labels from weak evidence.
