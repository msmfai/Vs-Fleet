#!/usr/bin/env bash
set -euo pipefail

file="${1:-docs/release/OWNER_DECISION_RECORD.md}"

if [ ! -f "$file" ]; then
  echo "FAIL: missing $file"
  exit 1
fi

fail=0

if ! rg -q '^Decision record status: APPROVED$' "$file"; then
  echo "FAIL: owner decision record is not approved"
  fail=1
fi

if rg -n '^- \[x\] Other: `TODO`' "$file"; then
  echo "FAIL: owner decision record has a checked Other choice without a concrete value"
  fail=1
fi

required_start="$(rg -n '^## Required Before Public GitHub Visibility$' "$file" | cut -d: -f1 | head -n1)"
required_end="$(rg -n '^## Required Before Binary Distribution$' "$file" | cut -d: -f1 | head -n1)"

if [ -z "$required_start" ] || [ -z "$required_end" ] || [ "$required_end" -le "$required_start" ]; then
  echo "FAIL: owner decision record required section boundaries are missing"
  exit 1
fi

required_block="$(sed -n "$((required_start + 1)),$((required_end - 1))p" "$file")"
binary_block="$(sed -n "$((required_end + 1)),\$p" "$file")"

section_block() {
  local source_block=$1
  local section=$2
  local section_line
  section_line="$(printf '%s\n' "$source_block" | rg -n "^${section}$" | cut -d: -f1 | head -n1 || true)"
  if [ -z "$section_line" ]; then
    return 1
  fi

  local next_section_line
  next_section_line="$(
    printf '%s\n' "$source_block" |
      tail -n +"$((section_line + 1))" |
      rg -n '^### ' |
      cut -d: -f1 |
      head -n1 || true
  )"

  if [ -n "$next_section_line" ]; then
    printf '%s\n' "$source_block" | sed -n "${section_line},$((section_line + next_section_line - 1))p"
  else
    printf '%s\n' "$source_block" | sed -n "${section_line},\$p"
  fi
}

missing_required=0
for section in \
  "### 1. License" \
  "### 2. Public History" \
  "### 4. Alpha Scope" \
  "### 5. Editor Server Licensing Boundary" \
  "### 6. Distribution Scope" \
  "### 7. Security Reporting Channel" \
  "### 8. Contribution Intake" \
  "### 9. Public CI Evidence" \
  "### 10. Privacy And Telemetry Posture" \
  "### 11. Dependency Review Evidence" \
  "### 12. Support Commitment" \
  "### 13. Branding Stability" \
  "### 14. Versioning And Compatibility" \
  "### 15. Community Intake And Moderation" \
  "### 16. Release Custody And Maintainer Authority" \
  "### 17. AI-Assisted Contribution Provenance" \
  "### 18. Supported Platform And Toolchain" \
  "### 19. Public Roadmap And Non-Goals" \
  "### 20. Public Name Collision And Trademark Posture" \
  "### 21. Local Data And Uninstall Policy" \
  "### 22. GitHub Actions Supply-Chain Posture"
do
  if ! block="$(section_block "$required_block" "$section")"; then
    echo "FAIL: owner decision record missing required section: $section"
    missing_required=1
    continue
  fi

  checked_count="$(printf '%s\n' "$block" | rg -c '^- \[x\] ' || true)"
  checked_count="${checked_count:-0}"
  if [ "$checked_count" -ne 1 ]; then
    echo "FAIL: $section must have exactly one checked choice; found $checked_count"
    missing_required=1
  fi
done

namespace_block="$(printf '%s\n' "$required_block" | sed -n '/^### 3\. Public Namespace$/,/^### 4\. Alpha Scope$/p')"
namespace_todos="$(printf '%s\n' "$namespace_block" | rg '`TODO`' || true)"
if [ -n "$namespace_todos" ]; then
  echo "FAIL: Public Namespace table still contains TODO placeholders"
  printf '%s\n' "$namespace_todos"
  missing_required=1
fi

if distribution_block="$(section_block "$required_block" "### 6. Distribution Scope")"; then
  if printf '%s\n' "$distribution_block" | rg -q '^- \[x\] Source plus|^- \[x\] Other:'; then
    for section in \
      "### 23. macOS Signing and Notarization" \
      "### 24. Update Channel"
    do
      if ! block="$(section_block "$binary_block" "$section")"; then
        echo "FAIL: owner decision record missing binary distribution section: $section"
        missing_required=1
        continue
      fi

      checked_count="$(printf '%s\n' "$block" | rg -c '^- \[x\] ' || true)"
      checked_count="${checked_count:-0}"
      if [ "$checked_count" -ne 1 ]; then
        echo "FAIL: $section must have exactly one checked choice when public binary distribution is selected; found $checked_count"
        missing_required=1
      fi
    done
  fi
fi

if [ "$missing_required" -ne 0 ]; then
  fail=1
fi

if [ "$fail" -ne 0 ]; then
  exit 1
fi

echo "Owner decision record passed."
