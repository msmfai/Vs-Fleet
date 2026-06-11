#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
usage: scripts/check-dco-signoff.sh [rev-range]

Check that every non-merge commit in rev-range has a Signed-off-by line.
When no rev-range is supplied, pull_request CI derives origin/$GITHUB_BASE_REF..HEAD.
Maintainer push/local runs without a rev-range are skipped.
EOF
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  exit 2
fi

rev_range="${1:-}"

if [ -z "$rev_range" ]; then
  if [ "${GITHUB_EVENT_NAME:-}" = "pull_request" ] && [ -n "${GITHUB_BASE_REF:-}" ]; then
    rev_range="origin/${GITHUB_BASE_REF}..HEAD"
  else
    echo "DCO sign-off check skipped: no pull request rev-range."
    exit 0
  fi
fi

if ! commits="$(git rev-list --no-merges "$rev_range" 2>/dev/null)"; then
  echo "FAIL: invalid DCO rev-range: $rev_range"
  exit 1
fi
if [ -z "$commits" ]; then
  echo "DCO sign-off check passed: no non-merge commits."
  exit 0
fi

fail=0
while IFS= read -r commit; do
  [ -n "$commit" ] || continue
  if ! git log -1 --format=%B "$commit" |
    rg -q '^Signed-off-by: [^<>]+ <[^<>[:space:]]+@[^<>[:space:]]+>$'; then
    subject="$(git log -1 --format=%s "$commit")"
    echo "FAIL: missing DCO Signed-off-by on $commit $subject"
    fail=1
  fi
done <<EOF
$commits
EOF

if [ "$fail" -ne 0 ]; then
  echo "Add a Signed-off-by line with 'git commit -s' or amend the commit."
  exit 1
fi

echo "DCO sign-off check passed."
