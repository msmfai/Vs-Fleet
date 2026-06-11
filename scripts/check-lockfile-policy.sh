#!/usr/bin/env bash
set -euo pipefail

root="$(pwd)"
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  root="$(git rev-parse --show-toplevel)"
else
  echo "FAIL: lockfile policy check must run inside a git worktree"
  exit 2
fi

fail=0

require_tracked() {
  local file=$1
  if [ ! -f "$root/$file" ]; then
    echo "FAIL: missing required lockfile: $file"
    fail=1
    return
  fi
  if ! git -C "$root" ls-files --error-unmatch "$file" >/dev/null 2>&1; then
    echo "FAIL: required lockfile is not tracked: $file"
    fail=1
  fi
}

require_not_ignored() {
  local file=$1
  local ignored
  ignored="$(git -C "$root" check-ignore -v --no-index "$file" 2>/dev/null || true)"
  if [ -n "$ignored" ]; then
    echo "FAIL: required lockfile is ignored by gitignore rules: $file"
    echo "$ignored"
    fail=1
  fi
}

for file in \
  Cargo.lock \
  crates/fleet-host/Cargo.lock \
  pnpm-lock.yaml \
  packages/fleet-bridge/package-lock.json \
  packages/extension/package-lock.json
do
  require_tracked "$file"
  require_not_ignored "$file"
done

if [ "$fail" -ne 0 ]; then
  exit 1
fi

echo "Lockfile policy check passed."
