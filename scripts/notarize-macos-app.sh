#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/notarize-macos-app.sh [--profile PROFILE] [--team-id TEAM_ID] APP_BUNDLE

Submits a signed macOS .app bundle to Apple's notary service, waits for
acceptance, staples the ticket to the app, and validates the staple.

Environment:
  AGENTSCAN_NOTARY_PROFILE   Keychain profile name. Default: agentscan-notary.
  AGENTSCAN_NOTARY_KEYCHAIN  Optional keychain path for the stored profile.
  AGENTSCAN_APPLE_TEAM_ID    Apple Developer Team ID.

Example:
  AGENTSCAN_APPLE_TEAM_ID=79S467K965 \
    scripts/notarize-macos-app.sh desktop/src-tauri/target/release/bundle/macos/agentscan.app
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
  echo "error: macOS app notarization must run on macOS" >&2
  exit 2
fi

if [[ $# -ne 1 ]]; then
  echo "error: exactly one app bundle path is required" >&2
  usage >&2
  exit 2
fi

if [[ -z "$team_id" ]]; then
  echo "error: team ID is required via --team-id or AGENTSCAN_APPLE_TEAM_ID" >&2
  exit 2
fi

app="$1"
if [[ ! -d "$app/Contents" ]]; then
  echo "error: app bundle not found: $app" >&2
  exit 2
fi

/usr/bin/codesign --verify --deep --strict --verbose=4 "$app"

workdir="$(/usr/bin/mktemp -d "${TMPDIR:-/tmp}/agentscan-notary.XXXXXX")"
trap 'rm -rf "$workdir"' EXIT

archive="$workdir/$(/usr/bin/basename "$app").zip"
/usr/bin/ditto -c -k --keepParent "$app" "$archive"

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

set +e
/usr/bin/xcrun "${submit_args[@]}" | /usr/bin/tee "$submission_json"
pipe_status=("${PIPESTATUS[@]}")
set -e

submit_exit="${pipe_status[0]}"
tee_exit="${pipe_status[1]}"

if [[ "$tee_exit" -ne 0 ]]; then
  exit "$tee_exit"
fi

json_field() {
  /usr/bin/plutil -extract "$1" raw -o - "$submission_json" 2>/dev/null || true
}

status="$(json_field status)"
submission_id="$(json_field id)"

if [[ "$status" != "Accepted" || "$submit_exit" -ne 0 ]]; then
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

/usr/bin/xcrun stapler staple "$app"
/usr/bin/xcrun stapler validate "$app"

echo "notarization accepted and stapled: $submission_id"
