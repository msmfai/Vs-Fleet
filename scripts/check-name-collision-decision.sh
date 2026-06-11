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

name_block="$(
  sed -n '/^### 20\. Public Name Collision And Trademark Posture$/,/^## Required Before Binary Distribution$/p' "$owner_record"
)"

checked_count="$(printf '%s\n' "$name_block" | rg -c '^- \[x\] ' || true)"
checked_count="${checked_count:-0}"
if [ "$checked_count" -ne 1 ]; then
  echo "FAIL: public name collision and trademark decision must have exactly one checked choice; found $checked_count"
  exit 1
fi

checked="$(printf '%s\n' "$name_block" | rg '^- \[x\] ' | head -n1)"

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
  if rg -ni 'TODO|TBD|PLACEHOLDER|pending owner decision|pending clearance' "$root/$file"; then
    echo "FAIL: $file still contains placeholder name-collision text"
    exit 1
  fi
}

namespace_value() {
  local surface=$1
  sed -n '/^### 3\. Public Namespace$/,/^### 4\. Alpha Scope$/p' "$owner_record" |
    awk -F'|' -v surface="$surface" '
      $2 {
        gsub(/^[[:space:]]+|[[:space:]]+$/, "", $2)
        if ($2 == surface) {
          value=$3
          gsub(/^[[:space:]]+|[[:space:]]+$/, "", value)
          print value
          exit
        }
      }
    ' |
    sed 's/^`//; s/`$//'
}

check_provisional_name() {
  require_text "README.md" 'Fleet.*provisional source-alpha working name' \
    "provisional Fleet name wording"
  require_text "README.md" 'makes no trademark claim' \
    "no trademark claim wording"
  require_text "README.md" 'stable package or binary publication under Fleet namespaces is deferred' \
    "deferred stable namespace publication"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" '^## Naming And Trademark Posture$' \
    "Naming And Trademark Posture release-notes section"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" 'Fleet.*provisional source-alpha working name' \
    "release-notes provisional Fleet name wording"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" 'makes no trademark claim to the `?Fleet`? name' \
    "release-notes no trademark claim wording"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" 'Stable package or binary publication under Fleet namespaces is deferred' \
    "release-notes deferred stable namespace publication"
  require_text "docs/release/PUBLIC_ALPHA_DECISIONS.md" 'name collision and trademark posture' \
    "public decision table name-collision row"
  require_text "docs/release/GITHUB_PUBLICATION_RUNBOOK.md" 'Public name collision/trademark posture has been recorded' \
    "publication runbook name-collision checkpoint"
  require_text "docs/release/NAME_COLLISION_REVIEW.md" '^Status: no trademark clearance claim\.$' \
    "no-clearance-claim status"
  require_text "docs/release/NAME_COLLISION_REVIEW.md" 'Known collision: JetBrains has used `?Fleet`? for a developer IDE/product' \
    "known JetBrains Fleet collision"
  require_text "docs/release/NAME_COLLISION_REVIEW.md" 'Stable package or binary publication under Fleet namespaces is deferred' \
    "review deferred stable namespace publication"
}

check_renamed_before_public() {
  local product_name
  product_name="$(namespace_value "Product name")"
  if [ -z "$product_name" ] || [ "$product_name" = "TODO" ] || [ "$product_name" = "Fleet" ] || \
    printf '%s\n' "$product_name" | rg -q ' or TODO$'; then
    echo "FAIL: rename decision requires Public Namespace Product name to be a concrete non-Fleet value"
    exit 1
  fi
  reject_placeholder_file "docs/release/NAME_COLLISION_REVIEW.md"
  require_text "docs/release/NAME_COLLISION_REVIEW.md" '^Selected public name:' \
    "a concrete selected public name line"
  require_text "docs/release/NAME_COLLISION_REVIEW.md" '^Rename scope:' \
    "a concrete rename scope line"
}

check_owner_clearance() {
  reject_placeholder_file "docs/release/NAME_COLLISION_REVIEW.md"
  require_text "docs/release/NAME_COLLISION_REVIEW.md" '^Selected public name: Fleet$' \
    "selected Fleet public name"
  require_text "docs/release/NAME_COLLISION_REVIEW.md" '^Clearance review date:' \
    "a concrete clearance review date"
  require_text "docs/release/NAME_COLLISION_REVIEW.md" '^Reviewed collision:' \
    "a concrete reviewed collision line"
  require_text "docs/release/NAME_COLLISION_REVIEW.md" '^Owner decision:' \
    "a concrete owner decision line"
}

case "$checked" in
  "- [x] Use \`Fleet\` only as a provisional source-alpha working name."*)
    check_provisional_name
    ;;
  "- [x] Rename the product and package namespaces before public visibility.")
    check_renamed_before_public
    ;;
  "- [x] Owner has reviewed name/trademark clearance and accepts using \`Fleet\`"*)
    check_owner_clearance
    ;;
  "- [x] Other: "*)
    other_value="$(printf '%s\n' "$checked" | sed -n 's/^- \[x\] Other: `\(.*\)`$/\1/p')"
    if [ -z "$other_value" ] || [ "$other_value" = "TODO" ]; then
      echo "FAIL: checked Other name-collision decision must contain a concrete value"
      exit 1
    fi
    reject_placeholder_file "docs/release/NAME_COLLISION_REVIEW.md"
    require_text "docs/release/NAME_COLLISION_REVIEW.md" '^Owner decision:' \
      "a concrete owner decision line"
    ;;
  *)
    echo "FAIL: unsupported public name collision and trademark decision: $checked"
    exit 1
    ;;
esac

echo "Name collision decision check passed."
