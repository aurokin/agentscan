# ADR: Real Agent Lifecycle E2E Testing

Status: accepted
Date: 2026-06-28

## Context

`agentscan` already has fixture, unit, and isolated tmux integration tests for
parser behavior, provider classification, daemon snapshots, metadata updates,
and pane-output status fallback. Those tests are the right default quality
baseline because they are deterministic and cheap.

They do not fully answer whether real coding agents still behave the way our
detection rules assume. Provider CLIs change title formats, prompt/footer
layouts, process trees, model flags, and busy/idle surfaces. The most important
product question is not just "can this pane be classified once?" It is whether a
real agent pane follows the lifecycle that users see:

1. the agent starts;
2. `agentscan` detects the provider;
3. the agent reaches a ready/available state;
4. a prompt is submitted through the real TUI;
5. `agentscan` flips the pane to busy and keeps it busy while work is running;
6. the agent completes;
7. `agentscan` returns the pane to ready/available without losing provider
   identity.

Real verification may require authenticated provider CLIs and can spend model
tokens. It should therefore be explicit, provider-selectable, and locally
controlled rather than part of the default CI baseline.

## Decision

Add a real-agent lifecycle e2e testing system as a local opt-in validation
surface for provider detection rules.

The harness should launch actual coding agents inside an isolated tmux server,
observe `agentscan` through the same supported snapshot/subscription surfaces as
normal consumers, drive a small prompt through the agent TUI, and assert the
full lifecycle:

```text
startup -> detected -> ready -> busy continuously -> ready
```

The harness is not a CI gate at this stage. It is intended for local,
on-request, provider-specific validation because real coverage depends on local
installs, auth state, provider availability, and model spend.

## Provider Selection

Provider execution must be granular.

The runner should support:

- listing runnable provider names without starting agents;
- running one provider by name;
- running multiple named providers in one invocation;
- running all configured providers only through an explicit opt-in flag.

There should be no implicit "run everything" behavior. If a user starts the
runner without a provider selection, it should print available providers and
ask for an explicit choice through command usage rather than launching agents.

An `--all` style flag is still valuable for deliberate local sweeps, but it must
be opt-in because each provider may require credentials and spend.

## Model And Effort Catalog

The e2e system should use a maintained catalog for provider launch parameters,
including command, startup args, supported model names, default model, default
effort or reasoning level, prompt submission behavior, and provider-specific
readiness or completion hints.

Model and effort choices must be overrideable without editing the shared
catalog. The intended precedence is:

1. repository catalog defaults;
2. local ignored override file for a developer machine;
3. CLI overrides for a single run.

This keeps normal probing reproducible while still allowing a developer to
select cheaper, faster, slower, or provider-specific models as needed.

The catalog must not become a detection input. It controls how tests launch and
drive agents; product detection still comes from live tmux metadata, terminal
titles, process evidence, and provider-scoped pane-output status fallback.

## Spend And Auth Boundary

Submitting prompts to real providers requires an explicit spend/auth opt-in.
The runner may start a provider and check startup/ready detection without this
opt-in when the provider can do so safely, but it must not submit a prompt that
could trigger model usage unless the user has deliberately enabled that mode.

The default prompt should be tiny, deterministic, and easy to detect in the
pane output, such as asking the agent to print a unique completion marker.
Provider catalog entries may use a slightly longer prompt when a CLI can
complete the marker-only request before the harness observes a stable busy
state. Those prompts should still be bounded, cheap, non-mutating, and should
ask for the completion marker indirectly so echoed prompt text cannot satisfy
completion. The prompt should not require repository mutation, network access
beyond the provider call, or long-running work.

Secrets, API keys, access tokens, home-directory paths, and provider account
details must be redacted from logs and artifacts wherever practical.

## Lifecycle Assertions

The runner should collect a timeline from live `agentscan` observations rather
than relying only on a final snapshot.

The local runner may shorten daemon reconcile/self-heal intervals for the
isolated daemon it owns. That keeps pane-output-only status checks bounded
without changing production defaults.

For each provider run, it should assert:

- provider identity appears within a bounded startup timeout;
- classification provenance matches the accepted evidence class for that
  provider unless the case explicitly allows alternatives;
