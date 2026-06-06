# Desktop Release And Smoke Workflow

This document covers the macOS-first desktop app in `desktop/`. The desktop
app is a Tauri shell over the installed `agentscan` CLI; it does not bundle
scanner logic or the CLI binary. The tag-driven GitHub release workflow
publishes a signed and notarized macOS desktop app zip alongside the CLI
artifacts.

## Scope

Use this workflow for local dogfooding, release-candidate verification, and
published release expectations:

- build the React frontend and Tauri app bundle;
- sign and notarize the macOS app bundle;
- package the notarized app bundle for GitHub Releases;
- install the built app locally;
- smoke the local profile, SSH failure path, picker rows, live subscription,
  global hotkey, and focus action.

Do not use this slice to change runtime architecture. Desktop release hardening
should not add scanner code, tmux parsing, provider inference, or daemon socket
clients to the desktop app.

## Version Policy

For now, keep these versions identical:

- `Cargo.toml`
- `desktop/package.json`
- `desktop/src-tauri/Cargo.toml`
- `desktop/src-tauri/tauri.conf.json`

The root Cargo package remains the release tag authority. The desktop metadata
follows it so a dogfood build can be matched to the CLI binary it expects.

Check the invariant before a desktop build:

```sh
scripts/check-desktop-version.sh
```

If the desktop app ever needs an independent cadence, document that decision
first and replace this check with an explicit compatibility matrix.

## Prerequisites

Install the normal project toolchains:

- Rust stable
- Node/npm
- Tauri prerequisites for macOS
- Xcode command line tools
- Apple Developer ID Application certificate in the login keychain
- Apple notarization credentials

The local helper scripts use the same Developer ID certificate and notarytool
profile as the CLI binary workflow in `docs/macos-release-signing.md`:

- `AGENTSCAN_CODESIGN_IDENTITY`
- `AGENTSCAN_APPLE_TEAM_ID`
- `AGENTSCAN_NOTARY_PROFILE`, defaulting to `agentscan-notary`
- `AGENTSCAN_NOTARY_KEYCHAIN`, only when the notary profile is stored in a
  non-default keychain

## Build

From the repo root:

```sh
scripts/check-desktop-version.sh
cargo build --release --locked
cd desktop
npm ci
npm run build
```

Build the unsigned app bundle when you only need a local compile check:

```sh
npm run tauri -- build --bundles app --no-sign -- --locked
```

The signed helper below clears Tauri's `APPLE_*` signing environment while it
builds the bundle, then signs and optionally notarizes through the repo helper
scripts. This keeps the local desktop workflow on one signing path and avoids
re-signing a Tauri-notarized bundle.

Build the signed macOS app bundle for dogfooding:

```sh
AGENTSCAN_CODESIGN_IDENTITY="Developer ID Application: Hunter Sadler (79S467K965)" \
  scripts/build-macos-desktop-app.sh
```

Build, sign, notarize, and staple the app bundle when evaluating a release
candidate:

```sh
AGENTSCAN_CODESIGN_IDENTITY="Developer ID Application: Hunter Sadler (79S467K965)" \
AGENTSCAN_APPLE_TEAM_ID=79S467K965 \
  scripts/build-macos-desktop-app.sh --notarize
```

Expected app bundle:

```sh
desktop/src-tauri/target/release/bundle/macos/agentscan.app
```

## Published Artifact

On `v*` tags, `.github/workflows/release.yml` builds the Apple Silicon desktop
app with `scripts/build-macos-desktop-app.sh --notarize`, verifies the stapled
bundle with `codesign` and `spctl`, packages it with `ditto`, and uploads it to
the GitHub Release as:

```sh
agentscan-desktop-aarch64-apple-darwin.zip
```

The desktop zip is included in `SHA256SUMS` with the CLI tarballs. The app
still preflights and executes a configured `agentscan` binary path; install the
CLI separately through the release tarball or `mise`.

## Verify Signing

Run these checks before installing or sharing the app:

```sh
codesign --verify --deep --strict --verbose=2 \
  desktop/src-tauri/target/release/bundle/macos/agentscan.app

spctl -a -vv -t execute \
  desktop/src-tauri/target/release/bundle/macos/agentscan.app
```

`scripts/build-macos-desktop-app.sh --notarize` runs `codesign` verification,
submits the zipped app bundle to notarytool, staples the accepted ticket, and
validates the staple. `spctl` should report an accepted Developer ID assessment
for a notarized app. If notarization is skipped or credentials are unavailable,
call that out in the smoke notes and do not treat the build as a release
candidate.

## Install For Local Dogfooding

Install by replacing the local app bundle:

```sh
rm -rf /Applications/agentscan.app
ditto desktop/src-tauri/target/release/bundle/macos/agentscan.app \
  /Applications/agentscan.app
```

Keep the CLI installed separately. The desktop app preflights and executes the
configured `agentscan` binary path; it does not embed the CLI binary.

## Smoke Checklist

Record the CLI version, desktop version, commit, signing/notarization result,
and whether each item passed.

Local profile:

- `agentscan --version` prints the expected version.
- `agentscan daemon status --format json` returns valid JSON.
- Desktop local profile preflight succeeds with the configured binary path.
- `agentscan hotkeys --format json` returns picker rows or an honest empty
  result.
- The desktop picker renders the same row set without desktop-side provider or
  status inference.
- `agentscan subscribe --format json` emits a bootstrap frame and live frames
  when pane state changes.
- The desktop live connection banner transitions out of connecting/offline
  state after the daemon is ready.
- `CommandOrControl+Shift+A` shows and focuses the picker window.
- Keyboard selection and a mouse double-click both trigger
  `agentscan focus <pane-id>` through the local profile.

SSH profile basics:

- An invalid host or unauthenticated host reports an SSH auth/network failure
  without changing the local picker state.
- A reachable host with a missing `agentscan` binary reports missing remote
  binary guidance.
- A reachable host with an incompatible or invalid JSON response reports an
  incompatible/misconfigured remote command.
- If a real remote `agentscan` host is available, `hotkeys`, `subscribe`, and
  `focus` use the same CLI command contract over SSH rather than forwarding
  sockets or parsing tmux locally.

Release decision:

- All required quality gates from `README.md` pass.
- The signed app bundle passes `codesign` verification.
- The app bundle passes Gatekeeper assessment with `spctl`.
- The GitHub Release includes `agentscan-desktop-aarch64-apple-darwin.zip` and
  its checksum.
- Any smoke failure is either fixed before release or recorded as a known
  non-release-blocking issue in Linear.
