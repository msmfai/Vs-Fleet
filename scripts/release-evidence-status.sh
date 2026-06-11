#!/usr/bin/env bash
set -euo pipefail

root="$(git rev-parse --show-toplevel 2>/dev/null || true)"
if [ -z "$root" ]; then
  echo "FAIL: release evidence status must run inside a git worktree" >&2
  exit 2
fi

cd "$root"

fail=0

check_file() {
  local file=$1
  local label=$2
  local status_label=$3

  if [ ! -f "$file" ]; then
    echo "$label: MISSING ($file)"
    fail=1
    return
  fi

  local status
  status="$(rg -i "^${status_label}:" "$file" | head -n1 | sed 's/^[^:]*:[[:space:]]*//; s/[[:space:]]*$//; s/^`//; s/`$//' || true)"
  if [ -z "$status" ]; then
    echo "$label: MISSING STATUS ($file)"
    fail=1
    return
  fi

  local placeholders
  placeholders="$(rg -n 'TODO|TBD|PLACEHOLDER|PENDING|not yet run|not yet reviewed|not yet configured' "$file" || true)"
  if [ -n "$placeholders" ]; then
    echo "$label: $status, placeholders remain ($file)"
    printf '%s\n' "$placeholders" | sed -n '1,8p'
    fail=1
    return
  fi

  echo "$label: $status ($file)"
}

check_file "docs/release/PUBLIC_BRANCH_EVIDENCE.md" \
  "Public branch evidence" \
  "Public branch evidence status"
check_file "docs/release/PUBLIC_CI_EVIDENCE.md" \
  "Public CI evidence" \
  "Public CI evidence status"
check_file "docs/release/GITHUB_PUBLICATION_EVIDENCE.md" \
  "GitHub publication evidence" \
  "GitHub publication evidence status"
check_file "docs/release/DEPENDENCY_REVIEW_EVIDENCE.md" \
  "Dependency review evidence" \
  "Dependency review status"

if [ "$fail" -ne 0 ]; then
  echo "Release evidence status: BLOCKED"
  exit 1
fi

echo "Release evidence status: COMPLETE"
