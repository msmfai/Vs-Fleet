#!/usr/bin/env bash
set -euo pipefail

root="$(pwd)"
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  root="$(git rev-parse --show-toplevel)"
else
  echo "FAIL: public tree size check must run inside a git worktree"
  exit 2
fi

default_limit=$((1024 * 1024))
icon_limit=$((5 * 1024 * 1024))
fail=0

file_size() {
  if stat -f %z "$1" >/dev/null 2>&1; then
    stat -f %z "$1"
  else
    stat -c %s "$1"
  fi
}

while IFS= read -r file; do
  path="$root/$file"
  [ -f "$path" ] || continue
  size="$(file_size "$path")"

  case "$file" in
    crates/fleet-host/icons/icon.png)
      if [ "$size" -gt "$icon_limit" ]; then
        echo "FAIL: $file is $size bytes; source icon must stay at or below $icon_limit bytes"
        fail=1
      fi
      continue
      ;;
  esac

  if [ "$size" -gt "$default_limit" ]; then
    echo "FAIL: $file is $size bytes; tracked public files must stay at or below $default_limit bytes"
    fail=1
  fi
done < <(git -C "$root" ls-files)

if [ "$fail" -ne 0 ]; then
  echo
  echo "Move large generated artifacts out of git, or add a narrow reviewed exception."
  exit 1
fi

echo "Public tree size check passed."
