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

### 19. Public Roadmap And Non-Goals

- [$([ "$checked" = "none" ] && echo x || echo ' ')] No public roadmap commitments during alpha. Issues, labels, and milestones are triage hints only.
- [$([ "$checked" = "roadmap" ] && echo x || echo ' ')] Publish a public roadmap before alpha.
- [$([ "$checked" = "other" ] && echo x || echo ' ')] Other: \`Private roadmap only\`

## Required Before Binary Distribution
EOF
}

write_no_roadmap_docs() {
  local root=$1
  mkdir -p "$root/docs/release" "$root/.github/ISSUE_TEMPLATE"
  printf 'No public roadmap commitments are made during alpha.\n' >"$root/README.md"
  cat >"$root/docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" <<'EOF'
# Notes

## Roadmap And Non-Goals

- No public roadmap commitments are made during alpha.
- Issues, labels, and milestones are triage hints, not delivery promises.
- Remote/container workflows, binary packages, stable APIs, and production support remain non-goals unless a later owner decision approves them.
EOF
  cat >"$root/.github/ISSUE_TEMPLATE/alpha_feedback.yml" <<'EOF'
body:
  - type: markdown
    attributes:
      value: Feedback and suggestions are not roadmap commitments.
EOF
  printf '| Public roadmap | Current docs avoid roadmap commitments. |\n' \
    >"$root/docs/release/PUBLIC_ALPHA_DECISIONS.md"
}

expect_pass() {
  local label=$1
  local owner=$2
  local root=$3
  if ! "$ROOT/scripts/check-roadmap-decision.sh" "$owner" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  local owner=$2
  local root=$3
  if "$ROOT/scripts/check-roadmap-decision.sh" "$owner" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

owner_none="$TMPDIR/owner-none.md"
none_root="$TMPDIR/none-root"
write_owner_record "$owner_none" APPROVED none
write_no_roadmap_docs "$none_root"
expect_pass "no-roadmap alpha docs are accepted" "$owner_none" "$none_root"

missing_issue_boundary="$TMPDIR/missing-issue-boundary"
write_no_roadmap_docs "$missing_issue_boundary"
perl -0pi -e 's/- Issues, labels, and milestones are triage hints, not delivery promises\.\n//' \
  "$missing_issue_boundary/docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md"
expect_fail "missing issue/milestone non-commitment is rejected" "$owner_none" "$missing_issue_boundary"

owner_roadmap="$TMPDIR/owner-roadmap.md"
roadmap_root="$TMPDIR/roadmap-root"
write_owner_record "$owner_roadmap" APPROVED roadmap
write_no_roadmap_docs "$roadmap_root"
cat >"$roadmap_root/docs/release/ROADMAP.md" <<'EOF'
# Roadmap

Roadmap commitment: public alpha roadmap is maintained in this file.
Non-goals: production support and binary packages remain out of scope.
Change process: owner updates this file before changing public commitments.
EOF
expect_pass "concrete public roadmap is accepted" "$owner_roadmap" "$roadmap_root"

roadmap_placeholder="$TMPDIR/roadmap-placeholder"
write_no_roadmap_docs "$roadmap_placeholder"
cat >"$roadmap_placeholder/docs/release/ROADMAP.md" <<'EOF'
Roadmap commitment: TODO
Non-goals: TODO
Change process: TODO
EOF
expect_fail "placeholder public roadmap is rejected" "$owner_roadmap" "$roadmap_placeholder"

owner_other="$TMPDIR/owner-other.md"
other_root="$TMPDIR/other-root"
write_owner_record "$owner_other" APPROVED other
write_no_roadmap_docs "$other_root"
printf 'Roadmap commitment: private roadmap only\n' >"$other_root/docs/release/ROADMAP.md"
expect_pass "concrete Other roadmap policy is accepted" "$owner_other" "$other_root"

owner_pending="$TMPDIR/owner-pending.md"
write_owner_record "$owner_pending" PENDING none
expect_fail "pending owner record is rejected" "$owner_pending" "$none_root"

echo "Roadmap decision check tests passed."
