# Agent Instructions

## Package Manager
- Use `cargo`: `cargo build`, `cargo test`, `cargo run -- --format text`
- Bare `cargo run -- ...` examples use the default cached `list` flow; direct tmux snapshots use `cargo run -- scan ...`.

## File-Scoped Commands
| Task | Command |
|------|---------|
| Format | `cargo fmt --all` |
| Format check | `cargo fmt --all --check` |
| Lint | `cargo clippy --all-targets --all-features -- -D warnings` |
| Complexity check | `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_arguments` |
| Test | `cargo test` |
| Daemon integration test | `cargo test --test daemon_integration` |
| Benchmark | `cargo bench --bench core_paths -- --noplot` |
| Run default list | `cargo run -- --format text` |
| Run default JSON list | `cargo run -- --format json` |
| Run direct snapshot | `cargo run -- scan --format text` |
| Run direct snapshot JSON | `cargo run -- scan --format json` |

## Quality Baseline
- Keep `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_arguments`, and `cargo test` passing.
- Coverage tooling is not part of the baseline yet; add it intentionally rather than ad hoc.

## Key Conventions
- `agentscan` owns discovery, classification, indexing, caching, and structured outputs for tmux agent panes.
- Plug-and-play detection is a core product invariant. Common agent panes should work without requiring users to install hooks, provider extensions, launch wrappers, or shell integration.
- Provider-specific support should start from upstream source analysis for open-source agents or empirical local probing for closed-source agents. Add heuristics only when the evidence is strong and false-positive risk is understood.
- Provider hooks and extensions are deep-roadmap enrichment only. They may eventually publish richer metadata, but they must not become a prerequisite for baseline detection.
- Keep shell wrappers thin; launch aliases and tmux binds may stay in shell, detection logic should not.
- Prefer tmux metadata and control-mode events over `ps` scans or repeated `capture-pane` calls.
- Do not add a permanent `fast` vs `full` split; the target behavior is one fast path.
- Treat `/proc` inspection as fallback for ambiguous panes, not the primary detection path.
- Treat pane output parsing as provider-scoped status fallback only after provider identity is already established. It must anchor to current prompt/footer/status shapes, avoid stale scrollback, and report provenance as `status.source="pane_output"`.
- Prefer explicit pane metadata via tmux user options when wrappers can publish it.
- Keep output formats stable; preserve machine-readable commands even if display labels change.
- Prefer honest labels from tmux metadata over richer but weakly inferred labels; deeper pane inspection is a later fallback, not a reason to invent display text.
- Do not use TSV as the canonical cache format; use a versioned JSON snapshot for persisted state and keep TSV as an output adapter only.
- Avoid editing `~/.dotfiles` integration during core scanner work unless the task explicitly includes migration.
- Document behavior changes in `ROADMAP.md` when they affect architecture, boundaries, or migration assumptions.
