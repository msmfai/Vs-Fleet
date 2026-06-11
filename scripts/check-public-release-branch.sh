#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
usage: scripts/check-public-release-branch.sh <public-branch> <source-ref> [owner-record]

Verify a prepared clean public branch before first GitHub visibility.
This does not create, switch, rewrite, or push branches.
EOF
}

public_branch="${1:-}"
source_ref="${2:-}"
owner_record="${3:-docs/release/OWNER_DECISION_RECORD.md}"

if [ -z "$public_branch" ] || [ -z "$source_ref" ] ||
  [ "$public_branch" = "-h" ] || [ "$public_branch" = "--help" ]; then
  usage
  exit 2
fi

root="$(git rev-parse --show-toplevel 2>/dev/null || true)"
if [ -z "$root" ]; then
  echo "FAIL: public release branch check must run inside a git worktree" >&2
  exit 2
fi

cd "$root"

source_commit="$(git -C "$root" rev-parse --verify "$source_ref^{commit}")"
git -C "$root" rev-parse --verify "$public_branch^{commit}" >/dev/null

fail=0

run_gate() {
  local label=$1
  shift

  echo "==> $label"
  if "$@"; then
    return 0
  fi
  fail=1
}

run_gate "history release check" \
  "$root/scripts/history-release-check.sh" "$owner_record" "$public_branch"

public_commit="$(git -C "$root" rev-parse --verify "$public_branch^{commit}")"
source_tree="$(git -C "$root" rev-parse --verify "$source_commit^{tree}")"
public_tree="$(git -C "$root" rev-parse --verify "$public_commit^{tree}")"
echo "==> public branch tree check"
if [ "$source_tree" != "$public_tree" ]; then
  echo "FAIL: public branch tree does not match source ref tree"
  fail=1
else
  echo "Public branch tree matches source ref."
fi

run_gate "secret release check" \
  "$root/scripts/secret-release-check.sh" "$public_branch"

echo "==> aggregate release check"
if ! FLEET_RELEASE_HISTORY_REF="$public_branch" "$root/scripts/release-check.sh"; then
  fail=1
fi

if [ "$fail" -ne 0 ]; then
  echo "Public release branch check failed."
  exit 1
fi

echo "Public release branch check passed."
