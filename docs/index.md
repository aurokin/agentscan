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
- `docs/desktop-release-smoke.md`
  macOS desktop app build, signing, notarization, install, and smoke workflow.
- `docs/desktop-platform-posture.md`
  Desktop platform posture, macOS-specific pieces, adapter seams, and deferred
  Linux/Windows work.
- `docs/adr/`
  Architecture decision records for durable product and implementation
  decisions.
  - `docs/adr/macos-daemon-autostart-and-executable-assessment.md`
    ADR for macOS daemon auto-start, executable assessment, observed
    AppleSystemPolicy panics, and the signed-versus-ad-hoc lifecycle boundary.
  - `docs/adr/desktop-shell-and-shared-client-contract.md`
    ADR for the desktop shell stack, macOS-first posture, and shared
    TUI/desktop command-runner client contract.
- `docs/notes/`
  Narrow follow-up or historical notes that are too specific for the primary docs but still worth keeping in the repo.
  - `docs/notes/provider-evidence-roadmap.md`
    Provider-specific evidence plans, plug-and-play detection policy, and the
    deep-roadmap hook/extension direction.
  - `docs/notes/copilot-cursor-closed-source-probing.md`
    Closed-source Copilot and Cursor probing checklist, evidence matrix, and
    accepted versus rejected classifier signals.
  - `docs/notes/droid-cli-support.md`
    Factory Droid CLI probing evidence, accepted signals, and baseline
    detection/status behavior.
  - `docs/notes/daemon-redesign-brief.md`
    Prepared target shape, migration slices, and harness risks for the future
    daemon redesign.
  - `docs/notes/daemon-redesign-decisions.md`
    Running implementation decision log for daemon redesign slices.
  - `docs/notes/desktop-spike-closeout.md`
    Stop/go decision, evidence, known gaps, and follow-up backlog map for the
    first macOS desktop spike.
- Linear
  Active milestones, blockers, sequencing, and execution detail.

## Documentation Rule

If a document explains what `agentscan` guarantees, it belongs in the repo. If a
document explains what is currently in progress or blocked, it belongs in
Linear until the behavior settles.
