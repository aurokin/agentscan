#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

agentscan_cmd() {
  if [[ -n "${AGENTSCAN_BIN:-}" ]]; then
    "$AGENTSCAN_BIN" "$@"
    return
  fi

  if [[ -x "$repo_root/target/debug/agentscan" ]]; then
    "$repo_root/target/debug/agentscan" "$@"
    return
  fi

  cargo run --manifest-path "$repo_root/Cargo.toml" -- "$@"
}

status_label() {
  case "${1:-unknown}" in
    busy) printf '%s' "[busy]" ;;
    idle) printf '%s' "[idle]" ;;
    *) printf '%s' "[?]" ;;
  esac
}

keys=(1 2 3 4 5 6 7 8 9 0 q w e r t y u i o p)
declare -A key_targets=()
client_tty=''
popup_args=()

while (($# > 0)); do
  case "${1:-}" in
    -f|--refresh)
      popup_args+=("$1")
      shift
      ;;
    *)
      echo "usage: $(basename "$0") [-f|--refresh]" >&2
      exit 2
      ;;
  esac
done

if command -v tmux >/dev/null 2>&1 && [[ -n "${TMUX:-}" ]]; then
  client_tty="$(tmux display-message -p '#{client_tty}' 2>/dev/null || true)"
fi

mapfile -t rows < <(agentscan_cmd "${popup_args[@]}" tmux popup --format tsv)

if ((${#rows[@]} == 0)); then
  echo "No panes available in cache."
  exit 0
fi

row_count=0
for row in "${rows[@]}"; do
  IFS=$'\t' read -r pane_id provider status session_name window_index pane_index display_label <<< "$row"
  [[ -n "$pane_id" ]] || continue

  key=""
  if ((row_count < ${#keys[@]})); then
    key="${keys[$row_count]}"
    key_targets["$key"]="$pane_id"
    printf '[%s] ' "$key"
  else
    printf '    '
  fi

  provider="${provider:-unknown}"
  printf '%s %s %s:%s.%s - %s\n' \
    "$provider" \
    "$(status_label "$status")" \
    "$session_name" \
    "$window_index" \
    "$pane_index" \
    "$display_label"
  ((row_count+=1))
done

echo
printf '%s' 'Select pane key, or press any other key to close: '

IFS= read -r -s -n 1 key || exit 0

if [[ -n "${key_targets[$key]-}" ]]; then
  if [[ -n "$client_tty" ]]; then
    agentscan_cmd focus --client-tty "$client_tty" "${key_targets[$key]}"
  else
    agentscan_cmd focus "${key_targets[$key]}"
  fi
fi
