# Docs Index

This repo uses progressively disclosed documentation.

## Where To Read

- `README.md`
  Operator-facing overview, current scope, quality baseline, and primary commands.
- `ROADMAP.md`
  Durable product direction, boundaries, and decision log.
- `docs/architecture.md`
  Runtime model, cache contract, command families, and architectural guardrails.
- `docs/integration.md`
  Wrapper metadata contract, automation surfaces, shell boundary, and migration posture.
- `docs/harness-engineering.md`
  Validation posture and the rule for what belongs in repo docs versus Linear.
- `docs/notes/`
  Narrow follow-up or historical notes that are too specific for the primary docs but still worth keeping in the repo.
- Linear
  Active milestones, blockers, sequencing, and execution detail.

## Documentation Rule

If a document explains what `agentscan` guarantees, it belongs in the repo. If a
document explains what is currently in progress or blocked, it belongs in
Linear until the behavior settles.
