#!/usr/bin/env bash
set -euo pipefail

owner_record="${1:-docs/release/OWNER_DECISION_RECORD.md}"
evidence_file="${2:-docs/release/PUBLIC_BRANCH_EVIDENCE.md}"
expected_source="${3:-}"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [ ! -f "$owner_record" ]; then
  echo "FAIL: missing owner decision record: $owner_record"
  exit 1
fi

if [ -z "$expected_source" ] && git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  expected_source="$(git rev-parse HEAD)"
fi

if ! rg -q '^Decision record status: APPROVED$' "$owner_record"; then
  echo "FAIL: owner decision record is not approved"
  exit 1
fi

history_block="$(
  sed -n '/^### 2\. Public History$/,/^### 3\. Public Namespace$/p' "$owner_record"
)"

checked_count="$(printf '%s\n' "$history_block" | rg -c '^- \[x\] ' || true)"
checked_count="${checked_count:-0}"
if [ "$checked_count" -ne 1 ]; then
  echo "FAIL: public history decision must have exactly one checked choice; found $checked_count"
  exit 1
fi

checked="$(printf '%s\n' "$history_block" | rg '^- \[x\] ' | head -n1)"

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

require_field() {
  local label=$1
  local value
  if ! value="$(field_value "$label")"; then
    echo "FAIL: $evidence_file must contain $label"
    exit 1
  fi
  if [ -z "$value" ] || [ "$value" = "TODO" ]; then
    echo "FAIL: $label must be concrete"
    exit 1
  fi
  printf '%s\n' "$value"
}

case "$checked" in
  "- [x] Publish the current branch history and accept that old commits may contain"*)
    echo "Public branch evidence check skipped: current history is explicitly accepted."
    exit 0
    ;;
  "- [x] Publish a cleaned/squashed history for the first public branch.")
    ;;
  *)
    echo "FAIL: unsupported public history decision: $checked"
    exit 1
    ;;
esac

if [ ! -f "$evidence_file" ]; then
  echo "FAIL: missing public branch evidence record: $evidence_file"
  exit 1
fi

if rg -ni 'TODO|TBD|PLACEHOLDER|PENDING|not yet run' "$evidence_file"; then
  echo "FAIL: public branch evidence record still contains placeholder text"
  exit 1
fi

status="$(require_field "Public branch evidence status")"
if [ "$status" != "PASS" ]; then
  echo "FAIL: public branch evidence status is \"$status\", expected \"PASS\""
  exit 1
fi

source_commit="$(require_field "Source commit")"
public_branch="$(require_field "Public branch")"
public_root="$(require_field "Public root commit")"
history_result="$(require_field "History check result")"
single_root="$(require_field "Single root commit")"
tree_matches="$(require_field "Public tree matches source commit tree")"
no_private_history="$(require_field "Public branch contains no prior private history")"

for pair in \
  "Source commit:$source_commit" \
  "Public root commit:$public_root"
do
  value="${pair#*:}"
  if ! printf '%s\n' "$value" | rg -q '^[0-9a-f]{40}$'; then
    echo "FAIL: ${pair%%:*} must be a full 40-character lowercase git SHA"
    exit 1
  fi
done

if [ -n "$expected_source" ] && [ "$source_commit" != "$expected_source" ]; then
  echo "FAIL: public branch source commit $source_commit does not match expected commit $expected_source"
  exit 1
fi

if ! git rev-parse --verify -q "$source_commit^{commit}" >/dev/null; then
  echo "FAIL: source commit does not exist locally: $source_commit"
  exit 1
fi

resolved_public="$(git rev-parse --verify "$public_branch^{commit}" 2>/dev/null || true)"
if [ -z "$resolved_public" ]; then
  echo "FAIL: public branch does not resolve locally: $public_branch"
  exit 1
fi

if [ "$resolved_public" != "$public_root" ]; then
  echo "FAIL: public branch $public_branch resolves to $resolved_public, expected $public_root"
  exit 1
fi

if [ "$(git rev-list --count "$public_branch")" != "1" ]; then
  echo "FAIL: public branch must contain exactly one commit"
  exit 1
fi

if [ "$(git rev-list --parents -n1 "$public_branch" | wc -w | tr -d ' ')" != "1" ]; then
  echo "FAIL: public branch root commit must have no parents"
  exit 1
fi

source_tree="$(git rev-parse "$source_commit^{tree}")"
public_tree="$(git rev-parse "$public_root^{tree}")"
if [ "$source_tree" != "$public_tree" ]; then
  echo "FAIL: public branch tree does not match source commit tree"
  exit 1
fi

if [ "$history_result" != "PASS" ]; then
  echo "FAIL: History check result must be PASS"
  exit 1
fi

for pair in \
  "Single root commit:$single_root" \
  "Public tree matches source commit tree:$tree_matches" \
  "Public branch contains no prior private history:$no_private_history"
do
  value="${pair#*:}"
  if [ "$value" != "yes" ]; then
    echo "FAIL: ${pair%%:*} must be yes"
    exit 1
  fi
done

if ! "$script_dir/history-release-check.sh" "$owner_record" "$public_branch"; then
  echo "FAIL: public branch history release check did not pass"
  exit 1
fi

echo "Public branch evidence check passed."
