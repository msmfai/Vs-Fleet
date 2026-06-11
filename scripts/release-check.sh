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
  README.md docs/QUICKSTART.md docs/release/ALPHA_RELEASE_CHECKLIST.md docs/release/PUBLIC_ALPHA_DECISIONS.md docs/release/GITHUB_PUBLICATION_RUNBOOK.md

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
  docs/release/GITHUB_PUBLICATION_RUNBOOK.md \
  docs/release/PUBLIC_CI_EVIDENCE.md \
  docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md \
  .github/workflows/ci.yml \
  .github/workflows/release-readiness.yml \
  .github/dependabot.yml \
  scripts/check-github-workflows.sh \
  scripts/test-github-workflows-check.sh \
  scripts/check-github-intake-templates.sh \
  scripts/test-github-intake-templates-check.sh \
  scripts/check-doc-links.sh \
  scripts/test-doc-link-check.sh \
  scripts/check-dependabot-config.sh \
  scripts/test-dependabot-config-check.sh \
  scripts/check-owner-decisions.sh \
  scripts/test-owner-decision-gate.sh \
  scripts/history-release-check.sh \
  scripts/test-history-release-check.sh \
  scripts/secret-release-check.sh \
  scripts/test-secret-release-check.sh \
  scripts/check-license-decision.sh \
  scripts/test-license-decision-check.sh \
  scripts/check-namespace-decision.sh \
  scripts/test-namespace-decision-check.sh \
  scripts/check-alpha-scope-decision.sh \
  scripts/test-alpha-scope-decision-check.sh \
  scripts/check-editor-server-boundary-decision.sh \
  scripts/test-editor-server-boundary-decision-check.sh \
  scripts/check-distribution-decision.sh \
  scripts/test-distribution-decision-check.sh \
  scripts/check-security-reporting-decision.sh \
  scripts/test-security-reporting-decision-check.sh \
  scripts/check-contribution-decision.sh \
  scripts/test-contribution-decision-check.sh \
  scripts/check-ci-evidence-decision.sh \
  scripts/test-ci-evidence-decision-check.sh \
  scripts/check-privacy-decision.sh \
  scripts/test-privacy-decision-check.sh \
  scripts/check-dependency-review-decision.sh \
  scripts/test-dependency-review-decision-check.sh \
  scripts/check-support-decision.sh \
  scripts/test-support-decision-check.sh \
  scripts/test-release-check.sh \
  scripts/check-release-notes.sh \
  scripts/test-release-notes-check.sh \
  .github/PULL_REQUEST_TEMPLATE.md \
  .github/ISSUE_TEMPLATE/config.yml \
  .github/ISSUE_TEMPLATE/bug_report.yml \
  .github/ISSUE_TEMPLATE/alpha_feedback.yml
do
  if [ ! -f "$required" ]; then
    echo "FAIL: missing $required"
    fail=1
  fi
done

if ! scripts/check-dependabot-config.sh .github/dependabot.yml; then
  fail=1
fi

if ! scripts/check-github-workflows.sh .github/workflows/ci.yml .github/workflows/release-readiness.yml; then
  fail=1
fi

if ! scripts/check-github-intake-templates.sh \
  .github/ISSUE_TEMPLATE/bug_report.yml \
  .github/ISSUE_TEMPLATE/alpha_feedback.yml \
  .github/ISSUE_TEMPLATE/config.yml \
  .github/PULL_REQUEST_TEMPLATE.md; then
  fail=1
fi

if ! scripts/check-doc-links.sh; then
  fail=1
fi

if ! scripts/secret-release-check.sh; then
  fail=1
fi

if [ ! -f docs/release/OWNER_DECISION_RECORD.md ]; then
  echo "FAIL: missing docs/release/OWNER_DECISION_RECORD.md"
  fail=1
elif ! scripts/check-owner-decisions.sh docs/release/OWNER_DECISION_RECORD.md; then
  fail=1
else
  if ! scripts/history-release-check.sh docs/release/OWNER_DECISION_RECORD.md; then
    fail=1
  fi
  if ! scripts/check-license-decision.sh docs/release/OWNER_DECISION_RECORD.md .; then
    fail=1
  fi
  if ! scripts/check-namespace-decision.sh docs/release/OWNER_DECISION_RECORD.md .; then
    fail=1
  fi
  if ! scripts/check-alpha-scope-decision.sh docs/release/OWNER_DECISION_RECORD.md .; then
    fail=1
  fi
  if ! scripts/check-editor-server-boundary-decision.sh docs/release/OWNER_DECISION_RECORD.md .; then
    fail=1
  fi
  if ! scripts/check-distribution-decision.sh docs/release/OWNER_DECISION_RECORD.md .; then
    fail=1
  fi
  if ! scripts/check-security-reporting-decision.sh docs/release/OWNER_DECISION_RECORD.md SECURITY.md; then
    fail=1
  fi
  if ! scripts/check-contribution-decision.sh docs/release/OWNER_DECISION_RECORD.md CONTRIBUTING.md .github/PULL_REQUEST_TEMPLATE.md; then
    fail=1
  fi
  if ! scripts/check-ci-evidence-decision.sh docs/release/OWNER_DECISION_RECORD.md docs/release/PUBLIC_CI_EVIDENCE.md "$(git rev-parse HEAD)"; then
    fail=1
  fi
  if ! scripts/check-privacy-decision.sh docs/release/OWNER_DECISION_RECORD.md .; then
    fail=1
  fi
  if ! scripts/check-dependency-review-decision.sh docs/release/OWNER_DECISION_RECORD.md docs/release/DEPENDENCY_REVIEW_EVIDENCE.md "$(git rev-parse HEAD)"; then
    fail=1
  fi
  if ! scripts/check-support-decision.sh docs/release/OWNER_DECISION_RECORD.md SUPPORT.md .; then
    fail=1
  fi
fi

if [ "$fail" -ne 0 ]; then
  echo
  echo "Release check failed. See docs/release/PUBLIC_ALPHA_DECISIONS.md."
  exit 1
fi

echo "Release check passed."
