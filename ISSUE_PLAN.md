# AUR-181 Issue Plan: Human Review Breaking Surface And Rollout Posture

## Goal

Prepare and record the explicit human approval gate for the daemon socket
migration before unrelated Agentscan provider-roadmap and support work resumes.

This issue is a review and decision checkpoint. It should not change shipped
behavior unless the human review rejects part of the current posture.

## Scope

Confirm these decisions:

| Criterion | Required decision |
|-----------|-------------------|
| Removed `agentscan popup` | Approve or reject no compatibility alias; human picker uses `agentscan tui`. |
| Removed cache IPC | Approve or reject removal of `cache show`, `cache path`, `cache validate`, and `AGENTSCAN_CACHE_PATH`. |
| Direct tmux bypass | Approve or reject limiting direct tmux reads to `agentscan scan` and supported one-shot `--refresh` flags. |
| Cache fallback | Approve or reject no permanent cache fallback in normal shipped behavior. |
| Migration docs | Approve or reject guidance for human picker users, shell aliases, tmux binds, wrapper scripts, normal automation, all-pane automation, raw snapshot consumers, and CI/no-auto-start users. |
| Provider-roadmap unblock | Approve or reject resuming AUR-181-blocked provider-roadmap and support issues after final gates and milestone publication. |

## Non-Goals

- Do not add aliases, compatibility shims, or cache fallback during review.
- Do not push the milestone branch until review and final gates are complete.
- Do not resume provider-roadmap/support work before the review decision is
  recorded.

## Steps

1. Prepare a concise human review packet.
   - Summarize the shipped breaking surfaces.
   - Point at the exact docs/release notes that describe migration targets.
   - Include verification results from AUR-180.

2. Get human confirmation.
   - Ask for explicit approval or rejection of each decision criterion against
     the current local head.
   - If any criterion is rejected, keep AUR-181 open, create or name a follow-up
     Linear issue with owner and acceptance criteria, and land no behavior/docs
     change under AUR-181 except recording the decision.
   - If approved, record the approver, timestamp, reviewed head, and per-criterion
     decisions in Linear.

3. Close the milestone locally after approval.
   - Mark AUR-181 Done.
   - Mark AUR-180 Done only after checking that AUR-181 was its remaining
     blocker, no review follow-up is required, and the reviewed docs commit
     remains in the local head.
   - Rerun the full quality baseline at the reviewed local head before final
     closure, even though AUR-180 already passed.
   - Run `git status -sb`; if an upstream exists, inspect `git log @{u}..HEAD`
     or equivalent ahead/behind state to confirm commits remain local and no
     push happened.
   - Provider-roadmap/support issues may resume only after AUR-181 approval,
     final gates, and milestone publication.

## Review Packet

Current local commits relevant to the review:

- `fe77462` migrates one-shot commands to daemon snapshots.
- `0fb9af4` moves the TUI to daemon socket subscriptions.
- `9915517` removes the cache transport surface.
- `d75a177` finalizes daemon socket docs and release notes.

The reviewed head will be the local branch head after this AUR-181 plan is
committed. At plan time, the behavior/docs posture under review is commit
`d75a177` plus this signoff-only plan update.

Current migration targets:

| Decision area | Current posture |
|---------------|-----------------|
| Human picker | `agentscan tui`; no `agentscan popup` alias |
| Normal automation | `agentscan list --format json` |
| All-pane automation | `agentscan list --all --format json` |
| Raw snapshot envelope | `agentscan snapshot --format json` |
| Direct tmux recovery | `agentscan scan` or supported one-shot `--refresh` flags |
| Daemon opt-out | `--no-auto-start` or `AGENTSCAN_NO_AUTO_START=1`; opt-out does not fall back to direct tmux |
| Socket isolation | `AGENTSCAN_SOCKET_PATH` |
| Removed cache IPC | no `agentscan cache`, no `cache path`, no `cache validate`, no `AGENTSCAN_CACHE_PATH` |

Docs that now carry the durable guidance:

- `CHANGELOG.md`
- `README.md`
- `docs/integration.md`
- `docs/architecture.md`
- `ROADMAP.md`
- `docs/harness-engineering.md`
- `MILESTONE_PLAN.md` as a historical completion record

Verification already run for AUR-180:

- `git diff --check`
- `cargo fmt --all --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_arguments`
- `cargo test`
- targeted docs vocabulary audit for stale cache/popup/future-target wording

Final closure will rerun the same full baseline after human approval.

## Plan Review Notes

Plan review subagent: Euler the 17th.

- Converted scope bullets into an explicit decision matrix.
- Defined rejection handling: AUR-181 stays open, follow-up work gets a named
  Linear issue with owner and acceptance criteria, and AUR-181 records only the
  decision.
- Defined final gates as a full baseline rerun at the reviewed local head.
- Tightened no-push evidence to `git status -sb` and upstream ahead/behind
  inspection when an upstream exists.
- Clarified that provider-roadmap/support issues resume only after approval,
  final gates, and milestone publication.
