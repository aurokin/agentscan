#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/build-macos-desktop-app.sh [--identity IDENTITY] [--notarize]

Builds the Tauri desktop app bundle and signs it with Developer ID. With
--notarize, the script also submits the signed app to Apple's notary service
and staples the accepted ticket.

Environment:
  AGENTSCAN_CODESIGN_IDENTITY  Default signing identity.
  AGENTSCAN_APPLE_TEAM_ID      Required when --notarize is used.
  AGENTSCAN_NOTARY_PROFILE     Keychain profile name. Default: agentscan-notary.
  AGENTSCAN_NOTARY_KEYCHAIN    Optional keychain path for the stored profile.

Example:
  AGENTSCAN_CODESIGN_IDENTITY="Developer ID Application: Hunter Sadler (79S467K965)" \
    scripts/build-macos-desktop-app.sh
USAGE
}

identity="${AGENTSCAN_CODESIGN_IDENTITY:-}"
notarize=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --identity)
      if [[ $# -lt 2 ]]; then
        echo "error: --identity requires a value" >&2
        exit 2
      fi
      identity="$2"
      shift 2
      ;;
    --notarize)
      notarize=1
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown option $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: macOS desktop app builds must run on macOS" >&2
  exit 2
fi

if [[ -z "$identity" ]]; then
  echo "error: signing identity is required via --identity or AGENTSCAN_CODESIGN_IDENTITY" >&2
  exit 2
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
app="$repo_root/desktop/src-tauri/target/release/bundle/macos/agentscan.app"

"$script_dir/check-desktop-version.sh"

(
  cd "$repo_root/desktop"
  pnpm install --frozen-lockfile
  env \
    -u APPLE_CERTIFICATE \
    -u APPLE_CERTIFICATE_PASSWORD \
    -u APPLE_ID \
    -u APPLE_PASSWORD \
    -u APPLE_PROVIDER_SHORT_NAME \
    -u APPLE_SIGNING_IDENTITY \
    -u APPLE_TEAM_ID \
    pnpm run tauri -- build --bundles app --no-sign -- --locked
)

"$script_dir/sign-macos-app.sh" --identity "$identity" "$app"

if [[ "$notarize" -eq 1 ]]; then
  "$script_dir/notarize-macos-app.sh" "$app"
fi

echo "$app"
