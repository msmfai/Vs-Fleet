#!/usr/bin/env bash
set -euo pipefail

owner_record="${1:-docs/release/OWNER_DECISION_RECORD.md}"
support_file="${2:-SUPPORT.md}"
root="${3:-.}"

if [ ! -f "$owner_record" ]; then
  echo "FAIL: missing owner decision record: $owner_record"
  exit 1
fi

if [ ! -f "$support_file" ]; then
  echo "FAIL: missing support policy: $support_file"
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

support_block="$(
  sed -n '/^### 12\. Support Commitment$/,/^### 13\. Branding Stability$/p' "$owner_record"
)"

checked_count="$(printf '%s\n' "$support_block" | rg -c '^- \[x\] ' || true)"
checked_count="${checked_count:-0}"
if [ "$checked_count" -ne 1 ]; then
  echo "FAIL: support commitment decision must have exactly one checked choice; found $checked_count"
  exit 1
fi

checked="$(printf '%s\n' "$support_block" | rg '^- \[x\] ' | head -n1)"

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
  if [ "$file" = "$support_file" ]; then
    if ! rg -qi "$pattern" "$support_file"; then
      echo "FAIL: $support_file must contain $description"
      exit 1
    fi
    return
  fi
  require_file "$file"
  if ! rg -qi "$pattern" "$root/$file"; then
    echo "FAIL: $file must contain $description"
    exit 1
  fi
}

reject_placeholder_file() {
  local file=$1
  if [ "$file" = "$support_file" ]; then
    if rg -ni 'TODO|TBD|PLACEHOLDER' "$support_file"; then
      echo "FAIL: $support_file still contains placeholder support text"
      exit 1
    fi
    return
  fi
  require_file "$file"
  if rg -ni 'TODO|TBD|PLACEHOLDER' "$root/$file"; then
    echo "FAIL: $file still contains placeholder support text"
    exit 1
  fi
}

reject_sla_text() {
  if rg -ni 'response (target|time|SLA)|support guarantee|production support|paid support|stable release line' "$support_file" >/tmp/fleet-support-sla.$$; then
    # The best-effort policy is allowed to reject these promises explicitly.
    if ! rg -qi 'no production support guarantees|no .*response SLAs|no .*paid support|no .*stable release lines' "$support_file"; then
      echo "FAIL: $support_file contains support/SLA language without explicit no-SLA/no-production boundary"
      sed -n '1,20p' /tmp/fleet-support-sla.$$
      rm -f /tmp/fleet-support-sla.$$
      exit 1
    fi
  fi
  rm -f /tmp/fleet-support-sla.$$
}

check_best_effort_alpha() {
  require_text "$support_file" 'pre-release alpha software' "pre-release alpha warning"
  require_text "$support_file" 'Support is best-effort' "best-effort support statement"
  require_text "$support_file" 'Breaking changes are expected' "breaking-changes warning"
  require_text "$support_file" 'no production support guarantees' "no production support guarantee"
  require_text "$support_file" 'response SLAs' "no response SLA statement"
  require_text "$support_file" 'paid support' "no paid support statement"
  require_text "$support_file" 'stable release lines' "no stable release-line statement"
  require_text "$support_file" 'Source builds and local macOS dogfooding' \
    "source/local macOS support scope"
  require_text "$support_file" 'not supported' "unsupported support scope boundary"
  require_text "$support_file" 'alpha commitments' "alpha commitment boundary"
  require_text "$support_file" 'Security vulnerabilities should follow `?SECURITY\.md`?' \
    "security reports excluded from public support issues"
  require_text "README.md" 'SUPPORT\.md.*alpha support boundary|alpha support boundary.*SUPPORT\.md' \
    "README support policy pointer"
  require_text "docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" 'production support, stable APIs, or backwards-compatible state formats' \
    "release-notes support non-commitment"
  reject_sla_text
}

case "$checked" in
  "- [x] Best-effort alpha support only. Breaking changes are expected;"*)
    check_best_effort_alpha
    ;;
  "- [x] Define a public triage or response target in \`SUPPORT.md\`.")
    reject_placeholder_file "$support_file"
    require_text "$support_file" '^Support commitment:' "a concrete Support commitment line"
    require_text "$support_file" '^Response target:' "a concrete Response target line"
    require_text "$support_file" '^Supported scope:' "a concrete Supported scope line"
    ;;
  "- [x] Other: "*)
    other_value="$(printf '%s\n' "$checked" | sed -n 's/^- \[x\] Other: `\(.*\)`$/\1/p')"
    if [ -z "$other_value" ] || [ "$other_value" = "TODO" ]; then
      echo "FAIL: checked Other support decision must contain a concrete value"
      exit 1
    fi
    reject_placeholder_file "$support_file"
    require_text "$support_file" '^Support commitment:' "a concrete Support commitment line"
    ;;
  *)
    echo "FAIL: unsupported support commitment decision: $checked"
    exit 1
    ;;
esac

echo "Support decision check passed."
