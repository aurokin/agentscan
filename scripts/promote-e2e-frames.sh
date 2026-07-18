#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage: scripts/promote-e2e-frames.sh [--force] <run-id> <provider> <frame-file> <state> <cli-version>"
}

force=false
if [[ ${1:-} == "--force" ]]; then
  force=true
  shift
fi

if [[ $# -ne 5 ]]; then
  usage >&2
  exit 2
fi

run_id=$1
provider=$2
frame_file=$3
state=$4
cli_version=$5

case "$state" in
  idle | busy | waiting) ;;
  *)
    echo "error: state must be idle, busy, or waiting: $state" >&2
    exit 2
    ;;
esac

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
source_file="$repo_root/target/provider-e2e/$run_id/$provider/$frame_file"
fixture_dir="$repo_root/tests/fixtures/pane_corpus/$provider/$cli_version"
fixture_file="$fixture_dir/$state.txt"
meta_file="$fixture_dir/$state.meta.toml"

if [[ ! -f "$source_file" ]]; then
  echo "error: frame not found: $source_file" >&2
  exit 2
fi

if [[ $force != true && ( -e "$fixture_file" || -e "$meta_file" ) ]]; then
  echo "error: fixture already exists; pass --force to overwrite: $fixture_file" >&2
  exit 2
fi

mkdir -p "$fixture_dir"
cp "$source_file" "$fixture_file"

cols=$(LC_ALL=C awk 'length > widest { widest = length } END { print widest + 0 }' "$fixture_file")
rows=$(awk 'END { print NR + 0 }' "$fixture_file")
captured=$(date +%F)

cat >"$meta_file" <<EOF
provider = "$provider"
cli_version = "$cli_version"
captured = "$captured"
# Byte-length prefill; the corpus test recomputes display width and may require adjustment.
cols = $cols
rows = $rows
expected_status = "$state"
expected_source = "pane_output"
origin = "$run_id"
# Fill in durable chrome whose removal or blanking must not invert the status.
corroborators = []
allow_other_providers = []
EOF

echo "Created $fixture_file"
echo "Created $meta_file"
echo "Fill in corroborators, adjust cols if needed, then run: cargo test pane_snapshot_corpus"
