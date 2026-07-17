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
3. targeted live process fallback for ambiguous launcher panes and shell
   wrappers
4. shallow provider-scoped pane output parsing for status only, after provider
   identity is already known and current prompt/footer shapes are stable enough
5. optional hooks, extensions, or wrappers as the final enrichment layer only

Hooks and extensions are deep-roadmap work. They should come after we have
exhausted upstream source analysis, local probing, and conservative
plug-and-play support for each provider.

Provider logs, transcript files, session databases, telemetry files, and other
historical state stores are not baseline detection inputs. They can be used to
understand a closed-source provider during research, but shipped
plug-and-play detection should rely on live tmux metadata, terminal titles,
foreground/root/descendant process evidence, and tightly scoped pane output.
When pane output supplies state, JSON should expose that provenance as
`status.source="pane_output"`.

## Coordination Status

The near-term provider evidence pass is complete:

- Pi plug-and-play support shipped from upstream source evidence. Pi remains
  conservative around ASCII titles and default title-derived status.
- opencode support shipped from upstream source analysis, exact title shapes,
  process/package evidence, and provider-scoped pane-output status fallback.
- GitHub Copilot and Cursor CLI support shipped from empirical local probing,
  with title branding treated as supporting context rather than standalone
  provider identity.
- Factory Droid CLI support shipped from empirical local probing, exact command
  evidence, supporting title labels, and provider-scoped pane-output status
  fallback.
- Aider support shipped from upstream source evidence, exact command and package
  path evidence, targeted `python -m aider` process evidence, and Python
  console-script invocation evidence. Status remains conservative because
  upstream exposes only a generic prompt surface.

The remaining provider-side integration issues are intentionally deep-roadmap:

- Codex and Claude Code hooks may later publish explicit tmux metadata, but
  baseline detection must not depend on them.
- Pi extension metadata may later enrich state, labels, and session identity,
  but default Pi detection must keep working without it.

Gemini CLI is deprecated as an active maintenance target. Existing support is
kept for current users, including enterprise users who still run Gemini CLI, but
its progress has stalled and agentscan is not planning ongoing drift updates for
the foreseeable future. Do not remove Gemini support solely because it is
deprecated; also do not prioritize new Gemini UI/status changes unless product
priorities change.

New provider work should continue to start with source analysis for
open-source agents or empirical probing for closed-source agents, then encode
only the strongest low-risk signals in the baseline scanner.

## Pi Coding Agent Plan

Upstream analyzed: `~/code/upstream/pi-mono` commit `385a11bf`, package
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
- Interactive mode renders chat, status, widgets, editor, then footer. The
  empty editor from `packages/tui/src/components/editor.ts` renders horizontal
  border lines, and the footer from
  `packages/coding-agent/src/modes/interactive/components/footer.ts` includes a
  context token such as `0.0%/200k` or `?/200k`.
- Current busy UI is explicit in source: `interactive-mode.ts` renders
  `Working...` with an interrupt hint, retry and compaction loaders with cancel
  hints, and bash execution renders `Running...` with a cancel hint.

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
- Use provider-scoped pane output status fallback only after Pi identity is
  already established. Mark idle from the current empty editor frame when it is
  near the current footer context token. Mark busy from current Pi loader text
  such as `Working...`, retry, compaction, or bash `Running...` cancel lines.
  Preserve `unknown` for stale editor frames, generic text, errors, warnings,
  or weak historical output.
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

- Claude Code: source analysis at `~/code/upstream/claude-code` commit
  `611aee9d3` found the current prompt/footer surface in the React/Ink TUI.
  The checkout has no configured Git remote, so this is local source evidence.
  `src/components/PromptInput/PromptInputModeIndicator.tsx` renders the main
  prompt marker as `❯`, while
  `src/components/PromptInput/PromptInput.tsx` renders the prompt input inside
  a bottom-bordered container and
  `src/components/PromptInput/PromptInputFooterLeftSide.tsx` renders footer
  hints such as `? for shortcuts`, permission-mode hints, and loading hints
  from `getSpinnerHintParts`. Current loading surfaces include an `esc to
  interrupt` hint; suppressed permission dialogs can also leave a current
  `Waiting for permission…` row above the prompt. The placeholder itself is
  not stable because `usePromptInputPlaceholder.ts` can render onboarding
  examples, queue hints, teammate messages, or no placeholder. Pane-output
  status fallback should therefore remain provider-scoped, require the current
  `❯` prompt to be near Claude footer/status text, mark busy from current
  interrupt or permission-wait markers, and preserve `unknown` for stale
  prompts, generic Claude mentions, or prompt lines without current footer
  context.
