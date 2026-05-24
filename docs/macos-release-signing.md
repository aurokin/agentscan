# macOS Release Signing

`agentscan` macOS release binaries should be Developer ID signed, hardened
runtime enabled, timestamped, and accepted by Apple's notary service before
publishing.

## Why This Matters

Signing is part of the macOS daemon lifecycle policy, not just release
polish. macOS release binaries are signed and notarized so detached daemon
starts, including implicit auto-start from daemon-backed commands, can run
through a trusted Apple policy assessment path.

Ad-hoc or local development builds should use foreground `agentscan daemon run`
instead of detached background startup. This boundary exists because this host
observed repeated kernel panics naming `agentscan` as the panicked task while
the kernel backtrace was in `AppleSystemPolicy` during process launch or
assessment. See
`docs/adr/macos-daemon-autostart-and-executable-assessment.md` for the incident
record and product policy.

## Local Signing

Prerequisites:

- A valid `Developer ID Application` certificate with private key in the login
  keychain.
- A notarytool keychain profile, created once:

```sh
xcrun notarytool store-credentials agentscan-notary \
  --apple-id "APPLE_ID_EMAIL" \
  --team-id 79S467K965
```

Sign a local binary:

```sh
AGENTSCAN_CODESIGN_IDENTITY="Developer ID Application: Hunter Sadler (79S467K965)" \
  scripts/sign-macos-binary.sh target/aarch64-apple-darwin/release/agentscan
```

Submit the signed binary for notarization:

```sh
AGENTSCAN_APPLE_TEAM_ID=79S467K965 \
  scripts/notarize-macos-binary.sh target/aarch64-apple-darwin/release/agentscan
```

The notarization helper wraps the CLI in a temporary zip because `notarytool`
submits archives. Bare CLI binaries and zip archives cannot be stapled; the
notary ticket is associated with the signed code hash.

## Local Desktop App Signing

The desktop app uses the same Developer ID posture, but signs and notarizes a
Tauri `.app` bundle instead of a bare CLI binary. The local desktop workflow
lives in `docs/desktop-release-smoke.md` and uses these helpers:

```sh
AGENTSCAN_CODESIGN_IDENTITY="Developer ID Application: Hunter Sadler (79S467K965)" \
  scripts/build-macos-desktop-app.sh

AGENTSCAN_CODESIGN_IDENTITY="Developer ID Application: Hunter Sadler (79S467K965)" \
AGENTSCAN_APPLE_TEAM_ID=79S467K965 \
  scripts/build-macos-desktop-app.sh --notarize
```

`scripts/sign-macos-app.sh` signs nested Mach-O files before signing the outer
bundle. `scripts/notarize-macos-app.sh` submits a zipped app bundle, waits for
acceptance, staples the ticket, and validates the staple. Unlike bare CLI
binaries, notarized `.app` bundles can be stapled.

## GitHub Actions Secrets

The release workflow signs and notarizes only the `aarch64-apple-darwin`
artifact. Configure these repository secrets:

- `APPLE_DEVELOPER_IDENTITY`: signing identity name, for example
  `Developer ID Application: Hunter Sadler (79S467K965)`
- `APPLE_DEVELOPER_ID_CERTIFICATE_BASE64`: base64-encoded `.p12` export of the
  Developer ID certificate and private key
- `APPLE_DEVELOPER_ID_CERTIFICATE_PASSWORD`: password used when exporting the
  `.p12`; leave empty only if the `.p12` was exported without a password
- `APPLE_KEYCHAIN_PASSWORD`: random CI-only password used for the temporary
  keychain
- `APPLE_ID`: Apple ID email for notarization
- `APPLE_APP_SPECIFIC_PASSWORD`: app-specific password for `APPLE_ID`
- `APPLE_TEAM_ID`: Apple Developer Team ID, for example `79S467K965`

Create the `.p12` secret from an exported Developer ID certificate:

1. Open Keychain Access.
2. Select the `login` keychain and the `My Certificates` category.
3. Expand `Developer ID Application: ...` and confirm the private key is nested
   underneath the certificate. Exporting only the certificate is not enough.
4. Select the certificate row and its private key, then choose
   `File > Export Items...`.
5. Save as `DeveloperIDApplication.p12` and set an export password unless you
   intentionally want an empty `.p12` password. Store that password as
   `APPLE_DEVELOPER_ID_CERTIFICATE_PASSWORD`; for an empty-password export, set
   the GitHub secret to an empty value.

Verify the exported file contains a usable signing identity before adding it to
GitHub:

```sh
tmp_keychain="$(mktemp -u /tmp/agentscan-signing.XXXXXX.keychain-db)"
security create-keychain -p test-password "$tmp_keychain"
security unlock-keychain -p test-password "$tmp_keychain"
security import DeveloperIDApplication.p12 \
  -P "P12_EXPORT_PASSWORD" \
  -A \
  -t cert \
  -f pkcs12 \
  -k "$tmp_keychain"
security find-identity -v -p codesigning "$tmp_keychain"
security delete-keychain "$tmp_keychain"
```

The verification output should include the same Developer ID Application
identity used in `APPLE_DEVELOPER_IDENTITY`.

Base64-encode the `.p12` for the GitHub secret:

```sh
base64 -i DeveloperIDApplication.p12 | pbcopy
```

Use the clipboard contents as `APPLE_DEVELOPER_ID_CERTIFICATE_BASE64`. Do not
commit the `.p12`, the base64 output, or the export password to the repo.

## Release Behavior

For macOS releases, `.github/workflows/release.yml`:

1. Builds the release binary.
2. Imports the Developer ID certificate into a temporary keychain.
3. Signs the binary with hardened runtime and secure timestamp.
4. Stores notarization credentials in the temporary keychain.
5. Submits the signed binary to Apple's notary service and waits for acceptance.
6. Packages the signed/notarized binary into the release tarball.

Linux artifacts are built and packaged without Apple signing.
