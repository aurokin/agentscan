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

  selection="$((row_count + 1))"
  key_targets["$selection"]="$pane_id"
  printf '[%s] ' "$selection"

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
printf '%s' 'Select pane number, or press Enter to close: '

IFS= read -r selection || exit 0
selection="${selection#"${selection%%[![:space:]]*}"}"
selection="${selection%"${selection##*[![:space:]]}"}"

if [[ -n "${key_targets[$selection]-}" ]]; then
  if [[ -n "$client_tty" ]]; then
    agentscan_cmd focus --client-tty "$client_tty" "${key_targets[$selection]}"
  else
    agentscan_cmd focus "${key_targets[$selection]}"
  fi
fi