- Codex: source analysis at `~/code/upstream/codex` commit
  `a27d3847b5` found the current idle composer and busy status shapes in the
  Rust TUI. `codex-rs/tui/src/keymap_setup.rs` configures the default composer
  placeholder as `Ask Codex to do anything`; snapshots in
  `codex-rs/tui/src/chatwidget/snapshots/` show that rendered as
  `› Ask Codex to do anything` near footer/status text such as `100% context
  left`, `Context 0% used`, or `Fast on`. `codex-rs/tui/src/status_indicator_widget.rs`
  renders current busy rows with headers such as `Working` followed by elapsed
  time and `esc to interrupt`; approval snapshots expose current action prompts
  such as `Yes, proceed` and `Press enter to confirm or esc to cancel`.
  Pane-output status fallback should therefore mark Codex idle only from the
  current placeholder composer near footer text, mark busy from current status
  or approval rows, and preserve `unknown` for stale prompts, generic Codex
  mentions, or historical output.
- Gemini CLI: source analysis at `~/code/upstream/gemini-cli` commit
  `dc47aaa2d` found the current idle prompt in
  `packages/cli/src/ui/components/InputPrompt.tsx`: the prompt prefix renders as
  `>` and the default placeholder renders as `Type your message or
  @path/to/file`. The footer is separately rendered by
  `packages/cli/src/ui/components/Footer.tsx` and commonly places workspace,
  sandbox, and model context directly below the prompt. Pane-output status
  fallback should therefore only mark Gemini idle when that current prompt
  placeholder appears near the bottom of the captured pane. Action/confirmation
  snapshots expose explicit current markers such as `Action Required`, `Apply
  this change?`, and `Allow execution of [...]`; those may be treated as busy.
  Generic mentions of Gemini, historical prompts, or arbitrary `>` lines should
  remain `unknown`.
- opencode: source analysis at `~/code/upstream/opencode` commit `0e118d196`
  found a strong plug-and-play baseline without requiring hooks or wrappers.
  `packages/opencode/package.json` publishes the `opencode` bin at
  `./bin/opencode`. That launcher resolves a platform package named
  `opencode-{darwin,linux,windows}-{arm64,x64}` with baseline and musl variants
  where relevant, then runs `bin/opencode`; source/dev runs enter through
  `packages/opencode/src/index.ts`. agentscan should continue treating
  package-manager shims, platform package binaries, and source entrypoints as
  strong process evidence only when the path shape is exact.
- opencode startup in `packages/opencode/src/index.ts` sets `AGENT=1`,
  `OPENCODE=1`, and `OPENCODE_PID=<pid>`. These are strong Linux process-env
  markers when correlated with the live process tree, but generic argv or text
  mentions of `OPENCODE` should not classify a pane. Worker processes set
  internal `OPENCODE_PROCESS_ROLE` / `OPENCODE_RUN_ID` values, which are useful
  corroborating context but not standalone provider evidence.
- opencode TUI title behavior in
  `packages/opencode/src/cli/cmd/tui/app.tsx` is explicit: the home route sets
  `OpenCode`; session routes set `OpenCode` for default session titles or
  `OC | <session title>` for non-default titles; plugin routes set
  `OC | <plugin id>`. Title updates are optional and can be disabled by
  `OPENCODE_DISABLE_TERMINAL_TITLE`, so title evidence is strong when present
  but process evidence remains necessary for disabled-title and custom-title
  cases. Default opencode titles do not carry run state.
