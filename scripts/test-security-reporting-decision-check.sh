#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

write_owner_record() {
  local file=$1
  local status=$2
  local checked=$3
  cat >"$file" <<EOF
# Owner Decision Record

Decision record status: $status

## Required Before Public GitHub Visibility

### 6. Security Reporting Channel

- [$([ "$checked" = "pvr" ] && echo x || echo ' ')] Enable GitHub Private Vulnerability Reporting.
- [$([ "$checked" = "contact" ] && echo x || echo ' ')] Add a private security email/contact to \`SECURITY.md\`.
- [$([ "$checked" = "other" ] && echo x || echo ' ')] Other: \`Use a private maintainer intake form\`

### 7. Contribution Intake
EOF
}

write_security() {
  local file=$1
  local body=$2
  printf '# Security Policy\n\n## Reporting a vulnerability\n\n%s\n' "$body" >"$file"
}

expect_pass() {
  local label=$1
  local owner=$2
  local security=$3
  if ! "$ROOT/scripts/check-security-reporting-decision.sh" "$owner" "$security" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  local owner=$2
  local security=$3
  if "$ROOT/scripts/check-security-reporting-decision.sh" "$owner" "$security" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

owner_pvr="$TMPDIR/owner-pvr.md"
security_pvr="$TMPDIR/security-pvr.md"
write_owner_record "$owner_pvr" APPROVED pvr
write_security "$security_pvr" "GitHub Private Vulnerability Reporting is enabled for this repository."
expect_pass "enabled GitHub PVR is accepted" "$owner_pvr" "$security_pvr"

security_pvr_ambiguous="$TMPDIR/security-pvr-ambiguous.md"
write_security "$security_pvr_ambiguous" "Use GitHub Private Vulnerability Reporting once it is enabled."
expect_fail "ambiguous GitHub PVR wording is rejected" "$owner_pvr" "$security_pvr_ambiguous"

owner_contact="$TMPDIR/owner-contact.md"
security_contact="$TMPDIR/security-contact.md"
write_owner_record "$owner_contact" APPROVED contact
write_security "$security_contact" "Security contact: security@example.invalid"
expect_pass "private contact line is accepted" "$owner_contact" "$security_contact"

security_missing_contact="$TMPDIR/security-missing-contact.md"
write_security "$security_missing_contact" "Email the maintainer after asking for a private reporting channel first."
expect_fail "private contact choice requires contact line" "$owner_contact" "$security_missing_contact"

security_placeholder_contact="$TMPDIR/security-placeholder-contact.md"
write_security "$security_placeholder_contact" "Security contact: TODO"
expect_fail "placeholder private contact is rejected" "$owner_contact" "$security_placeholder_contact"

owner_other="$TMPDIR/owner-other.md"
security_other="$TMPDIR/security-other.md"
write_owner_record "$owner_other" APPROVED other
write_security "$security_other" "Security reporting path: private maintainer intake form"
expect_pass "concrete Other reporting path is accepted" "$owner_other" "$security_other"

owner_multi="$TMPDIR/owner-multi.md"
cat >"$owner_multi" <<'EOF'
# Owner Decision Record

Decision record status: APPROVED

## Required Before Public GitHub Visibility

### 6. Security Reporting Channel

- [x] Enable GitHub Private Vulnerability Reporting.
- [x] Add a private security email/contact to `SECURITY.md`.
- [ ] Other: `TODO`

### 7. Contribution Intake
EOF
expect_fail "multiple checked security choices are rejected" "$owner_multi" "$security_pvr"

owner_pending="$TMPDIR/owner-pending.md"
write_owner_record "$owner_pending" PENDING pvr
expect_fail "pending owner record is rejected" "$owner_pending" "$security_pvr"

echo "Security reporting decision check tests passed."
