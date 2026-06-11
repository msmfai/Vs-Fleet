#!/usr/bin/env bash
set -euo pipefail

file="${1:-docs/release/OWNER_DECISION_RECORD.md}"

if [ ! -f "$file" ]; then
  echo "FAIL: missing $file"
  exit 1
fi

if ! rg -q '^Decision record status: APPROVED$' "$file"; then
  echo "FAIL: owner decision record is not approved"
  exit 1
fi

if rg -n '^- \[x\] Other: `TODO`' "$file"; then
  echo "FAIL: owner decision record has a checked Other choice without a concrete value"
  exit 1
fi

required_start="$(rg -n '^## Required Before Public GitHub Visibility$' "$file" | cut -d: -f1 | head -n1)"
required_end="$(rg -n '^## Required Before Binary Distribution$' "$file" | cut -d: -f1 | head -n1)"

if [ -z "$required_start" ] || [ -z "$required_end" ] || [ "$required_end" -le "$required_start" ]; then
  echo "FAIL: owner decision record required section boundaries are missing"
  exit 1
fi

required_block="$(sed -n "$((required_start + 1)),$((required_end - 1))p" "$file")"

missing_required=0
for section in \
  "### 1. License" \
  "### 2. Public History" \
  "### 4. Distribution Scope" \
  "### 5. Security Reporting Channel" \
  "### 6. Contribution Intake" \
  "### 7. Public CI Evidence"
do
  section_line="$(printf '%s\n' "$required_block" | rg -n "^${section}$" | cut -d: -f1 | head -n1 || true)"
  if [ -z "$section_line" ]; then
    echo "FAIL: owner decision record missing required section: $section"
    missing_required=1
    continue
  fi

  next_section_line="$(
    printf '%s\n' "$required_block" |
      tail -n +"$((section_line + 1))" |
      rg -n '^### ' |
      cut -d: -f1 |
      head -n1 || true
  )"

  if [ -n "$next_section_line" ]; then
    block="$(printf '%s\n' "$required_block" | sed -n "${section_line},$((section_line + next_section_line - 1))p")"
  else
    block="$(printf '%s\n' "$required_block" | sed -n "${section_line},\$p")"
  fi

  checked_count="$(printf '%s\n' "$block" | rg -c '^- \[x\] ' || true)"
  if [ "$checked_count" -ne 1 ]; then
    echo "FAIL: $section must have exactly one checked choice; found $checked_count"
    missing_required=1
  fi
done

namespace_block="$(printf '%s\n' "$required_block" | sed -n '/^### 3\. Public Namespace$/,/^### 4\. Distribution Scope$/p')"
if printf '%s\n' "$namespace_block" | rg -n '`TODO`'; then
  echo "FAIL: Public Namespace table still contains TODO placeholders"
  missing_required=1
fi

if [ "$missing_required" -ne 0 ]; then
  exit 1
fi

echo "Owner decision record passed."
