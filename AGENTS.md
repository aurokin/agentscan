# Agent Instructions

## Package Manager
- Use `cargo`: `cargo build`, `cargo test`, `cargo run -- --format text`

## File-Scoped Commands
| Task | Command |
|------|---------|
| Format | `cargo fmt --all` |
| Test | `cargo test` |
| Run scanner | `cargo run -- --format text` |
| Run JSON output | `cargo run -- --format json` |

## Commit Attribution
- AI commits MUST include:
```text
Co-Authored-By: <agent name> <email>
```

## Key Conventions
- `agentscan` owns discovery, classification, indexing, caching, and structured outputs for tmux agent panes.
- Keep shell wrappers thin; launch aliases and tmux binds may stay in shell, detection logic should not.
- Prefer tmux metadata and control-mode events over `ps` scans or repeated `capture-pane` calls.
- Do not add a permanent `fast` vs `full` split; the target behavior is one fast path.
- Treat `/proc` inspection as fallback for ambiguous panes, not the primary detection path.
- Prefer explicit pane metadata via tmux user options when wrappers can publish it.
- Keep output formats stable; preserve machine-readable commands even if display labels change.
- Do not use TSV as the canonical cache format; use a versioned JSON snapshot for persisted state and keep TSV as an output adapter only.
- Avoid editing `~/.dotfiles` integration during core scanner work unless the task explicitly includes migration.
- Document behavior changes in `ROADMAP.md` when they affect architecture, boundaries, or migration assumptions.
