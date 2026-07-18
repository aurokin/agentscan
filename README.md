<p align="center">
  <img src="assets/agentscan-logo.png" alt="agentscan" width="280" />
</p>

# agentscan

See every AI coding agent running in your tmux sessions — Claude Code, Codex,
Gemini, Copilot, Cursor, Aider, and more — in one place: which panes are busy,
which are idle and waiting on you, and jump straight to any of them. One fast
Rust binary gives you a CLI, an interactive TUI picker, and a macOS desktop
dock, all driven by tmux metadata with no hooks, wrappers, or shell
integration to install.

```console
$ agentscan
👾 api:1.0 - fix flaky auth tests
💭 api:2.1 - codex
✨ dotfiles:1.0 - gemini
👾 blog:3.0 - draft release notes
```

<!-- TODO: replace the sample above with a real GIF of `agentscan tui` recorded
     from live agent sessions (assets/agentscan-tui.gif). -->

`agentscan tui` opens the same list as an interactive picker with busy/idle
status and single-key jump-to-pane; the desktop app puts it in a dock on your
Mac. Detection is plug-and-play: common agent panes are recognized from tmux
metadata alone, and status falls back to `unknown` rather than guessing.

## Requirements

- **tmux 3.2 or newer.** Live pane updates rely on tmux control-mode
  `refresh-client -B` subscriptions, introduced in tmux 3.2. On older tmux the
  daemon still starts but never receives live events, so pane status can appear
  stale; `agentscan doctor` warns when the installed tmux is too old.
- **macOS (Apple Silicon) or Linux (x86_64 / ARM64).** Prebuilt CLI binaries
  are published for those targets only, as a deliberate distribution decision
  (not an interim gap): Intel Macs are on Apple's way out and there is no Intel
  hardware in this project's test fleet to verify artifacts on. On an Intel
  Mac, build from source. The desktop app is macOS Apple Silicon only.

## Install

