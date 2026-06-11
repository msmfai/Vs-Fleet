#!/usr/bin/env bash
set -euo pipefail

owner_record="${1:-docs/release/OWNER_DECISION_RECORD.md}"
evidence_file="${2:-docs/release/DEPENDENCY_REVIEW_EVIDENCE.md}"
expected_commit="${3:-}"

if [ ! -f "$owner_record" ]; then
  echo "FAIL: missing owner decision record: $owner_record"
  exit 1
fi

if [ ! -f "$evidence_file" ]; then
  echo "FAIL: missing dependency review evidence record: $evidence_file"
  exit 1
fi

if [ -z "$expected_commit" ] && git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  expected_commit="$(git rev-parse HEAD)"
fi

if ! rg -q '^Decision record status: APPROVED$' "$owner_record"; then
  echo "FAIL: owner decision record is not approved"
  exit 1
fi

review_block="$(
  sed -n '/^### 11\. Dependency Review Evidence$/,/^## Required Before Binary Distribution$/p' "$owner_record"
)"

checked_count="$(printf '%s\n' "$review_block" | rg -c '^- \[x\] ' || true)"
checked_count="${checked_count:-0}"
if [ "$checked_count" -ne 1 ]; then
  echo "FAIL: dependency review decision must have exactly one checked choice; found $checked_count"
  exit 1
fi

checked="$(printf '%s\n' "$review_block" | rg '^- \[x\] ' | head -n1)"

reject_placeholder_evidence() {
  if rg -ni 'TODO|TBD|PLACEHOLDER|PENDING|not yet run' "$evidence_file"; then
    echo "FAIL: dependency review evidence record still contains placeholder text"
    exit 1
  fi
}

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

require_status() {
  local expected=$1
  local status
  if ! status="$(field_value "Dependency review status")"; then
    echo "FAIL: $evidence_file must contain 'Dependency review status: $expected'"
    exit 1
  fi
  if [ "$status" != "$expected" ]; then
    echo "FAIL: dependency review status is \"$status\", expected \"$expected\""
    exit 1
  fi
}

require_commit() {
  local commit
  if ! commit="$(field_value "Commit")"; then
    echo "FAIL: $evidence_file must contain a Commit field"
    exit 1
  fi
  if ! printf '%s\n' "$commit" | rg -q '^[0-9a-f]{40}$'; then
    echo "FAIL: Commit must be a full 40-character lowercase git SHA"
    exit 1
  fi
  if [ -n "$expected_commit" ] && [ "$commit" != "$expected_commit" ]; then
    echo "FAIL: dependency review commit $commit does not match expected commit $expected_commit"
    exit 1
  fi
}

require_field_value() {
  local label=$1
  local expected=$2
  local value
  if ! value="$(field_value "$label")"; then
    echo "FAIL: $evidence_file must contain $label"
    exit 1
  fi
  if [ "$value" != "$expected" ]; then
    echo "FAIL: $label is \"$value\", expected \"$expected\""
    exit 1
  fi
}

require_concrete_field() {
  local label=$1
  local value
  if ! value="$(field_value "$label")"; then
    echo "FAIL: $evidence_file must contain $label"
    exit 1
  fi
  if [ -z "$value" ]; then
    echo "FAIL: $label must be concrete"
    exit 1
  fi
}

case "$checked" in
  "- [x] Run the dependency review commands in \`docs/release/DEPENDENCY_REVIEW.md\`"*)
    reject_placeholder_evidence
    require_status "PASS"
    require_commit
    require_concrete_field "Reviewed date"
    require_field_value "cargo tree" "pass"
    require_field_value "cargo metadata --locked" "pass"
    require_field_value "fleet-bridge npm audit" "pass"
    require_field_value "extension npm audit" "pass"
    require_field_value "generated artifact check" "pass"
    require_concrete_field "Accepted findings"
    ;;
  "- [x] Publish the first source alpha without dependency review and accept"*)
    reject_placeholder_evidence
    require_status "SKIPPED_ACCEPTED_RISK"
    require_commit
    require_concrete_field "Accepted risk"
    ;;
  "- [x] Other: "*)
    other_value="$(printf '%s\n' "$checked" | sed -n 's/^- \[x\] Other: `\(.*\)`$/\1/p')"
    if [ -z "$other_value" ] || [ "$other_value" = "TODO" ]; then
      echo "FAIL: checked Other dependency review decision must contain a concrete value"
      exit 1
    fi
    reject_placeholder_evidence
    require_status "PASS"
    require_commit
    require_concrete_field "Dependency review evidence path"
    ;;
  *)
    echo "FAIL: unsupported dependency review decision: $checked"
    exit 1
    ;;
esac

echo "Dependency review decision check passed."
