#!/usr/bin/env bash
set -euo pipefail

owner_record="${1:-docs/release/OWNER_DECISION_RECORD.md}"
evidence_file="${2:-docs/release/PUBLIC_BRANCH_EVIDENCE.md}"
expected_source="${3:-}"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(git rev-parse --show-toplevel 2>/dev/null || true)"

release_control_path() {
  local path=$1
  local rel=""
  local physical_root=""
  local physical_path=""

  if [ -n "$root" ]; then
    physical_root="$(cd "$root" && pwd -P)"
    case "$path" in
      "$root"/*) rel="${path#"$root/"}" ;;
      /*)
        if [ -d "$(dirname "$path")" ]; then
          physical_path="$(cd "$(dirname "$path")" && pwd -P)/$(basename "$path")"
          case "$physical_path" in
            "$physical_root"/*) rel="${physical_path#"$physical_root/"}" ;;
            *) rel="" ;;
          esac
        fi
        ;;
      *) rel="$path" ;;
    esac
  fi

  if [ -n "$rel" ]; then
    printf '%s\n' "$rel"
  fi
}

trees_match_except_release_control() {
  local left=$1
  local right=$2
  local allowed=$3

  if [ "$(git -C "$root" rev-parse "$left^{tree}")" = "$(git -C "$root" rev-parse "$right^{tree}")" ]; then
    return 0
  fi

  if [ -z "$allowed" ]; then
    return 1
  fi

  local diff_names
  diff_names="$(git -C "$root" diff --name-only "$left" "$right")"
  diff_names="$(
    printf '%s\n' "$diff_names" | awk -v allowed="$allowed" '
      NF &&
      $0 != allowed &&
      $0 != "docs/release/PUBLIC_BRANCH_EVIDENCE.md" &&
      $0 != "docs/release/PUBLIC_CI_EVIDENCE.md" &&
      $0 != "docs/release/GITHUB_PUBLICATION_EVIDENCE.md" &&
      $0 != "docs/release/DEPENDENCY_REVIEW_EVIDENCE.md" {
        print
      }
    '
  )"
  [ -z "$diff_names" ]
}

if [ ! -f "$owner_record" ]; then
  echo "FAIL: missing owner decision record: $owner_record"
  exit 1
fi

if [ -z "$expected_source" ] && [ -n "$root" ]; then
  expected_source="$(git -C "$root" rev-parse HEAD)"
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
release_control_file="$(field_value "Release-control evidence file" || true)"
allowed_release_control="$(release_control_path "$evidence_file")"

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
  if [ -z "$allowed_release_control" ] ||
    ! trees_match_except_release_control "$source_commit" "$expected_source" "$allowed_release_control"; then
    echo "FAIL: public branch source commit $source_commit does not match expected commit $expected_source"
    exit 1
  fi
fi

if [ -n "$release_control_file" ] && [ "$release_control_file" != "$allowed_release_control" ]; then
  echo "FAIL: Release-control evidence file must match tracked evidence path $allowed_release_control"
  exit 1
fi

if ! git -C "$root" rev-parse --verify -q "$source_commit^{commit}" >/dev/null; then
  echo "FAIL: source commit does not exist locally: $source_commit"
  exit 1
fi

resolved_public="$(git -C "$root" rev-parse --verify "$public_branch^{commit}" 2>/dev/null || true)"
if [ -z "$resolved_public" ]; then
  echo "FAIL: public branch does not resolve locally: $public_branch"
  exit 1
fi

if [ "$resolved_public" != "$public_root" ]; then
  echo "FAIL: public branch $public_branch resolves to $resolved_public, expected $public_root"
  exit 1
fi

if [ "$(git -C "$root" rev-list --count "$public_branch")" != "1" ]; then
  echo "FAIL: public branch must contain exactly one commit"
  exit 1
fi

if [ "$(git -C "$root" rev-list --parents -n1 "$public_branch" | wc -w | tr -d ' ')" != "1" ]; then
  echo "FAIL: public branch root commit must have no parents"
  exit 1
fi

if ! trees_match_except_release_control "$source_commit" "$public_root" "$allowed_release_control"; then
  echo "FAIL: public branch tree does not match source commit tree outside release-control evidence"
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
