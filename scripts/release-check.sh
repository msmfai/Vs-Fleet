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
  . \
  ':(exclude)scripts/history-release-check.sh'

check_tracked_absent 'not ready for (a )?(general )?public alpha yet|still blocked for public open-source release|No open-source license has been chosen yet|UNLICENSED' \
  "public release-facing docs still describe unresolved alpha blockers" \
  README.md docs/QUICKSTART.md docs/release/ALPHA_RELEASE_CHECKLIST.md docs/release/PUBLIC_ALPHA_DECISIONS.md

if git ls-files | rg '(^|/)coverage/|(^|/)node_modules/|(^|/)out/|\.vsix$|Fleet\.app/' >/tmp/fleet-release-check.$$; then
  echo "FAIL: generated dependency/build outputs are tracked"
  sed -n '1,80p' /tmp/fleet-release-check.$$
  fail=1
fi
rm -f /tmp/fleet-release-check.$$

for manifest in \
  crates/fleet-cli/Cargo.toml \
  crates/fleet-e2e/Cargo.toml \
  crates/fleet-host-core/Cargo.toml \
  crates/fleet-host/Cargo.toml \
  crates/fleet-hub/Cargo.toml \
  crates/fleet-protocol/Cargo.toml \
  crates/fleet-reporter/Cargo.toml
do
  if ! rg -q '^publish[[:space:]]*=[[:space:]]*false$' "$manifest"; then
    echo "FAIL: $manifest must set publish = false for source-only alpha"
    fail=1
  fi
done

for manifest in \
  packages/fleet-bridge/package.json \
  packages/extension/package.json
do
  if ! rg -q '"private"[[:space:]]*:[[:space:]]*true' "$manifest"; then
    echo "FAIL: $manifest must set \"private\": true for source-only alpha"
    fail=1
  fi
done

for required in \
  SECURITY.md \
  CONTRIBUTING.md \
  SUPPORT.md \
  CODE_OF_CONDUCT.md \
  docs/QUICKSTART.md \
  docs/ARCHITECTURE.md \
  docs/release/RELEASE_PROCESS.md \
  docs/release/DEPENDENCY_REVIEW.md \
  docs/release/DEPENDENCY_REVIEW_EVIDENCE.md \
  docs/release/PUBLIC_CI_EVIDENCE.md \
  docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md \
  scripts/check-license-decision.sh \
  scripts/test-license-decision-check.sh \
  scripts/check-namespace-decision.sh \
  scripts/test-namespace-decision-check.sh \
  scripts/check-security-reporting-decision.sh \
  scripts/test-security-reporting-decision-check.sh \
  scripts/check-contribution-decision.sh \
  scripts/test-contribution-decision-check.sh \
  scripts/check-ci-evidence-decision.sh \
  scripts/test-ci-evidence-decision-check.sh \
  scripts/check-dependency-review-decision.sh \
  scripts/test-dependency-review-decision-check.sh \
  scripts/check-release-notes.sh \
  scripts/test-release-notes-check.sh \
  .github/workflows/release-readiness.yml \
  .github/PULL_REQUEST_TEMPLATE.md \
  .github/ISSUE_TEMPLATE/bug_report.yml \
  .github/ISSUE_TEMPLATE/alpha_feedback.yml
do
  if [ ! -f "$required" ]; then
    echo "FAIL: missing $required"
    fail=1
  fi
done

if [ ! -f docs/release/OWNER_DECISION_RECORD.md ]; then
  echo "FAIL: missing docs/release/OWNER_DECISION_RECORD.md"
  fail=1
elif ! scripts/check-owner-decisions.sh docs/release/OWNER_DECISION_RECORD.md; then
  fail=1
elif ! scripts/check-license-decision.sh docs/release/OWNER_DECISION_RECORD.md .; then
  fail=1
elif ! scripts/check-namespace-decision.sh docs/release/OWNER_DECISION_RECORD.md .; then
  fail=1
elif ! scripts/check-security-reporting-decision.sh docs/release/OWNER_DECISION_RECORD.md SECURITY.md; then
  fail=1
elif ! scripts/check-contribution-decision.sh docs/release/OWNER_DECISION_RECORD.md CONTRIBUTING.md .github/PULL_REQUEST_TEMPLATE.md; then
  fail=1
elif ! scripts/check-ci-evidence-decision.sh docs/release/OWNER_DECISION_RECORD.md docs/release/PUBLIC_CI_EVIDENCE.md "$(git rev-parse HEAD)"; then
  fail=1
elif ! scripts/check-dependency-review-decision.sh docs/release/OWNER_DECISION_RECORD.md docs/release/DEPENDENCY_REVIEW_EVIDENCE.md "$(git rev-parse HEAD)"; then
  fail=1
fi

if [ "$fail" -ne 0 ]; then
  echo
  echo "Release check failed. See docs/release/PUBLIC_ALPHA_DECISIONS.md."
  exit 1
fi

echo "Release check passed."
