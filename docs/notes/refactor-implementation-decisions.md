# Refactor Implementation Decisions

This note tracks implementation choices for the multi-PR refactor sequence.

## PR 1: Pane-output frame helpers

- Start with Codex, Claude, Gemini, and Pi because they share the simplest bottom-frame status pattern.
- Keep provider-specific marker predicates in each provider module; the shared helper only owns line indexing, tail anchoring, and gap checks.
- Add a small committed decisions log so rationale is reviewable at the end of the sequence.

## PR 2: Remaining pane-output frame helpers

- Extend `PaneOutputFrame` with bounded line/window/trailing helpers before moving the more complex providers, so Opencode, Grok, Hermes, Copilot, Cursor CLI, Droid, and Antigravity do not need direct slice arithmetic.
- Keep shape-specific stale-scrollback rules inside each provider; the helper remains a shared frame API rather than a generic status classifier.