- the pane reaches ready/available before prompt submission;
- after prompt submission, busy appears within a bounded timeout;
- after first busy, the pane does not report ready/available again until the
  completion marker or provider-specific completion signal is observed;
- after completion, the pane returns to ready/available within a bounded
  timeout;
- provider identity remains stable through the whole run;
- status provenance is reported honestly, especially when
  `status.source="pane_output"` wins.

Transient startup unknown states are acceptable. A false ready state during
active work is not acceptable unless the provider behavior is explicitly
documented and the detection rule is revised with that limitation.

## Provider Outcomes

Each provider starts as `unknown`. The runner promotes it only when it has enough
local evidence:

- `unknown`: the configured provider command is not installed or not on `PATH`.
- `blocked`: the command exists, but auth, install, rate limit, consent, or
  environment state prevents the lifecycle check from producing product signal.
- `failed`: the provider launched far enough to exercise agentscan, and a
  provider identity/status/lifecycle assertion failed.
- `success`: the provider completed the lifecycle check.

The command should exit non-zero only for `failed` outcomes or runner errors.
This keeps a local machine's installation inventory separate from compatibility
regressions against the latest provider CLIs.

## Failure Classification

The runner should report lifecycle failures as product diagnostics, not only as
generic timeouts. Each timeout should include the last structured pane
observation and a small pane-tail excerpt so the owner can distinguish these
cases:

- `target_pane_not_observed`: the daemon or harness did not report the launched
  pane.
- `provider_identity_missing`: the target pane appeared, but agentscan did not
  identify it as a provider.
- `provider_identity_mismatch`: agentscan identified the target pane as a
  different provider than the e2e catalog expected.
- `provider_status_matcher_update_needed`: the expected provider was detected,
  but status stayed `unknown` or `not_checked`; this is the expected report when
  a provider UI shape changed and the provider-specific status matcher needs an
  update.
- `provider_status_mismatch`: agentscan reported a concrete status different
  from the lifecycle expectation; the pane tail determines whether this is true
  provider behavior or stale status reporting.

## Artifacts

Every run should write diagnostics under `target/` so failed transitions can be
debugged without rerunning a paid provider call.

Useful artifacts include:

- timeline JSONL from snapshot or subscription observations;
- final snapshot JSON;
- `inspect` JSON before and after the prompt;
- tmux `list-panes` output for the target pane;
- captured pane tails before prompt, during busy, and after completion;
- daemon stdout/stderr;
- runner metadata with provider, model, effort, prompt id, timeout settings, and
  redacted command details.

Artifacts are diagnostics only. They are not canonical state and should not
become product inputs.

## Relationship To Existing Tests

The existing deterministic harnesses remain the baseline for `cargo test`.
They should continue to cover parser behavior, classifier decisions, daemon
socket behavior, false-positive cases, pane-output matchers, and snapshot
contracts.

Real-agent e2e tests complement those harnesses by catching provider drift and
validating lifecycle behavior against actual CLIs. When a real-agent e2e run
finds drift, the follow-up should be to update the evidence notes and add or
adjust deterministic tests for the accepted signal. The real run is a probe and
validation surface, not the only place a rule should be encoded.

## Consequences

- Provider detection work can be validated against real CLIs without making CI
  depend on credentials, network availability, or token spend.
- Developers can run a narrow provider check when changing one detection rule,
  or deliberately run all configured providers before a release.
- Model and effort settings are explicit and reproducible, reducing accidental
  expensive runs while still allowing realistic coverage.
- Lifecycle regressions become visible: especially false idle reports during
  active work and failure to return to ready after completion.
- The harness reinforces the product invariant that pane output may refine
  status only after provider identity is established by stronger evidence.

## Non-Goals

- Do not make real-agent e2e runs part of the default CI baseline.
- Do not require provider hooks, wrappers, shell integration, or metadata for
  baseline detection cases.
- Do not use the e2e catalog as product detection evidence.
- Do not submit model-spending prompts without explicit local opt-in.
- Do not treat historical transcripts, provider logs, or session databases as
  core detection inputs.
- Do not weaken deterministic tests because real-agent probes exist.
