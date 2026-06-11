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

roadmap_block="$(
  sed -n '/^### 19\. Public Roadmap And Non-Goals$/,/^### 20\. Public Name Collision And Trademark Posture$/p' "$owner_record"
)"

checked_count="$(printf '%s\n' "$roadmap_block" | rg -c '^- \[x\] ' || true)"
checked_count="${checked_count:-0}"
if [ "$checked_count" -ne 1 ]; then
  echo "FAIL: public roadmap and non-goals decision must have exactly one checked choice; found $checked_count"
  exit 1
fi

checked="$(printf '%s\n' "$roadmap_block" | rg '^- \[x\] ' | head -n1)"

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
    echo "FAIL: $file still contains placeholder roadmap text"
    exit 1
  fi
}

check_no_public_roadmap_commitment() {
  require_text "README.md" 'No public roadmap commitments are made during alpha' \
    "no public roadmap commitment"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" '^## Roadmap And Non-Goals$' \
    "Roadmap And Non-Goals release-notes section"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" 'No public roadmap commitments are made during alpha' \
    "release-notes no-roadmap promise"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" 'Issues, labels, and milestones are triage hints, not delivery promises' \
    "release-notes issue/milestone non-commitment"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" 'Remote/container workflows, binary packages, stable APIs, and production' \
    "release-notes alpha non-goals subject"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" 'support remain non-goals unless a later owner decision approves them' \
    "release-notes alpha non-goals boundary"
  require_text ".github/ISSUE_TEMPLATE/alpha_feedback.yml" 'suggestions are not roadmap commitments' \
    "alpha feedback non-roadmap warning"
  require_text "docs/release/PUBLIC_ALPHA_DECISIONS.md" 'roadmap commitments' \
    "public decision table roadmap warning"
}

case "$checked" in
  "- [x] No public roadmap commitments during alpha."*)
    check_no_public_roadmap_commitment
    ;;
  "- [x] Publish a public roadmap before alpha."*)
    reject_placeholder_file "docs/release/ROADMAP.md"
    require_text "docs/release/ROADMAP.md" '^Roadmap commitment:' \
      "a concrete Roadmap commitment line"
    require_text "docs/release/ROADMAP.md" '^Non-goals:' \
      "a concrete Non-goals line"
    require_text "docs/release/ROADMAP.md" '^Change process:' \
      "a concrete Change process line"
    ;;
  "- [x] Other: "*)
    other_value="$(printf '%s\n' "$checked" | sed -n 's/^- \[x\] Other: `\(.*\)`$/\1/p')"
    if [ -z "$other_value" ] || [ "$other_value" = "TODO" ]; then
      echo "FAIL: checked Other roadmap decision must contain a concrete value"
      exit 1
    fi
    reject_placeholder_file "docs/release/ROADMAP.md"
    require_text "docs/release/ROADMAP.md" '^Roadmap commitment:' \
      "a concrete Roadmap commitment line"
    ;;
  *)
    echo "FAIL: unsupported public roadmap and non-goals decision: $checked"
    exit 1
    ;;
esac

echo "Roadmap decision check passed."
