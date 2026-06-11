#!/usr/bin/env bash
set -euo pipefail

owner_record="${1:-docs/release/OWNER_DECISION_RECORD.md}"
evidence_file="${2:-docs/release/PUBLIC_CI_EVIDENCE.md}"
expected_commit="${3:-}"
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

if [ ! -f "$evidence_file" ]; then
  echo "FAIL: missing public CI evidence record: $evidence_file"
  exit 1
fi

if [ -z "$expected_commit" ] && [ -n "$root" ]; then
  expected_commit="$(git -C "$root" rev-parse HEAD)"
fi

if ! rg -q '^Decision record status: APPROVED$' "$owner_record"; then
  echo "FAIL: owner decision record is not approved"
  exit 1
fi

ci_block="$(
  sed -n '/^### 9\. Public CI Evidence$/,/^### 10\. Privacy And Telemetry Posture$/p' "$owner_record"
)"

checked_count="$(printf '%s\n' "$ci_block" | rg -c '^- \[x\] ' || true)"
checked_count="${checked_count:-0}"
if [ "$checked_count" -ne 1 ]; then
  echo "FAIL: public CI evidence decision must have exactly one checked choice; found $checked_count"
  exit 1
fi

checked="$(printf '%s\n' "$ci_block" | rg '^- \[x\] ' | head -n1)"

reject_placeholder_evidence() {
  if rg -ni 'TODO|TBD|PLACEHOLDER|PENDING|not yet run' "$evidence_file"; then
    echo "FAIL: public CI evidence record still contains placeholder text"
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
    allowed_release_control="$(release_control_path "$evidence_file")"
    release_control_file="$(field_value "Release-control evidence file" || true)"
    if [ -n "$release_control_file" ] && [ "$release_control_file" != "$allowed_release_control" ]; then
      echo "FAIL: Release-control evidence file must match checked evidence path $allowed_release_control"
      exit 1
    fi
    if [ -z "$allowed_release_control" ] ||
      ! trees_match_except_release_control "$commit" "$expected_commit" "$allowed_release_control"; then
      echo "FAIL: CI evidence commit $commit does not match expected commit $expected_commit"
      exit 1
    fi
  fi
}

require_status() {
  local expected=$1
  local status
  if ! status="$(field_value "Public CI evidence status")"; then
    echo "FAIL: $evidence_file must contain 'Public CI evidence status: $expected'"
    exit 1
  fi
  if [ "$status" != "$expected" ]; then
    echo "FAIL: public CI evidence status is \"$status\", expected \"$expected\""
    exit 1
  fi
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
  if ! printf '%s\n' "$value" | rg -q "$pattern"; then
    echo "FAIL: $label must contain $description"
    exit 1
  fi
}

case "$checked" in
  "- [x] Require GitHub Actions green on the exact branch/commit before public"*)
    reject_placeholder_evidence
    require_status "PASS"
    require_commit
    require_field_pattern "Branch" '^[A-Za-z0-9._/-]+$' "a concrete branch name"
    require_field_pattern "CI workflow run" '^https://github\.com/[^/]+/[^/]+/actions/runs/[0-9]+$' \
      "a GitHub Actions run URL"
    require_field_pattern "Release Readiness workflow run" '^https://github\.com/[^/]+/[^/]+/actions/runs/[0-9]+$' \
      "a GitHub Actions run URL"
    ;;
  "- [x] Accept local check output only for the first publish.")
    reject_placeholder_evidence
    require_status "LOCAL_ONLY"
    require_commit
    require_field_pattern "Local check transcript" '.+' "a concrete transcript path or URL"
    ;;
  "- [x] Other: "*)
    other_value="$(printf '%s\n' "$checked" | sed -n 's/^- \[x\] Other: `\(.*\)`$/\1/p')"
    if [ -z "$other_value" ] || [ "$other_value" = "TODO" ]; then
      echo "FAIL: checked Other CI evidence decision must contain a concrete value"
      exit 1
    fi
    reject_placeholder_evidence
    require_status "PASS"
    require_commit
    require_field_pattern "CI evidence path" '.+' "a concrete CI evidence path or URL"
    ;;
  *)
    echo "FAIL: unsupported public CI evidence decision: $checked"
    exit 1
    ;;
esac

echo "Public CI evidence decision check passed."
