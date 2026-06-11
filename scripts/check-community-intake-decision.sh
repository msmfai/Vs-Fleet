#!/usr/bin/env bash
set -euo pipefail

owner_record="${1:-docs/release/OWNER_DECISION_RECORD.md}"
root="${2:-.}"

if [ ! -f "$owner_record" ]; then
  echo "FAIL: missing owner decision record: $owner_record"
  exit 1
fi

if [ ! -d "$root" ]; then
  echo "FAIL: missing repository root: $root"
  exit 1
fi

if ! rg -q '^Decision record status: APPROVED$' "$owner_record"; then
  echo "FAIL: owner decision record is not approved"
  exit 1
fi

community_block="$(
  sed -n '/^### 15\. Community Intake And Moderation$/,/^## Required Before Binary Distribution$/p' "$owner_record"
)"

checked_count="$(printf '%s\n' "$community_block" | rg -c '^- \[x\] ' || true)"
checked_count="${checked_count:-0}"
if [ "$checked_count" -ne 1 ]; then
  echo "FAIL: community intake and moderation decision must have exactly one checked choice; found $checked_count"
  exit 1
fi

checked="$(printf '%s\n' "$community_block" | rg '^- \[x\] ' | head -n1)"

require_file() {
  local file=$1
  if [ ! -f "$root/$file" ]; then
    echo "FAIL: missing $file"
    exit 1
  fi
}

require_text() {
  local file=$1
  local pattern=$2
  local description=$3
  require_file "$file"
  if ! rg -qi "$pattern" "$root/$file"; then
    echo "FAIL: $file must contain $description"
    exit 1
  fi
}

reject_placeholder_file() {
  local file=$1
  require_file "$file"
  if rg -ni 'TODO|TBD|PLACEHOLDER' "$root/$file"; then
    echo "FAIL: $file still contains placeholder community intake text"
    exit 1
  fi
}

check_limited_public_intake() {
  require_text ".github/ISSUE_TEMPLATE/config.yml" '^blank_issues_enabled:[[:space:]]*false$' \
    "blank issues disabled for alpha intake"
  require_text ".github/ISSUE_TEMPLATE/bug_report.yml" 'Report a reproducible problem in the supported local alpha path' \
    "narrow bug-report scope"
  require_text ".github/ISSUE_TEMPLATE/bug_report.yml" 'local macOS Fleet host, local code serve-web sessions, reporter, bridge, or CLI' \
    "supported local alpha scope"
  require_text ".github/ISSUE_TEMPLATE/bug_report.yml" 'Do not report vulnerabilities or exploit details in public issues; use SECURITY\.md' \
    "public vulnerability-reporting warning"
  require_text ".github/ISSUE_TEMPLATE/alpha_feedback.yml" 'product/readiness feedback rather than a specific reproducible bug' \
    "alpha feedback scope"
  require_text ".github/ISSUE_TEMPLATE/alpha_feedback.yml" 'Security/privacy expectations' \
    "security/privacy feedback topic"
  require_text "CODE_OF_CONDUCT.md" 'public discussion still needs clear expectations' \
    "public discussion expectations"
  require_text "CODE_OF_CONDUCT.md" 'Do not post private data, credentials, unredacted logs, local paths' \
    "privacy and credential posting boundary"
  require_text "CODE_OF_CONDUCT.md" 'Maintainers may edit, hide, lock, or remove issues, pull requests, comments, or accounts' \
    "moderation enforcement powers"
  require_text "docs/release/GITHUB_PUBLICATION_RUNBOOK.md" 'Discussions' \
    "repository discussion setting review"
}

case "$checked" in
  "- [x] Open public issues only for scoped bug reports and alpha feedback;"*)
    check_limited_public_intake
    ;;
  "- [x] Keep public issues and discussions closed during alpha;"*)
    reject_placeholder_file "docs/release/COMMUNITY_INTAKE.md"
    require_text "docs/release/COMMUNITY_INTAKE.md" '^Community intake commitment:' \
      "a concrete Community intake commitment line"
    require_text "docs/release/COMMUNITY_INTAKE.md" '^Feedback channel:' \
      "a concrete Feedback channel line"
    require_text "docs/release/COMMUNITY_INTAKE.md" '^Moderation policy:' \
      "a concrete Moderation policy line"
    ;;
  "- [x] Other: "*)
    other_value="$(printf '%s\n' "$checked" | sed -n 's/^- \[x\] Other: `\(.*\)`$/\1/p')"
    if [ -z "$other_value" ] || [ "$other_value" = "TODO" ]; then
      echo "FAIL: checked Other community intake decision must contain a concrete value"
      exit 1
    fi
    reject_placeholder_file "docs/release/COMMUNITY_INTAKE.md"
    require_text "docs/release/COMMUNITY_INTAKE.md" '^Community intake commitment:' \
      "a concrete Community intake commitment line"
    ;;
  *)
    echo "FAIL: unsupported community intake and moderation decision: $checked"
    exit 1
    ;;
esac

echo "Community intake decision check passed."
