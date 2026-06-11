#!/usr/bin/env bash
set -euo pipefail

owner_record="${1:-docs/release/OWNER_DECISION_RECORD.md}"
evidence_file="${2:-docs/release/GITHUB_PUBLICATION_EVIDENCE.md}"
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
  echo "FAIL: missing GitHub publication evidence record: $evidence_file"
  exit 1
fi

if [ -z "$expected_commit" ] && [ -n "$root" ]; then
  expected_commit="$(git -C "$root" rev-parse HEAD)"
fi

if ! rg -q '^Decision record status: APPROVED$' "$owner_record"; then
  echo "FAIL: owner decision record is not approved"
  exit 1
fi

reject_placeholder_evidence() {
  if rg -ni 'TODO|TBD|PLACEHOLDER|PENDING|not yet reviewed|not yet configured' "$evidence_file"; then
    echo "FAIL: GitHub publication evidence record still contains placeholder text"
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

namespace_block="$(
  sed -n '/^### 3\. Public Namespace$/,/^### 4\. Alpha Scope$/p' "$owner_record"
)"

decision_for() {
  local surface=$1
  printf '%s\n' "$namespace_block" |
    awk -F'|' -v surface="$surface" '
      function trim(s) {
        gsub(/^[[:space:]]+|[[:space:]]+$/, "", s)
        return s
      }
      trim($2) == surface {
        value = trim($3)
        gsub(/^`|`$/, "", value)
        print value
        found = 1
        exit
      }
      END { if (!found) exit 1 }
    '
}

require_namespace_value() {
  local surface=$1
  local value
  if ! value="$(decision_for "$surface")"; then
    echo "FAIL: Public Namespace table missing decision for $surface"
    exit 1
  fi
  if [ -z "$value" ] || [[ "$value" == *TODO* ]] || [[ "$value" == *" or "* ]]; then
    echo "FAIL: Public Namespace decision for $surface is not concrete: $value"
    exit 1
  fi
  printf '%s\n' "$value"
}

require_status() {
  local status
  if ! status="$(field_value "GitHub publication evidence status")"; then
    echo "FAIL: $evidence_file must contain 'GitHub publication evidence status: PASS'"
    exit 1
  fi
  if [ "$status" != "PASS" ]; then
    echo "FAIL: GitHub publication evidence status is \"$status\", expected \"PASS\""
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
    allowed_release_control="$(release_control_path "$evidence_file")"
    release_control_file="$(field_value "Release-control evidence file" || true)"
    if [ -n "$release_control_file" ] && [ "$release_control_file" != "$allowed_release_control" ]; then
      echo "FAIL: Release-control evidence file must match checked evidence path $allowed_release_control"
      exit 1
    fi
    if [ -z "$allowed_release_control" ] ||
      ! trees_match_except_release_control "$commit" "$expected_commit" "$allowed_release_control"; then
      echo "FAIL: GitHub publication evidence commit $commit does not match expected commit $expected_commit"
      exit 1
    fi
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

require_field_pattern() {
  local label=$1
  local pattern=$2
  local description=$3
  local value
  if ! value="$(field_value "$label")"; then
    echo "FAIL: $evidence_file must contain $label"
    exit 1
  fi
  if ! printf '%s\n' "$value" | rg -qi "$pattern"; then
    echo "FAIL: $label must contain $description"
    exit 1
  fi
}

require_not_value() {
  local label=$1
  local rejected=$2
  local value
  if ! value="$(field_value "$label")"; then
    echo "FAIL: $evidence_file must contain $label"
    exit 1
  fi
  if [ "$value" = "$rejected" ]; then
    echo "FAIL: $label must not be \"$rejected\" for first public alpha"
    exit 1
  fi
}

reject_placeholder_evidence
require_status
require_commit

github_org="$(require_namespace_value "GitHub org/user")"
github_repo="$(require_namespace_value "GitHub repo name")"
expected_repo_url="https://github.com/$github_org/$github_repo"

require_field_value "Repository" "$expected_repo_url"
require_field_pattern "Default branch" '^[A-Za-z0-9._/-]+$' "a concrete branch name"
require_field_value "Visibility consequences reviewed" "yes"
require_field_value "Repository name matches namespace" "yes"
require_field_pattern "Issues setting" '^(enabled|disabled|enabled per support commitment|disabled per support commitment)$' \
  "enabled/disabled and consistency with support commitment"
require_field_pattern "Discussions setting" '^(disabled|enabled by owner decision)$' \
  "disabled unless deliberately enabled"
require_field_pattern "Wiki setting" '^(disabled|enabled by owner decision)$' \
  "disabled unless deliberately enabled"
require_field_pattern "Releases setting" 'source tags and release notes only' \
  "source-only release scope"
require_field_pattern "Packages setting" '^(disabled|not used for source-only alpha)$' \
  "packages disabled or unused for source-only alpha"
require_field_value "GitHub Actions setting" "enabled"
require_field_pattern "Security reporting channel available" '^(GitHub Private Vulnerability Reporting enabled|private contact documented in SECURITY\.md|other approved private channel available: .+)$' \
  "the approved security reporting channel"
require_field_pattern "Secret scanning or accepted unavailable reason" '^(enabled|unavailable: .+)$' \
  "enabled or a concrete unavailable reason"
require_field_pattern "Dependabot alerts or accepted unavailable reason" '^(enabled|unavailable: .+)$' \
  "enabled or a concrete unavailable reason"
require_not_value "Default branch protection" "none"
require_field_pattern "Default branch protection" '^(enabled|owner-approved deferred: .+)$' \
  "enabled or owner-approved deferred rationale"
require_field_pattern "Required source checks" 'source|CI|clippy|test' \
  "source checks or CI checks"
require_field_pattern "Required release checks" 'release-readiness|Release Readiness|release check' \
  "release readiness checks"
require_field_pattern "Linear history policy" '^(preferred|required|not required by owner decision: .+)$' \
  "a concrete linear-history policy"
require_field_pattern "Signed commit policy" '^(not required|required|deferred: .+)$' \
  "a concrete signed-commit policy"

echo "GitHub publication evidence check passed."
