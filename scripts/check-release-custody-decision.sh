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

custody_block="$(
  sed -n '/^### 16\. Release Custody And Maintainer Authority$/,/^### 17\. AI-Assisted Contribution Provenance$/p' "$owner_record"
)"

checked_count="$(printf '%s\n' "$custody_block" | rg -c '^- \[x\] ' || true)"
checked_count="${checked_count:-0}"
if [ "$checked_count" -ne 1 ]; then
  echo "FAIL: release custody and maintainer authority decision must have exactly one checked choice; found $checked_count"
  exit 1
fi

checked="$(printf '%s\n' "$custody_block" | rg '^- \[x\] ' | head -n1)"

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
    echo "FAIL: $file still contains placeholder release custody text"
    exit 1
  fi
}

check_single_maintainer_alpha() {
  require_text "docs/release/GITHUB_PUBLICATION_RUNBOOK.md" \
    'Release Custody' \
    "release custody review steps"
  require_text "docs/release/GITHUB_PUBLICATION_RUNBOOK.md" \
    'Only the approved release authority may push source tags or create GitHub releases' \
    "single release-authority boundary"
}

case "$checked" in
  "- [x] Single-maintainer alpha. Only the repository owner or named"*)
    check_single_maintainer_alpha
    ;;
  "- [x] Multi-maintainer governance before public alpha."*)
    reject_placeholder_file "docs/release/MAINTAINERS.md"
    require_text "docs/release/MAINTAINERS.md" '^Repository admins:' \
      "repository admins"
    require_text "docs/release/MAINTAINERS.md" '^Release approvers:' \
      "release approvers"
    require_text "docs/release/MAINTAINERS.md" '^Package publishers:' \
      "package publishers"
    require_text "docs/release/MAINTAINERS.md" '^Emergency removal owner:' \
      "emergency removal owner"
    ;;
  "- [x] Other: "*)
    other_value="$(printf '%s\n' "$checked" | sed -n 's/^- \[x\] Other: `\(.*\)`$/\1/p')"
    if [ -z "$other_value" ] || [ "$other_value" = "TODO" ]; then
      echo "FAIL: checked Other release custody decision must contain a concrete value"
      exit 1
    fi
    reject_placeholder_file "docs/release/MAINTAINERS.md"
    require_text "docs/release/MAINTAINERS.md" '^Release custody commitment:' \
      "a concrete Release custody commitment line"
    ;;
  *)
    echo "FAIL: unsupported release custody and maintainer authority decision: $checked"
    exit 1
    ;;
esac

echo "Release custody decision check passed."
