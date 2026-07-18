# Adding A Provider

This is the playbook for adding (or fixing) support for an agent CLI provider.
It distills the process used for real additions — see
`docs/notes/kimi-code-support.md` and the 0.10.0 CHANGELOG entry for a complete
worked example. Provider support is the most invariant-sensitive kind of
change: read this whole page before writing code.

## Ground Rules

- **Plug-and-play is the product invariant.** Common agent panes must classify
  without hooks, provider extensions, launch wrappers, or shell integration.
  Metadata published via tmux user options is optional enrichment, never a
  prerequisite for baseline detection.
- **Evidence first.** Support starts from upstream source analysis for
  open-source agents, or empirical local probing for closed-source ones. Probe
  in an isolated tmux server (temporary `TMUX_TMPDIR`, dedicated socket — see
  `docs/harness-engineering.md`), never against your live tmux server. Record
  what you observed before encoding anything.
- **Pane output is never provider identity.** It is a provider-scoped status
  fallback only, applied after identity is already established, and it must
  report `status.source="pane_output"`.

## Step 1: Evidence Note

Write `docs/notes/<provider>-support.md` before or alongside the code. It must
record:

- the probed CLI version(s) and platform(s);
- observed idle, busy, and completed pane states (titles,
  `pane_current_command`, process tree, current screen shapes);
- an evidence matrix: each signal, its strength, its baseline use, and its
  false-positive posture — including signals you **rejected** and why;
- unprobed states that deliberately stay `unknown`.

`docs/notes/kimi-code-support.md` is the template in practice.

## Step 2: Ledger Row

Add a row to the Current Providers table in
`docs/notes/provider-evidence-ledger.md` summarizing identity evidence, status
evidence, and caveats. The ledger is the quick map of what evidence supports
each provider; keep it honest about what is conservative or supporting-only.

## Step 3: Identity Classification

Identity evidence is consulted in a fixed order of strength:

1. **metadata** — explicit `@agent.*` tmux user options or aliases accepted by
   `agentscan tmux set-metadata`;
2. **command** — exact command or known package/shim path evidence
   (`src/app/provider.rs`); prefer exact matches — short generic command words
   must never suffix-match;
3. **title** — observed terminal title shapes (`src/app/classify/title.rs`);
   startup titles are usually display labels, not identity, because post-prompt
   titles become arbitrary session text;
4. **process** — targeted process-tree evidence, used only as fallback for
   ambiguous panes, never as the primary path.

Prefer honest labels from tmux metadata over richer but weakly inferred ones.

## Step 4: Pane-Output Status Matcher

If (and only if) the provider's TUI exposes durable busy/idle shapes, add a
matcher module at `src/app/classify/pane_output/<provider>.rs` and register it
in `src/app/classify/pane_output.rs`. Calibration doctrine (also in
CLAUDE.md and the ledger's Strictness Calibration section):

- Calibrate strictness by **failure mode**, not precision alone. Provider TUIs
  restyle between releases; every check must degrade toward `unknown` when its
  assumption breaks — never silently flip busy/idle.
- Anchor on durable primitives: glyph ranges tied to branding, box/border
  shapes, geometry with slack in windows and tails.
- Exact decorative strings (separators, hints, tips) are **corroborators
  only**: their presence may upgrade confidence, their absence routes to
  `unknown` — it must never invert the answer.
- Anchor to the current prompt/footer region; ignore stale scrollback.
- Ambiguous shapes (could be live UI or echoed output) report `unknown` and
  get recorded in the support note, not encoded as a guess.

## Step 5: Tests

Cover, using the existing patterns in `src/app/tests/classification.rs` and
`src/app/tests/provider_classification.rs`:

- identity from each accepted evidence class, and negative cases for the
  rejected signals from your evidence matrix;
- pane-output status: busy, idle, and the degrade-to-`unknown` cases (missing
  corroborator, stale scrollback, unprobed shapes);
- display-label behavior for startup titles;
- icon/display registration touchpoints (`src/app/tests/cli.rs`,
  `src/app/tests/tui.rs`) as the Kimi change did.

### Pane Snapshot Corpus

`tests/fixtures/pane_corpus/` is the regression corpus for provider-scoped
pane-output matchers. Its data-driven walker checks that each captured screen
still produces its expected status, that another provider's matcher does not
claim it accidentally, and that removing or blanking named corroborators can
only preserve the status or degrade it to `unknown` — never invert it.

Each fixture is a text screen and TOML sidecar at:

```text
tests/fixtures/pane_corpus/<provider>/<cli-version>/<state>.txt
tests/fixtures/pane_corpus/<provider>/<cli-version>/<state>.meta.toml
```

`<provider>` must be a registered provider name, `<cli-version>` is the probed
CLI version (use `unversioned` only when it is genuinely unknown), and `<state>`
is `idle`, `busy`, or `waiting`. The sidecar records `provider`, `cli_version`,
capture date (`captured`), terminal geometry (`cols` and `rows`),
`expected_status`, `expected_source = "pane_output"`, `origin`,
`corroborators`, and `allow_other_providers`. Its provider, version, and status
must agree with the directory and filename. Geometry is checked using terminal
display width, not byte length.

To hand-seed a fixture, save the current prompt/footer region as `<state>.txt`,
write the matching sidecar using an existing fixture as the template, and run
`cargo test pane_snapshot_corpus`. Keep enough current TUI chrome to support
the matcher, but do not preserve unrelated or sensitive scrollback. Set
`origin = "hand-seeded"` and record the real CLI version and geometry.

`corroborators` lists exact strings present in the screen whose removal and
same-width blanking the walker mutates independently. Choose decorative chrome
that upgrades confidence; the mutation must return the original status or
`unknown`, never the opposite status. `allow_other_providers` is an explicit,
two-way-checked exception for a screen that legitimately matches another
provider: every named provider must still match, and every unlisted provider
must not. Prefer making matchers more specific over adding an exception.

Frames captured by the real-agent harness can be promoted with:

```bash
scripts/promote-e2e-frames.sh <run-id> <provider> <frame-file> <state> <cli-version>
```

`<frame-file>` may be a pane-tail artifact name such as
`pane-tail-busy.txt` or a path relative to that provider's artifact directory.
The script copies it into the corpus and prefills the sidecar; it refuses to
overwrite existing fixtures unless `--force` is passed first. Review the
screen, fill in `corroborators`, adjust the byte-derived `cols` prefill if its
terminal display width differs, and run `cargo test pane_snapshot_corpus`.

## Step 6: E2E Catalog Entry

Add a `[providers.<name>]` entry to `tests/provider_e2e/catalog.toml` (command,
expected provider, expected match kinds, prompt/completion marker, timeouts,
startup steps) so the local real-agent lifecycle harness can exercise the
provider. The harness is local and opt-in — see `docs/harness-engineering.md`
for the run contract and spend/auth boundaries.

## Step 7: CHANGELOG Stanza

Add an entry under `## Unreleased` in `CHANGELOG.md` describing the support in
user-facing terms: what classifies the pane, what the status fallback anchors
on, icons, and where the evidence lives. The 0.10.0 Kimi Code stanza is the
model.

## Quality Baseline

Before sending the change, the standard gates must pass:

```
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_arguments
cargo test
```

Desktop-visible additions (provider logos in `desktop/src/assets/providers/`,
`desktop/src/providerLogos.ts`) also need `pnpm build` and `pnpm test` in
`desktop/`.
