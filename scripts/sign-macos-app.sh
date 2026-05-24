#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/sign-macos-app.sh [--identity IDENTITY] APP_BUNDLE

Signs a macOS .app bundle with Developer ID, hardened runtime, and a secure
timestamp. Nested Mach-O files are signed before the outer bundle.

Environment:
  AGENTSCAN_CODESIGN_IDENTITY  Default signing identity.

Example:
  AGENTSCAN_CODESIGN_IDENTITY="Developer ID Application: Hunter Sadler (79S467K965)" \
    scripts/sign-macos-app.sh desktop/src-tauri/target/release/bundle/macos/agentscan.app
USAGE
}

identity="${AGENTSCAN_CODESIGN_IDENTITY:-}"

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
    -h | --help)
      usage
      exit 0
      ;;
    --)
      shift
      break
      ;;
    -*)
      echo "error: unknown option $1" >&2
      usage >&2
      exit 2
      ;;
    *)
      break
      ;;
  esac
done

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: macOS app signing must run on macOS" >&2
  exit 2
fi

if [[ -z "$identity" ]]; then
  echo "error: signing identity is required via --identity or AGENTSCAN_CODESIGN_IDENTITY" >&2
  exit 2
fi

if [[ $# -ne 1 ]]; then
  echo "error: exactly one app bundle path is required" >&2
  usage >&2
  exit 2
fi

app="$1"
if [[ ! -d "$app/Contents" ]]; then
  echo "error: app bundle not found: $app" >&2
  exit 2
fi

signed_macho=0
while IFS= read -r -d '' candidate; do
  if /usr/bin/file "$candidate" | /usr/bin/grep -q 'Mach-O'; then
    echo "signing nested code $candidate"
    /usr/bin/codesign \
      --force \
      --sign "$identity" \
      --options runtime \
      --timestamp \
      "$candidate"
    signed_macho=1
  fi
done < <(/usr/bin/find "$app/Contents" -type f -print0)

if [[ "$signed_macho" -eq 0 ]]; then
  echo "error: no Mach-O files found in app bundle: $app" >&2
  exit 2
fi

echo "signing app bundle $app"
/usr/bin/codesign \
  --force \
  --sign "$identity" \
  --options runtime \
  --timestamp \
  "$app"

/usr/bin/codesign --verify --deep --strict --verbose=4 "$app"
/usr/bin/codesign -dv --verbose=4 "$app" 2>&1 \
  | /usr/bin/grep -E 'Identifier|Authority|TeamIdentifier|flags|Timestamp|Runtime|CDHash'
