#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/notarize-macos-binary.sh [--profile PROFILE] [--team-id TEAM_ID] BINARY

Submits a signed macOS CLI binary to Apple's notary service by wrapping it in a
temporary zip. Bare CLI binaries and zip archives cannot be stapled; the
notarization ticket is associated with the signed code hash.

Environment:
  AGENTSCAN_NOTARY_PROFILE  Keychain profile name. Default: agentscan-notary
  AGENTSCAN_NOTARY_KEYCHAIN Optional keychain path for the stored profile.
  AGENTSCAN_APPLE_TEAM_ID   Apple Developer Team ID.

Example:
  AGENTSCAN_APPLE_TEAM_ID=79S467K965 \
    scripts/notarize-macos-binary.sh target/aarch64-apple-darwin/release/agentscan
USAGE
}

profile="${AGENTSCAN_NOTARY_PROFILE:-agentscan-notary}"
keychain="${AGENTSCAN_NOTARY_KEYCHAIN:-}"
team_id="${AGENTSCAN_APPLE_TEAM_ID:-}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --profile)
      if [[ $# -lt 2 ]]; then
        echo "error: --profile requires a value" >&2
        exit 2
      fi
      profile="$2"
      shift 2
      ;;
    --team-id)
      if [[ $# -lt 2 ]]; then
        echo "error: --team-id requires a value" >&2
        exit 2
      fi
      team_id="$2"
      shift 2
      ;;
    --keychain)
      if [[ $# -lt 2 ]]; then
        echo "error: --keychain requires a value" >&2
        exit 2
      fi
      keychain="$2"
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

if [[ $# -ne 1 ]]; then
  echo "error: exactly one binary path is required" >&2
  usage >&2
  exit 2
fi

if [[ -z "$team_id" ]]; then
  echo "error: team ID is required via --team-id or AGENTSCAN_APPLE_TEAM_ID" >&2
  exit 2
fi

binary="$1"
if [[ ! -f "$binary" ]]; then
  echo "error: binary not found: $binary" >&2
  exit 2
fi

/usr/bin/codesign --verify --strict --verbose=4 "$binary"

workdir="$(/usr/bin/mktemp -d "${TMPDIR:-/tmp}/agentscan-notary.XXXXXX")"
trap 'rm -rf "$workdir"' EXIT

archive="$workdir/$(/usr/bin/basename "$binary").zip"
/usr/bin/ditto -c -k --keepParent "$binary" "$archive"

echo "submitting $archive"
submission_json="$workdir/submission.json"
submit_args=(
  notarytool submit "$archive"
  --keychain-profile "$profile"
  --team-id "$team_id"
  --wait
  --output-format json
)
if [[ -n "$keychain" ]]; then
  submit_args+=(--keychain "$keychain")
fi

/usr/bin/xcrun "${submit_args[@]}" | /usr/bin/tee "$submission_json"

status="$(/usr/bin/sed -n 's/.*"status"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$submission_json" | /usr/bin/head -n 1)"
submission_id="$(/usr/bin/sed -n 's/.*"id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$submission_json" | /usr/bin/head -n 1)"

if [[ "$status" != "Accepted" ]]; then
  if [[ -n "$submission_id" ]]; then
    echo "notarization failed; fetching log for $submission_id" >&2
    log_args=(
      notarytool log "$submission_id"
      --keychain-profile "$profile"
      --team-id "$team_id"
      "$workdir/notary-log.json"
    )
    if [[ -n "$keychain" ]]; then
      log_args+=(--keychain "$keychain")
    fi
    /usr/bin/xcrun "${log_args[@]}" || true
    [[ -f "$workdir/notary-log.json" ]] && /bin/cat "$workdir/notary-log.json" >&2
  fi
  exit 1
fi

echo "notarization accepted: $submission_id"
