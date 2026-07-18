#!/usr/bin/env bash

set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
catalog="$repo_root/tests/provider_e2e/catalog.toml"
state_file="$repo_root/provider-versions.json"
file_issues=false

if [[ ${1:-} == "--file-issues" ]]; then
  file_issues=true
  shift
fi

if (( $# > 0 )); then
  echo "Usage: $0 [--file-issues]" >&2
  exit 2
fi

if [[ -n ${GITHUB_ACTIONS:-} ]]; then
  file_issues=true
fi

# Validate once up front: malformed committed state is a script error, not a
# transient provider-registry failure.
jq -e 'type == "object"' "$state_file" >/dev/null

providers=()
while IFS= read -r provider; do
  providers+=("$provider")
done < <(grep '^\[providers\.' "$catalog" | sed -E 's/^\[providers\.([^]]+)\]$/\1/')

fetch_latest() {
  local source=$1
  local package=$2

  case "$source" in
    npm)
      local encoded_package
      encoded_package=$(jq -nr --arg package "$package" '$package | @uri')
      curl --fail --silent --show-error --location \
        "https://registry.npmjs.org/${encoded_package}/latest" | jq -er '.version'
      ;;
    pypi)
      curl --fail --silent --show-error --location \
        "https://pypi.org/pypi/${package}/json" | jq -er '.info.version'
      ;;
    github)
      gh api "repos/${package}/releases/latest" --jq '.tag_name | sub("^v"; "")'
      ;;
    *)
      return 2
      ;;
  esac
}

for provider in "${providers[@]}"; do
  if ! jq -e --arg provider "$provider" 'has($provider)' "$state_file" >/dev/null; then
    echo "warning: $provider is missing from provider-versions.json; skipping" >&2
    continue
  fi

  source=$(jq -r --arg provider "$provider" '.[$provider].source' "$state_file")
  package=$(jq -r --arg provider "$provider" '.[$provider].package // empty' "$state_file")
  recorded=$(jq -r --arg provider "$provider" '.[$provider].version' "$state_file")

  if [[ $source == "manual" ]]; then
    echo "$provider: manual source; skipping"
    continue
  fi

  if [[ -z $package ]]; then
    echo "warning: $provider has no package configured; skipping" >&2
    continue
  fi

  if ! latest=$(fetch_latest "$source" "$package"); then
    echo "warning: failed to fetch latest $source version for $provider ($package); skipping" >&2
    continue
  fi

  if [[ $latest == "$recorded" ]]; then
    echo "$provider: unchanged at $recorded"
    continue
  fi

  title="Provider release watch: $provider $latest"
  body=$(cat <<EOF
Provider \`$provider\` moved from \`$recorded\` to \`$latest\`.

Run \`cargo run --example provider_e2e -- --provider $provider\` locally to validate the provider TUI.

When validation is complete, update \`provider-versions.json\` with the validated version.
EOF
)

  if [[ $file_issues != true ]]; then
    echo "$provider: $recorded -> $latest; would file issue: $title"
    continue
  fi

  issue_titles=$(gh issue list --state all --search "$title" --limit 100 --json title)
  if jq -e --arg title "$title" 'any(.[]; .title == $title)' <<<"$issue_titles" >/dev/null; then
    echo "$provider: issue already exists: $title"
    continue
  fi

  gh label create provider-watch \
    --description "Tracks upstream provider releases requiring local e2e validation" \
    --force
  gh issue create --title "$title" --body "$body" --label provider-watch
done
