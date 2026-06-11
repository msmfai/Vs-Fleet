#!/usr/bin/env bash
set -euo pipefail

owner_record="${1:-docs/release/OWNER_DECISION_RECORD.md}"
contributing_file="${2:-CONTRIBUTING.md}"
pr_template="${3:-.github/PULL_REQUEST_TEMPLATE.md}"

if [ ! -f "$owner_record" ]; then
  echo "FAIL: missing owner decision record: $owner_record"
  exit 1
fi

if [ ! -f "$contributing_file" ]; then
  echo "FAIL: missing contribution guide: $contributing_file"
  exit 1
fi

if [ ! -f "$pr_template" ]; then
  echo "FAIL: missing pull request template: $pr_template"
  exit 1
fi

if ! rg -q '^Decision record status: APPROVED$' "$owner_record"; then
  echo "FAIL: owner decision record is not approved"
  exit 1
fi

ai_block="$(
  sed -n '/^### 17\. AI-Assisted Contribution Provenance$/,/^### 18\. Supported Platform And Toolchain$/p' "$owner_record"
)"

checked_count="$(printf '%s\n' "$ai_block" | rg -c '^- \[x\] ' || true)"
checked_count="${checked_count:-0}"
if [ "$checked_count" -ne 1 ]; then
  echo "FAIL: AI-assisted contribution provenance decision must have exactly one checked choice; found $checked_count"
  exit 1
fi

checked="$(printf '%s\n' "$ai_block" | rg '^- \[x\] ' | head -n1)"

require_text() {
  local file=$1
  local pattern=$2
  local description=$3
  if ! rg -qi "$pattern" "$file"; then
    echo "FAIL: $file must contain $description"
    exit 1
  fi
}

reject_placeholder_text() {
  local failed=0
  for file in "$contributing_file" "$pr_template"; do
    if rg -ni 'TODO|TBD|PLACEHOLDER|AI policy pending|generated contribution policy pending' "$file"; then
      failed=1
    fi
  done
  if [ "$failed" -ne 0 ]; then
    echo "FAIL: AI contribution docs still contain placeholder policy text"
    exit 1
  fi
}

check_allowed_with_certification() {
  reject_placeholder_text
  require_text "$contributing_file" 'AI-assisted|AI generated|generated with AI|model-generated' \
    "AI-assisted contribution policy"
  require_text "$contributing_file" 'reviewed.*understand|understand.*reviewed' \
    "human review/responsibility requirement"
  require_text "$contributing_file" 'right to submit|license.*right|rights to submit' \
    "right-to-submit certification"
  require_text "$contributing_file" 'private prompts|private model transcripts|private logs|workspace paths' \
    "private prompt/log exclusion"
  require_text "$contributing_file" 'generated build outputs|raw logs|machine-specific paths' \
    "generated artifact exclusion"
  require_text "$pr_template" 'AI-assisted|AI generated|generated with AI|model-generated' \
    "AI-assisted contribution checkbox"
  require_text "$pr_template" 'reviewed.*understand|understand.*reviewed' \
    "human review certification checkbox"
  require_text "$pr_template" 'private prompts|private model transcripts|private logs|workspace paths' \
    "private prompt/log certification"
}

check_approval_required() {
  reject_placeholder_text
  require_text "$contributing_file" 'AI-assisted|AI generated|generated with AI|model-generated' \
    "AI-assisted contribution policy"
  require_text "$contributing_file" 'maintainer approval|maintainer-requested|explicit maintainer' \
    "maintainer approval requirement"
  require_text "$pr_template" 'maintainer approval|maintainer-requested|explicit maintainer' \
    "maintainer approval PR certification"
}

case "$checked" in
  "- [x] Allow AI-assisted contributions if the contributor certifies"*)
    check_allowed_with_certification
    ;;
  "- [x] Require maintainer approval before accepting AI-generated code"*)
    check_approval_required
    ;;
  "- [x] Other: "*)
    other_value="$(printf '%s\n' "$checked" | sed -n 's/^- \[x\] Other: `\(.*\)`$/\1/p')"
    if [ -z "$other_value" ] || [ "$other_value" = "TODO" ]; then
      echo "FAIL: checked Other AI contribution decision must contain a concrete value"
      exit 1
    fi
    reject_placeholder_text
    require_text "$contributing_file" '^AI contribution policy:' \
      "a concrete AI contribution policy line"
    require_text "$pr_template" '^AI contribution policy:' \
      "a concrete AI contribution policy line"
    ;;
  *)
    echo "FAIL: unsupported AI-assisted contribution provenance decision: $checked"
    exit 1
    ;;
esac

echo "AI contribution decision check passed."