- Current TUI input in
  `packages/opencode/src/cli/cmd/tui/component/prompt/index.tsx` renders idle
  placeholders as `Ask anything... "..."` or shell mode `Run a command...
  "..."`; the session footer in
  `packages/opencode/src/cli/cmd/tui/routes/session/footer.tsx` can expose
  `/status`, LSP/MCP counts, and pending permission counts. Direct interactive
  split-footer mode in `packages/opencode/src/cli/cmd/run/footer.prompt.tsx`
  uses the same `Run a command... "git status"` shell placeholder and the first
  idle prompt `Ask anything... "Fix a TODO in the codebase"`. Pane-output
  fallback should therefore remain provider-scoped, only mark idle when a
  current prompt placeholder appears near the bottom of the capture, and treat
  explicit current markers such as `esc interrupt`, `Permission required`,
  `Allow once`, `Allow always`, `Reject permission`, and question prompts as
  busy. Historical prompt text, generic opencode mentions, and weak footer text
  should remain `unknown`.
- opencode has server and TUI plugin surfaces. Server plugins are loaded through
  `packages/opencode/src/plugin/index.ts` and can observe config, event, tool,
  prompt, and message hooks; TUI plugins are loaded through
  `packages/opencode/src/cli/cmd/tui/plugin/runtime.ts`, and upstream tips
  describe `.opencode/plugins/*.ts` files as event hooks. These surfaces are
  useful future enrichment points for explicit tmux metadata, but they should
  remain optional and deferred because plug-and-play detection already has
  strong title, process, env, and pane-output evidence.
- Hermes Agent: source analysis found package `hermes-agent`, console scripts
  `hermes`, `hermes-agent`, and `hermes-acp`, and a checked-in `hermes`
  wrapper that dispatches to `hermes_cli.main:main`. Upstream does not appear
  to set a process title or tmux title explicitly, so title-only Hermes
  classification should stay out of the baseline. Local tmux probing on
  2026-05-04 used three panes:
  - `agentscan:5.1` idle/new exposed `pane_current_command=python3.11`,
    `pane_title_raw=agentscan: hermes`, and foreground argv
    `/Users/auro/.hermes/hermes-agent/venv/bin/python3 /Users/auro/.local/bin/hermes`.
  - `agentscan:6.1` busy exposed the same tmux/process identity shape and a
    current pane footer containing `⚕ ❯ msg=interrupt · /queue · /bg · /steer · Ctrl+C cancel`.
  - `agentscan:7.1` idle/used exposed the same tmux/process identity shape and
    a current pane prompt line `❯` below the Hermes status bar.
  Hermes baseline detection should therefore prefer metadata, exact foreground
  `hermes` / `hermes-agent` commands, and targeted process evidence for Python
  launchers whose argv points at the Hermes package or bin shim. Pane-output
  status should remain provider-scoped and current-prompt anchored after
  identity is known.
- Aider: source analysis at `~/code/upstream/aider` commit
  `5dc9490bb35f9729ef2c95d00a19ccd30c26339c` found the open-source Apache-2.0
  package `aider-chat`, console script `aider = "aider.main:main"`, and module
  entrypoint `aider/__main__.py` for `python -m aider`. Upstream install docs
  cover the preferred installer plus `uv tool install`, `pipx install`, and
  `python -m pip install` flows, all using `aider-chat`.
- Aider does not appear to set a stable terminal title, process title, tmux
  metadata, OSC status, bottom toolbar, or structured live state. The interactive
  prompt is built through prompt_toolkit and renders a generic `> ` prompt, which
  is too common to use as status evidence. Baseline detection should therefore
  accept explicit metadata aliases, exact foreground `aider` commands, targeted
  `python -m aider` evidence, known `aider-chat` package paths, and Python
  console-script invocations, while leaving status `unknown` unless wrapper
  metadata publishes state or upstream later adds a durable live-state signal.

Closed-source providers require empirical probing and conservative inference.
For each provider, capture snapshots while idle, busy, waiting for input, and
after restart/resume:

- tmux title
- `pane_current_command`
- foreground process group from the pane TTY
- root/descendant process argv and selected environment
- terminal output/status lines only if later justified and scoped to panes that
  already have provider evidence

Local files, sockets, logs, and provider session stores are research-only unless
a future roadmap item explicitly promotes them into an opt-in integration.

Closed-source queue:

- Antigravity CLI: available through local `agy`; the IDE cask launcher and
  terminal-first CLI share the command name but expose different behavior.
- GitHub Copilot CLI: probed locally and documented in
  `docs/notes/copilot-cursor-closed-source-probing.md`.
