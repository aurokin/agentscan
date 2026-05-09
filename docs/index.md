# Docs Index

This repo uses progressively disclosed documentation.

## Where To Read

- `README.md`
  Operator-facing overview, current scope, quality baseline, and primary commands.
- `ROADMAP.md`
  Durable product direction, boundaries, and decision log.
- `CHANGELOG.md`
  Unreleased user-facing changes and migration notes.
- `docs/architecture.md`
  Runtime model, daemon/socket contract, command families, and architectural guardrails.
- `docs/integration.md`
  Wrapper metadata contract, daemon-backed automation surfaces, shell boundary, and migration posture.
- `docs/harness-engineering.md`
  Validation posture and the rule for what belongs in repo docs versus Linear.
- `docs/macos-release-signing.md`
  Local and GitHub Actions Developer ID signing/notarization workflow for
  macOS release binaries.
- `docs/notes/`
  Narrow follow-up or historical notes that are too specific for the primary docs but still worth keeping in the repo.
  - `docs/notes/macos-daemon-autostart-adr.md`
    ADR for macOS daemon auto-start, executable assessment, observed
    AppleSystemPolicy panics, and the signed-versus-ad-hoc lifecycle boundary.
  - `docs/notes/provider-evidence-roadmap.md`
    Provider-specific evidence plans, plug-and-play detection policy, and the
    deep-roadmap hook/extension direction.
- Linear
  Active milestones, blockers, sequencing, and execution detail.

## Documentation Rule

If a document explains what `agentscan` guarantees, it belongs in the repo. If a
document explains what is currently in progress or blocked, it belongs in
Linear until the behavior settles.
