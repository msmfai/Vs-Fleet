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

versioning_block="$(
  sed -n '/^### 14\. Versioning And Compatibility$/,/^## Required Before Binary Distribution$/p' "$owner_record"
)"

checked_count="$(printf '%s\n' "$versioning_block" | rg -c '^- \[x\] ' || true)"
checked_count="${checked_count:-0}"
if [ "$checked_count" -ne 1 ]; then
  echo "FAIL: versioning and compatibility decision must have exactly one checked choice; found $checked_count"
  exit 1
fi

checked="$(printf '%s\n' "$versioning_block" | rg '^- \[x\] ' | head -n1)"

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
    echo "FAIL: $file still contains placeholder versioning text"
    exit 1
  fi
}

check_alpha_unstable() {
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" \
    'Version: `?\[v0\.1\.0-alpha\.1\]`?' \
    "an alpha pre-release version placeholder"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" \
    'No stable upgrade path is promised during alpha' \
    "no stable upgrade path promise"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" \
    'No auto-update channel is enabled unless explicitly approved' \
    "no auto-update promise"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" \
    'production support, stable APIs, or backwards-compatible state formats' \
    "no stable API/state compatibility promise"
  require_text "SUPPORT.md" 'stable release lines' "no stable release-line statement"
  require_text "SECURITY.md" 'no stable release lines' "no stable security release-line promise"
  require_text "docs/release/RELEASE_PROCESS.md" 'v0\.1\.0-alpha\.1' \
    "the first alpha tag"
  require_text "docs/release/RELEASE_PROCESS.md" 'upgrade and rollback expectations' \
    "upgrade/rollback review reminder"
}

case "$checked" in
  "- [x] Alpha pre-release tags only. No stable API, protocol, state-file, or"*)
    check_alpha_unstable
    ;;
  "- [x] Commit to semver-compatible public CLI, protocol, and state changes"*)
    reject_placeholder_file "docs/release/VERSIONING.md"
    require_text "docs/release/VERSIONING.md" '^Versioning commitment:' \
      "a concrete Versioning commitment line"
    require_text "docs/release/VERSIONING.md" '^Compatibility scope:' \
      "a concrete Compatibility scope line"
    require_text "docs/release/VERSIONING.md" '^Migration policy:' \
      "a concrete Migration policy line"
    ;;
  "- [x] Other: "*)
    other_value="$(printf '%s\n' "$checked" | sed -n 's/^- \[x\] Other: `\(.*\)`$/\1/p')"
    if [ -z "$other_value" ] || [ "$other_value" = "TODO" ]; then
      echo "FAIL: checked Other versioning decision must contain a concrete value"
      exit 1
    fi
    reject_placeholder_file "docs/release/VERSIONING.md"
    require_text "docs/release/VERSIONING.md" '^Versioning commitment:' \
      "a concrete Versioning commitment line"
    ;;
  *)
    echo "FAIL: unsupported versioning and compatibility decision: $checked"
    exit 1
    ;;
esac

echo "Versioning decision check passed."
