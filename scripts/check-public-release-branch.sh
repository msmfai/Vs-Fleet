#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
usage: scripts/check-public-release-branch.sh <public-branch> <source-ref> [owner-record] [evidence-file]

Verify a prepared clean public branch before first GitHub visibility.
This does not create, switch, rewrite, or push branches.
EOF
}

public_branch="${1:-}"
source_ref="${2:-}"
owner_record="${3:-docs/release/OWNER_DECISION_RECORD.md}"
evidence_file="${4:-docs/release/PUBLIC_BRANCH_EVIDENCE.md}"

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

run_gate "public branch evidence check" \
  "$root/scripts/check-public-branch-evidence.sh" \
  "$owner_record" "$evidence_file" "$source_commit"

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
