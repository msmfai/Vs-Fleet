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

### 12. Support Commitment

- [$([ "$checked" = "best" ] && echo x || echo ' ')] Best-effort alpha support only. Breaking changes are expected; there are
  no production support guarantees, response SLAs, paid support terms, or stable
  release lines.
- [$([ "$checked" = "target" ] && echo x || echo ' ')] Define a public triage or response target in \`SUPPORT.md\`.
- [$([ "$checked" = "other" ] && echo x || echo ' ')] Other: \`Private support only\`

## Required Before Binary Distribution
EOF
}

write_best_effort_docs() {
  local root=$1
  mkdir -p "$root/docs/release"
  cat >"$root/SUPPORT.md" <<'EOF'
# Support

Fleet is pre-release alpha software.
Support is best-effort.
Breaking changes are expected.
There are no production support guarantees, response SLAs, paid support terms,
or stable release lines yet.
Source builds and local macOS dogfooding are the intended alpha path.
Public binary distribution and remote/container deployment are not supported
alpha commitments.
Security vulnerabilities should follow SECURITY.md, not public issues.
EOF
  printf 'See SUPPORT.md for the current alpha support boundary.\n' >"$root/README.md"
  printf 'Not supported: production support, stable APIs, or backwards-compatible state formats.\n' \
    >"$root/docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md"
}

expect_pass() {
  local label=$1
  local owner=$2
  local root=$3
  if ! "$ROOT/scripts/check-support-decision.sh" "$owner" "$root/SUPPORT.md" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  local owner=$2
  local root=$3
  if "$ROOT/scripts/check-support-decision.sh" "$owner" "$root/SUPPORT.md" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

owner_best="$TMPDIR/owner-best.md"
best_root="$TMPDIR/best-root"
write_owner_record "$owner_best" APPROVED best
write_best_effort_docs "$best_root"
expect_pass "best-effort alpha support is accepted" "$owner_best" "$best_root"

missing_boundary="$TMPDIR/missing-boundary"
write_best_effort_docs "$missing_boundary"
printf 'Support is best-effort.\n' >"$missing_boundary/SUPPORT.md"
expect_fail "missing no-SLA boundary is rejected" "$owner_best" "$missing_boundary"

owner_target="$TMPDIR/owner-target.md"
target_root="$TMPDIR/target-root"
write_owner_record "$owner_target" APPROVED target
write_best_effort_docs "$target_root"
cat >"$target_root/SUPPORT.md" <<'EOF'
# Support

Support commitment: public alpha triage.
Response target: maintainer triage when capacity allows, normally within one week.
Supported scope: source alpha on local macOS.
EOF
expect_pass "explicit response target is accepted" "$owner_target" "$target_root"

target_placeholder="$TMPDIR/target-placeholder"
write_best_effort_docs "$target_placeholder"
cat >"$target_placeholder/SUPPORT.md" <<'EOF'
Support commitment: TODO
Response target: TODO
Supported scope: TODO
EOF
expect_fail "placeholder response target is rejected" "$owner_target" "$target_placeholder"

owner_other="$TMPDIR/owner-other.md"
other_root="$TMPDIR/other-root"
write_owner_record "$owner_other" APPROVED other
write_best_effort_docs "$other_root"
printf 'Support commitment: private preview only\n' >"$other_root/SUPPORT.md"
expect_pass "concrete Other support policy is accepted" "$owner_other" "$other_root"

owner_pending="$TMPDIR/owner-pending.md"
write_owner_record "$owner_pending" PENDING best
expect_fail "pending owner record is rejected" "$owner_pending" "$best_root"

echo "Support decision check tests passed."
