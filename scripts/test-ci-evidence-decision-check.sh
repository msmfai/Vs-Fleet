#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

COMMIT="0123456789abcdef0123456789abcdef01234567"
OTHER_COMMIT="abcdef0123456789abcdef0123456789abcdef01"

write_owner_record() {
  local file=$1
  local status=$2
  local checked=$3
  cat >"$file" <<EOF
# Owner Decision Record

Decision record status: $status

## Required Before Public GitHub Visibility

### 9. Public CI Evidence

- [$([ "$checked" = "github" ] && echo x || echo ' ')] Require GitHub Actions green on the exact branch/commit before public
  visibility.
- [$([ "$checked" = "local" ] && echo x || echo ' ')] Accept local check output only for the first publish.
- [$([ "$checked" = "other" ] && echo x || echo ' ')] Other: \`Use GitLab pipeline evidence\`

### 10. Privacy And Telemetry Posture
EOF
}

write_evidence() {
  local file=$1
  local status=$2
  local commit=$3
  local body=$4
  cat >"$file" <<EOF
# Public CI Evidence

Public CI evidence status: $status
Commit: $commit
$body
EOF
}

expect_pass() {
  local label=$1
  local owner=$2
  local evidence=$3
  if ! "$ROOT/scripts/check-ci-evidence-decision.sh" "$owner" "$evidence" "$COMMIT" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  local owner=$2
  local evidence=$3
  if "$ROOT/scripts/check-ci-evidence-decision.sh" "$owner" "$evidence" "$COMMIT" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

owner_github="$TMPDIR/owner-github.md"
evidence_github="$TMPDIR/evidence-github.md"
write_owner_record "$owner_github" APPROVED github
write_evidence "$evidence_github" PASS "$COMMIT" \
  "Branch: build/fleet-v1
Release Readiness workflow run: https://github.com/example/fleet/actions/runs/123456789"
expect_pass "GitHub Actions evidence is accepted" "$owner_github" "$evidence_github"

evidence_pending="$TMPDIR/evidence-pending.md"
write_evidence "$evidence_pending" PENDING "$COMMIT" \
  "Branch: TODO
Release Readiness workflow run: TODO"
expect_fail "placeholder CI evidence is rejected" "$owner_github" "$evidence_pending"

evidence_wrong_commit="$TMPDIR/evidence-wrong-commit.md"
write_evidence "$evidence_wrong_commit" PASS "$OTHER_COMMIT" \
  "Branch: build/fleet-v1
Release Readiness workflow run: https://github.com/example/fleet/actions/runs/123456789"
expect_fail "wrong commit evidence is rejected" "$owner_github" "$evidence_wrong_commit"

evidence_bad_url="$TMPDIR/evidence-bad-url.md"
write_evidence "$evidence_bad_url" PASS "$COMMIT" \
  "Branch: build/fleet-v1
Release Readiness workflow run: https://gitlab.com/example/fleet/-/pipelines/123"
expect_fail "non-GitHub run URL is rejected for GitHub Actions decision" "$owner_github" "$evidence_bad_url"

owner_local="$TMPDIR/owner-local.md"
evidence_local="$TMPDIR/evidence-local.md"
write_owner_record "$owner_local" APPROVED local
write_evidence "$evidence_local" LOCAL_ONLY "$COMMIT" \
  "Local check transcript: docs/release/local-checks-alpha.1.txt"
expect_pass "local-only evidence is accepted when owner chose it" "$owner_local" "$evidence_local"

owner_other="$TMPDIR/owner-other.md"
evidence_other="$TMPDIR/evidence-other.md"
write_owner_record "$owner_other" APPROVED other
write_evidence "$evidence_other" PASS "$COMMIT" \
  "CI evidence path: https://gitlab.com/example/fleet/-/pipelines/123"
expect_pass "concrete Other CI evidence is accepted" "$owner_other" "$evidence_other"

evidence_other_fail="$TMPDIR/evidence-other-fail.md"
write_evidence "$evidence_other_fail" FAIL "$COMMIT" \
  "CI evidence path: https://gitlab.com/example/fleet/-/pipelines/123"
expect_fail "Other CI evidence still requires passing status" "$owner_other" "$evidence_other_fail"

owner_pending="$TMPDIR/owner-pending.md"
write_owner_record "$owner_pending" PENDING github
expect_fail "pending owner record is rejected" "$owner_pending" "$evidence_github"

echo "Public CI evidence decision check tests passed."