With [Homebrew](https://brew.sh/) (Apple Silicon macOS and Linux; the tap is
[aurokin/homebrew-tap](https://github.com/aurokin/homebrew-tap)):

```sh
brew install aurokin/tap/agentscan
brew install --cask aurokin/tap/agentscan-desktop   # macOS desktop app
```

With [mise](https://mise.jdx.dev/) (uses [ubi](https://github.com/houseabsolute/ubi)
under the hood):

```sh
mise use -g ubi:aurokin/agentscan@latest
```

Or download a tarball for your platform from the
[latest release](https://github.com/aurokin/agentscan/releases/latest) and verify
it against `SHA256SUMS` before extracting:

```sh
# in the directory holding the downloaded tarball and SHA256SUMS
sha256sum --check SHA256SUMS
tar -xzf agentscan-aarch64-apple-darwin.tar.gz   # pick the tarball for your platform
```

Release artifacts:

- `agentscan-aarch64-apple-darwin.tar.gz` — macOS Apple Silicon CLI
- `agentscan-x86_64-unknown-linux-gnu.tar.gz` — Linux x86_64 CLI
- `agentscan-aarch64-unknown-linux-gnu.tar.gz` — Linux ARM64 CLI
- `agentscan-desktop-aarch64-apple-darwin.zip` — macOS desktop app (signed & notarized, Apple Silicon)

### Build from source

Requires a [Rust toolchain](https://rustup.rs/) (edition 2024):

```sh
cargo build --release
# binary at target/release/agentscan
```

### Updating

Updates are manual by design — neither the CLI nor the desktop app modifies
itself. The desktop app's Settings window shows an "Update available" hint
when a newer release is published (a day-cached, display-only check against
GitHub Releases that stays silent offline).

```sh
brew upgrade agentscan                        # if installed via Homebrew
brew upgrade --cask agentscan-desktop         # desktop app via Homebrew
mise up                                       # if installed via mise/ubi
# or download the new tarball / desktop zip from GitHub Releases
```

After updating the CLI, restart the daemon so it runs the new binary:

```sh
agentscan daemon restart
```

## Quickstart

Have tmux running with at least one agent session (for example, a pane running
`claude` or `codex`), then:

```sh
# List agent panes across your tmux server (default command)
agentscan

# Interactive picker: busy/idle status, press a key to jump to that pane
agentscan tui

# Check your environment: tmux version, daemon health, config
agentscan doctor
```

The first run auto-starts a background daemon that indexes tmux panes over
control mode; later commands read from it. Each line shows a provider icon,
the pane's tmux address (`session:window.pane`), and a label taken from the
pane's title or metadata. A tmux popup key bind works well for the TUI, e.g.:

```tmux
bind-key a display-popup -E -w 80% -h 60% "agentscan tui"
```

If something looks wrong, start with `agentscan doctor`. It is read-only — it
never mutates tmux or daemon state and never auto-starts a daemon — and bundles
binary version and macOS trust, config validity, tmux reachability, daemon
health, a discovery summary, and the picker contract into one checklist. See
`docs/daemon-operations.md` for daemon lifecycle and troubleshooting.

## Privacy & security

Everything runs locally. agentscan reads tmux pane metadata (commands, titles,
paths, user options) and, as a last-resort status check for already-identified
providers, the current on-screen content of a pane — never scrollback,
transcripts, provider logs, or session stores. Nothing is uploaded or phoned
home; there is no telemetry. The desktop app's only network call is the
GitHub Releases update check described above, and it stays silent offline. To
report a vulnerability, see [SECURITY.md](SECURITY.md).

## Commands

- `agentscan` / `agentscan list` — list agent panes (daemon-backed)
- `agentscan tui` — interactive picker (interactive-only, not for scripts)
- `agentscan focus <pane_id>` — jump to a pane
- `agentscan inspect <pane_id>` — one-pane diagnostics: provider evidence,
  status source, classification reasons
- `agentscan doctor` — read-only environment and daemon health report
- `agentscan scan` — direct tmux snapshot, bypassing the daemon
- `agentscan daemon start|run|status|stop|restart` — daemon lifecycle
- `agentscan snapshot` / `agentscan subscribe` — raw snapshot envelope / live
  JSON Lines events
- `agentscan providers` / `agentscan hotkeys` / `agentscan hotkey <key>` —
  provider and picker metadata surfaces
- `agentscan tmux hotkey|set-metadata|clear-metadata` — tmux-facing helpers

For repo-local tmux `display-popup` testing without installing the binary on
`PATH`, use `tmux display-popup -E "$PWD/target/debug/agentscan" tui` after
building once. For local ad-hoc macOS builds or debugging detached-start
failures, run the daemon in the foreground with `agentscan daemon run`.

## Configuration

`agentscan` reads optional user configuration from:

```toml
# ${XDG_CONFIG_HOME:-~/.config}/agentscan/config.toml
icons = "emoji"
picker_group_by = "session"
picker_keys = [
  "1", "2", "3", "4", "5",
  "Q", "E", "R", "F", "G", "T",
  "Z", "X", "C", "V", "B",
]
disable_reconcile = true
disable_proc_fallback = false
```

`picker_group_by` accepts `session`, `git-repo`, or `cwd`. `session` preserves
the default tmux-location order. `git-repo` and `cwd` group and order picker rows
by workspace context first, then by `session:window.pane`.

Supported icon modes:

- `emoji`: default provider icons for terminals without Nerd Font coverage
- `nerd-font`: current Nerd Font provider icons
- `nerd-font-patched`: custom agent glyphs from the `agent-icons-v9` patched
  font manifest; requires a terminal font patched with those private-use
  codepoints

Icon mode precedence is CLI, then environment, then config file, then default:

```sh
agentscan list --icons nerd-font
AGENTSCAN_ICONS=nerd-font-patched agentscan tui
```

Picker keys use the config file only. If omitted, the default order is
`1 2 3 4 5 Q E R F G T Z X C V B`. Custom keys remap those 16 selection
slots, so the list must contain exactly 16 unique single ASCII letters or
digits; letters are normalized case-insensitively. `N` and `P` are reserved for
TUI paging.

Runtime toggles use environment values first, then config file values, then
built-in defaults:

```sh
AGENTSCAN_DISABLE_RECONCILE=0 agentscan daemon run
AGENTSCAN_DISABLE_PROC_FALLBACK=1 agentscan daemon run
```

`disable_reconcile` defaults to `true`: the daemon's event-driven path is
authoritative and the connect/reconnect bootstrap recovers ground truth, so the
periodic reconcile polling loop is off unless you set `disable_reconcile =
false` to re-enable it. `disable_proc_fallback` defaults to `false`; setting it
to `true` skips process-tree inspection for ambiguous panes. The daemon reads
both on startup.

`agentscan providers` previews the active text icon mode, and
`agentscan providers --format json` exposes every icon mode and codepoint for
scripts or font tweaking.

## Automation & JSON output

`agentscan list --format json` is the supported machine-readable surface
(`--all` to include non-agent panes). `agentscan snapshot --format json`
exposes the raw versioned snapshot envelope, `agentscan subscribe --format
json` streams live JSON Lines daemon events, and `doctor`, `daemon status`,
`providers`, and `hotkeys` all take `--format json` too. `agentscan tui` is
interactive-only and never a data contract.

See `docs/integration.md` for the full automation contract and
`docs/notes/automation-migration.md` for migration off removed surfaces
(`popup`, `cache`, TUI-shaped output).

## Docs

- `docs/index.md`: map of the repo's progressively disclosed documentation
- `docs/architecture.md`: runtime model, daemon/socket contract, command
  families, and guardrails
- `docs/integration.md`: wrapper metadata, automation surfaces, and the shell
  boundary
- `docs/daemon-operations.md`: daemon auto-start, status, telemetry, and
  troubleshooting
- `docs/desktop.md`: desktop app operation, local/SSH profiles, and picker
  behavior
- `ROADMAP.md`: durable product direction, boundaries, and decision log
- `CHANGELOG.md`: unreleased user-facing changes and migration notes

Background notes: `docs/notes/shipped-scope.md` (detailed capability
inventory), `docs/notes/automation-migration.md` (automation contract and
migration), and `docs/notes/reference-shell-workflow.md` (the shell workflow
this project replaced).

## Quality Gates

Current local baseline:

- `cargo fmt --all --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_arguments`
- `cargo test`

Desktop shell checks:

- `cd desktop && pnpm build`
- `cd desktop && pnpm test`
- `cargo test --manifest-path desktop/src-tauri/Cargo.toml`
- `cd desktop && pnpm tauri dev`
- `scripts/check-desktop-version.sh`

Test coverage includes committed file-based fixtures for representative tmux
title snapshots and snapshot envelopes, property tests for parser and
normalization invariants, and isolated daemon integration tests that start a
temporary tmux server and assert live state behavior. Performance is tracked
with `cargo bench --bench core_paths -- --noplot` against committed fixtures.
