# Provider Evidence Roadmap

This note captures the current direction for provider-specific detection work.
It is intentionally planning-level: shipped contracts belong in
`docs/integration.md`, while active sequencing and ownership live in Linear.

## Product Principle

`agentscan` must remain plug-and-play by default. Users should not need to
install agent hooks, provider extensions, launch wrappers, or shell integration
just to see common agent panes in tmux.

The detection ladder stays:

1. explicit tmux metadata when present
2. provider-specific tmux title and pane metadata signals
3. targeted process-tree fallback for ambiguous launcher panes
4. optional hooks, extensions, or wrappers as the final enrichment layer only

Hooks and extensions are deep-roadmap work. They should come after we have
exhausted upstream source analysis, local probing, and conservative
plug-and-play support for each provider.

## Pi Coding Agent Plan

Upstream analyzed: `~/code/upstream/pi-mono`, package
`@mariozechner/pi-coding-agent`.

Current upstream signals:

- CLI binary is `pi`, with npm entrypoint `dist/cli.js`.
- Compiled Bun binary is emitted as `dist/pi`.
- Startup sets `process.title = APP_NAME`, defaulting to `pi`.
- Startup sets `PI_CODING_AGENT=true`.
- Default terminal title is `π - <cwd>` or `π - <session name> - <cwd>`.
- Default terminal title does not encode ready/busy state.
- Optional terminal progress uses OSC `9;4`, but `showTerminalProgress` defaults
  false and is not yet part of agentscan's tmux evidence model.

Implementation behavior:

- Classify default Greek Pi titles such as `π - agentscan` as Pi.
- Preserve the current conservative behavior for bare ASCII `pi - <cwd>` titles
  unless another Pi signal is present.
- Add targeted process fallback for unresolved launcher panes with:
  - Linux `PI_CODING_AGENT=true` process evidence
  - `@mariozechner/pi-coding-agent/dist/cli.js`
  - known package-manager shims such as `node_modules/.bin/pi`,
    `/opt/homebrew/bin/pi`, and `/usr/local/bin/pi`
  - compiled binary paths only when the surrounding path indicates the Pi
    package or build output
- Keep Pi status `unknown` unless there is direct state evidence. Default Pi
  titles should not be interpreted as idle or busy.
- Treat default Greek Pi title text as context for display labels, not as an
  activity label.
- Use `inspect` diagnostics to expose which Pi signal won.

## Deep-Roadmap Provider-Side Enrichment

Optional provider-side support is the last layer, not near-term implementation
work. It should improve labels, session ids, and activity state only after the
plug-and-play baseline is broadly settled.

Deep-roadmap targets:

- Codex hook support: use provider lifecycle events or local hook surfaces to
  publish explicit tmux metadata for provider, label, cwd, state, and session id.
- Claude Code hook support: use Claude's hook/lifecycle surfaces to publish the
  same metadata where available.
- Pi extension support: use Pi's extension API to publish tmux metadata from
  session and agent events.

These integrations should be additive and deeply deferred. A missing hook or
extension must never turn a normally detectable pane into `unknown`.

## Provider Research Queue

Open-source providers should be analyzed from source before adding heuristics.
Each analysis should record:

- package and binary names
- process tree and argv shapes for npm, pnpm, Homebrew, Bun, and source runs
- environment markers
- terminal title formats
- explicit state or lifecycle signals
- false-positive risks
- tests needed in agentscan

Completed source-analysis baselines:

- opencode: source analysis found `OpenCode` / `OC | ...` title shapes,
  package and platform binary paths, and Linux `OPENCODE` process markers.
  Default opencode titles do not carry run state; richer status should remain a
  later optional plugin/metadata path.

Closed-source providers require empirical probing and conservative inference.
For each provider, capture snapshots while idle, busy, waiting for input, and
after restart/resume:

- tmux title
- `pane_current_command`
- process argv and selected environment
- terminal output/status lines only if later justified
- any stable local files, sockets, or logs that indicate state without secrets

Closed-source queue:

- GitHub Copilot CLI: available now because a Copilot subscription is available.
- Cursor CLI: blocked on access to a subscription or test environment.

Closed-source probing should produce a written evidence matrix before code
changes. If a signal is weak, agentscan should prefer `unknown` over a richer
but invented classification.
