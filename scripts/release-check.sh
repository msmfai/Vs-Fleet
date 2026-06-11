#!/usr/bin/env bash
set -euo pipefail

fail=0

check_tracked_absent() {
  local pattern=$1
  local description=$2
  shift 2
  if git grep -n -E "$pattern" -- "$@" ':(exclude)scripts/release-check.sh' >/tmp/fleet-release-check.$$ 2>/dev/null; then
    echo "FAIL: $description"
    sed -n '1,40p' /tmp/fleet-release-check.$$
    fail=1
  fi
  rm -f /tmp/fleet-release-check.$$
}

if [ ! -f LICENSE ]; then
  echo "FAIL: missing root LICENSE"
  fail=1
fi

check_tracked_absent 'license[[:space:]]*=[[:space:]]*"UNLICENSED"|"license"[[:space:]]*:[[:space:]]*"UNLICENSED"' \
  "package manifests still declare UNLICENSED" \
  Cargo.toml crates packages

local_path_pattern='/private/tmp/|/private/var/folders/[[:alnum:]]{2}/|/var/folders/[[:alnum:]]{2}/|C:\\Users\\[^[:space:]"/]+'
if [ -n "${USER:-}" ]; then
  local_path_pattern="/Users/${USER}(/|$)|${local_path_pattern}"
fi

check_tracked_absent "$local_path_pattern" \
  "tracked release-facing text artifacts contain local absolute paths" \
  .

if git ls-files | rg '(^|/)coverage/|(^|/)node_modules/|(^|/)out/|\.vsix$|Fleet\.app/' >/tmp/fleet-release-check.$$; then
  echo "FAIL: generated dependency/build outputs are tracked"
  sed -n '1,80p' /tmp/fleet-release-check.$$
  fail=1
fi
rm -f /tmp/fleet-release-check.$$

for required in \
  SECURITY.md \
  CONTRIBUTING.md \
  SUPPORT.md \
  CODE_OF_CONDUCT.md \
  .github/PULL_REQUEST_TEMPLATE.md \
  .github/ISSUE_TEMPLATE/bug_report.yml \
  .github/ISSUE_TEMPLATE/alpha_feedback.yml
do
  if [ ! -f "$required" ]; then
    echo "FAIL: missing $required"
    fail=1
  fi
done

if [ "$fail" -ne 0 ]; then
  echo
  echo "Release check failed. See docs/release/PUBLIC_ALPHA_DECISIONS.md."
  exit 1
fi

echo "Release check passed."