- Cursor CLI: probed locally through `cursor-agent` and documented in
  `docs/notes/copilot-cursor-closed-source-probing.md`.
- Factory Droid CLI: probed locally through `droid` and documented in
  `docs/notes/droid-cli-support.md`.
- Kimi Code (Moonshot Kimi CLI): probed locally through `kimi` on macOS and
  Linux and documented in `docs/notes/kimi-code-support.md`.

Closed-source probing should produce a written evidence matrix before code
changes. If a signal is weak, agentscan should prefer `unknown` over a richer
but invented classification.

## Closed-Source Implementation Direction

Antigravity / AGY CLI:

- Treat exact live `agy` foreground commands as Antigravity provider evidence.
- Current local probing used the native `~/.local/bin/agy` CLI in an isolated
  tmux session. tmux reported `pane_current_command=agy`, argv `agy`, and a
  generic hostname pane title while the TUI rendered the Antigravity login
  screen.
- The Homebrew cask wrapper at `/opt/homebrew/bin/agy` launches the
  Antigravity IDE app and exits, so it is not itself a durable tmux pane
  signal.
- Official Antigravity CLI docs describe a terminal-first TUI installed as
  `agy`; `agy chat` is not a documented subcommand in the current public CLI
  docs or observed local help.
- Do not infer Antigravity from Gemini-specific titles, packages, slash
  commands, or Google-adjacent wording. Gemini CLI remains `gemini`; AGY may
  import Gemini extensions through `agy plugin import gemini`, but that is not
  Gemini CLI provider evidence.
- Signed-in idle, working, approval, subagent, and browser task-state UI shapes
  were not available in local probing, so baseline status stays `unknown`
  unless explicit pane metadata publishes state. Add provider-scoped pane-output
  status only after current footer/status shapes are captured.

GitHub Copilot CLI:

- Treat exact live `copilot` / `github-copilot` foreground commands as provider
  evidence.
- Current local probing used GitHub Copilot CLI 1.0.39 in an isolated tmux
  session. tmux reported `pane_current_command=node`, default title
  `GitHub Copilot`, and foreground process evidence from the pane TTY resolved
  the native package binary as `copilot`.
- The npm install path uses `@github/copilot/npm-loader.js`, which delegates to
  a platform package such as `@github/copilot-darwin-arm64/copilot`; these
  package paths are strong process-tree evidence even when tmux title updates
  are disabled.
- A custom `--name` value becomes the tmux title, so arbitrary Copilot titles
  should be treated as labels only when process evidence already establishes
  the provider.
- During work, the live pane rendered `Thinking (Esc to cancel)`, but the tmux
  title remained stable. agentscan now treats pane-output status parsing as a
  provider-scoped fallback after Copilot identity is known.
- Baseline status may use a short, provider-scoped pane tail for exact live
  Copilot busy prompts such as `Thinking (Esc to cancel)` and folder-trust
  prompts.
- Baseline status may infer idle only from the anchored current Copilot prompt
  and `/ commands · ? help` footer shape. Stale `Thinking` lines above the
  current prompt should not keep the pane busy.
- Treat `COPILOT_HOME`, `COPILOT_MODEL`, and similar environment variables as
  supporting process context only, not provider identity by themselves.
- Treat Copilot hooks, plugins, statusline/footer customization, and session
  stores as deferred optional integrations.
- Do not read Copilot session-state files or logs in baseline detection.

Cursor CLI:

- Keep exact `cursor-agent` command evidence as the safe baseline.
- Treat bare `agent` as too generic unless future local probing finds strong
  Cursor-specific argv or path evidence.
- Treat `CURSOR_AGENT` and `CURSOR_CLI` environment variables as supporting
  context only, not provider identity by themselves.
- Local probing confirmed the default idle `Cursor Agent` title can be generic
  while `pane_current_command=node`; foreground process evidence still resolves
  `cursor-agent`.
- Baseline status may infer idle from anchored current Cursor footers such as
  `→ Plan, search, build anything` and `→ Add a follow-up`.
- Baseline status may infer busy from anchored current Cursor footer/status
  shapes, including `ctrl+c to stop` and the Cursor spinner plus `Running`
  status line. Ordinary response text containing the word `Running` should not
  drive status.
