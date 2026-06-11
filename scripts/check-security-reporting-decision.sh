#!/usr/bin/env bash
set -euo pipefail

owner_record="${1:-docs/release/OWNER_DECISION_RECORD.md}"
security_file="${2:-SECURITY.md}"

if [ ! -f "$owner_record" ]; then
  echo "FAIL: missing owner decision record: $owner_record"
  exit 1
fi

if [ ! -f "$security_file" ]; then
  echo "FAIL: missing security policy: $security_file"
  exit 1
fi

if ! rg -q '^Decision record status: APPROVED$' "$owner_record"; then
  echo "FAIL: owner decision record is not approved"
  exit 1
fi

security_block="$(
  sed -n '/^### 5\. Security Reporting Channel$/,/^### 6\. Contribution Intake$/p' "$owner_record"
)"

checked_count="$(printf '%s\n' "$security_block" | rg -c '^- \[x\] ' || true)"
checked_count="${checked_count:-0}"
if [ "$checked_count" -ne 1 ]; then
  echo "FAIL: security reporting decision must have exactly one checked choice; found $checked_count"
  exit 1
fi

checked="$(printf '%s\n' "$security_block" | rg '^- \[x\] ' | head -n1)"

reject_ambiguous_security_text() {
  local ambiguous='once it is enabled|if private reporting is not enabled yet|ask for a private reporting channel first|contact the maintainer out of band'
  if rg -ni "$ambiguous" "$security_file"; then
    echo "FAIL: SECURITY.md still contains ambiguous pre-decision reporting language"
    exit 1
  fi
}

line_value_is_concrete() {
  local label=$1
  local line
  line="$(rg -i "^${label}:" "$security_file" | head -n1 || true)"
  if [ -z "$line" ]; then
    return 1
  fi

  local value="${line#*:}"
  value="$(printf '%s' "$value" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//')"
  if [ -z "$value" ] || [[ "$value" =~ TODO|TBD|PLACEHOLDER|your-security-contact ]]; then
    return 1
  fi
  return 0
}

case "$checked" in
  "- [x] Enable GitHub Private Vulnerability Reporting.")
    reject_ambiguous_security_text
    if ! rg -qi 'GitHub Private Vulnerability Reporting is enabled' "$security_file"; then
      echo "FAIL: SECURITY.md must explicitly state GitHub Private Vulnerability Reporting is enabled"
      exit 1
    fi
    ;;
  "- [x] Add a private security email/contact to \`SECURITY.md\`.")
    reject_ambiguous_security_text
    if ! line_value_is_concrete "Security contact"; then
      echo "FAIL: SECURITY.md must include a concrete 'Security contact:' line"
      exit 1
    fi
    ;;
  "- [x] Other: "*)
    other_value="$(printf '%s\n' "$checked" | sed -n 's/^- \[x\] Other: `\(.*\)`$/\1/p')"
    if [ -z "$other_value" ] || [ "$other_value" = "TODO" ]; then
      echo "FAIL: checked Other security reporting decision must contain a concrete value"
      exit 1
    fi
    reject_ambiguous_security_text
    if ! line_value_is_concrete "Security reporting path" && ! line_value_is_concrete "Security contact"; then
      echo "FAIL: SECURITY.md must include a concrete 'Security reporting path:' or 'Security contact:' line"
      exit 1
    fi
    ;;
  *)
    echo "FAIL: unsupported security reporting decision: $checked"
    exit 1
    ;;
esac

echo "Security reporting decision check passed."
