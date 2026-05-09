#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/sign-macos-binary.sh [--identity IDENTITY] BINARY...

Signs one or more macOS Mach-O binaries with Developer ID, hardened runtime,
and a secure timestamp.

Environment:
  AGENTSCAN_CODESIGN_IDENTITY  Default signing identity.

Example:
  AGENTSCAN_CODESIGN_IDENTITY="Developer ID Application: Hunter Sadler (79S467K965)" \
    scripts/sign-macos-binary.sh target/aarch64-apple-darwin/release/agentscan
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
    -h|--help)
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

if [[ -z "$identity" ]]; then
  echo "error: signing identity is required via --identity or AGENTSCAN_CODESIGN_IDENTITY" >&2
  exit 2
fi

if [[ $# -eq 0 ]]; then
  echo "error: at least one binary path is required" >&2
  usage >&2
  exit 2
fi

for binary in "$@"; do
  if [[ ! -f "$binary" ]]; then
    echo "error: binary not found: $binary" >&2
    exit 2
  fi

  echo "signing $binary"
  /usr/bin/codesign \
    --force \
    --sign "$identity" \
    --options runtime \
    --timestamp \
    "$binary"

  /usr/bin/codesign --verify --strict --verbose=4 "$binary"
  /usr/bin/codesign -dv --verbose=4 "$binary" 2>&1 \
    | /usr/bin/grep -E 'Identifier|Authority|TeamIdentifier|flags|Timestamp|Runtime|CDHash'
done
