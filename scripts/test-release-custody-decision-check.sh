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

### 16. Release Custody And Maintainer Authority

- [$([ "$checked" = "single" ] && echo x || echo ' ')] Single-maintainer alpha. Only the repository owner or named maintainer may push release tags, create GitHub releases, change repository settings, or publish packages.
- [$([ "$checked" = "multi" ] && echo x || echo ' ')] Multi-maintainer governance before public alpha.
- [$([ "$checked" = "other" ] && echo x || echo ' ')] Other: \`Private preview release authority\`

## Required Before Binary Distribution
EOF
}

write_evidence() {
  local file=$1
  cat >"$file" <<'EOF'
# GitHub Publication Evidence

## Release Custody

Release authority: single maintainer repository owner
Tag protection or accepted unavailable reason: enabled
Release artifact custody: source tags and release notes only
Package publishing credentials: none for source-only alpha
Emergency removal owner: repository owner
EOF
}

write_runbook() {
  local root=$1
  mkdir -p "$root/docs/release"
  cat >"$root/docs/release/GITHUB_PUBLICATION_RUNBOOK.md" <<'EOF'
# GitHub Publication Runbook

## Release Custody

Only the approved release authority may push source tags or create GitHub releases.
EOF
}

expect_pass() {
  local label=$1
  local owner=$2
  local evidence=$3
  local root=$4
  if ! "$ROOT/scripts/check-release-custody-decision.sh" "$owner" "$evidence" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  local owner=$2
  local evidence=$3
  local root=$4
  if "$ROOT/scripts/check-release-custody-decision.sh" "$owner" "$evidence" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

owner_single="$TMPDIR/owner-single.md"
evidence="$TMPDIR/evidence.md"
root="$TMPDIR/root"
write_owner_record "$owner_single" APPROVED single
write_evidence "$evidence"
write_runbook "$root"
expect_pass "single-maintainer custody evidence is accepted" "$owner_single" "$evidence" "$root"

placeholder_evidence="$TMPDIR/placeholder-evidence.md"
write_evidence "$placeholder_evidence"
perl -0pi -e 's/Package publishing credentials: none for source-only alpha/Package publishing credentials: TODO/' \
  "$placeholder_evidence"
expect_fail "placeholder custody evidence is rejected" "$owner_single" "$placeholder_evidence" "$root"

missing_runbook="$TMPDIR/missing-runbook"
mkdir -p "$missing_runbook/docs/release"
printf '# Runbook\n' >"$missing_runbook/docs/release/GITHUB_PUBLICATION_RUNBOOK.md"
expect_fail "missing release custody runbook is rejected" "$owner_single" "$evidence" "$missing_runbook"

owner_multi="$TMPDIR/owner-multi.md"
multi_root="$TMPDIR/multi-root"
write_owner_record "$owner_multi" APPROVED multi
write_runbook "$multi_root"
cat >"$multi_root/docs/release/MAINTAINERS.md" <<'EOF'
# Maintainers

Repository admins: maintainer@example.invalid
Release approvers: maintainer@example.invalid
Package publishers: none for source-only alpha
Emergency removal owner: maintainer@example.invalid
EOF
expect_pass "multi-maintainer governance file is accepted" "$owner_multi" "$evidence" "$multi_root"

multi_placeholder="$TMPDIR/multi-placeholder"
write_runbook "$multi_placeholder"
cat >"$multi_placeholder/docs/release/MAINTAINERS.md" <<'EOF'
Repository admins: TODO
Release approvers: TODO
Package publishers: TODO
Emergency removal owner: TODO
EOF
expect_fail "placeholder maintainer governance is rejected" "$owner_multi" "$evidence" "$multi_placeholder"

owner_other="$TMPDIR/owner-other.md"
other_root="$TMPDIR/other-root"
write_owner_record "$owner_other" APPROVED other
write_runbook "$other_root"
printf 'Release custody commitment: private preview release authority only\n' \
  >"$other_root/docs/release/MAINTAINERS.md"
expect_pass "concrete Other release custody is accepted" "$owner_other" "$evidence" "$other_root"

owner_pending="$TMPDIR/owner-pending.md"
write_owner_record "$owner_pending" PENDING single
expect_fail "pending owner record is rejected" "$owner_pending" "$evidence" "$root"

echo "Release custody decision check tests passed."
