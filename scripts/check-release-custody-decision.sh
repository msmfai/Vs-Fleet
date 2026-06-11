#!/usr/bin/env bash
set -euo pipefail

owner_record="${1:-docs/release/OWNER_DECISION_RECORD.md}"
evidence_file="${2:-docs/release/GITHUB_PUBLICATION_EVIDENCE.md}"
root="${3:-.}"

if [ ! -f "$owner_record" ]; then
  echo "FAIL: missing owner decision record: $owner_record"
  exit 1
fi

if [ ! -f "$evidence_file" ]; then
  echo "FAIL: missing GitHub publication evidence record: $evidence_file"
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

field_value() {
  local label=$1
  local line
  line="$(rg -i "^${label}:" "$evidence_file" | head -n1 || true)"
  if [ -z "$line" ]; then
    return 1
  fi
  local value="${line#*:}"
  value="$(printf '%s' "$value" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//; s/^`//; s/`$//')"
  printf '%s\n' "$value"
}

require_field_pattern() {
  local label=$1
  local pattern=$2
  local description=$3
  local value
  if ! value="$(field_value "$label")"; then
    echo "FAIL: $evidence_file must contain $label"
    exit 1
  fi
  if printf '%s\n' "$value" | rg -qi 'TODO|TBD|PLACEHOLDER|PENDING'; then
    echo "FAIL: $label still contains placeholder text"
    exit 1
  fi
  if ! printf '%s\n' "$value" | rg -qi "$pattern"; then
    echo "FAIL: $label must contain $description"
    exit 1
  fi
}

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
  require_field_pattern "Release authority" \
    'single maintainer|owner maintainer|repository owner' \
    "single-maintainer or repository-owner release authority"
  require_field_pattern "Tag protection or accepted unavailable reason" \
    '^(enabled|owner-approved deferred: .+|unavailable: .+)$' \
    "enabled, unavailable, or owner-approved deferred tag protection"
  require_field_pattern "Release artifact custody" \
    'source tags and release notes only|no binary artifacts|source-only' \
    "source-only release artifact custody"
  require_field_pattern "Package publishing credentials" \
    'none for source-only alpha|disabled|not created|not used' \
    "no package publishing credentials for source-only alpha"
  require_field_pattern "Emergency removal owner" \
    '.+' \
    "a concrete emergency removal owner"
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
