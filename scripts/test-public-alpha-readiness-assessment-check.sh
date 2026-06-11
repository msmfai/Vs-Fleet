#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

valid="$ROOT/docs/release/PUBLIC_ALPHA_READINESS_ASSESSMENT.md"

expect_pass() {
  local label=$1
  local file=$2
  if ! "$ROOT/scripts/check-public-alpha-readiness-assessment.sh" "$file" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  local file=$2
  if "$ROOT/scripts/check-public-alpha-readiness-assessment.sh" "$file" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_output() {
  local pattern=$1
  if ! rg -q "$pattern" "$TMPDIR/out"; then
    echo "FAIL: expected checker output to contain: $pattern" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_pass "repository assessment" "$valid"

missing_verdict="$TMPDIR/missing-verdict.md"
cp "$valid" "$missing_verdict"
perl -0pi -e 's/Current verdict: GATED FOR PUBLIC SOURCE ALPHA\./Current verdict: APPROVED./' "$missing_verdict"
expect_fail "missing gated verdict" "$missing_verdict"
expect_output "current gated source-alpha verdict"

missing_remote_boundary="$TMPDIR/missing-remote-boundary.md"
cp "$valid" "$missing_remote_boundary"
perl -0pi -e 's/- "Fleet supports remote machines, containers, or SSH workflows\."\n//' "$missing_remote_boundary"
expect_fail "missing remote/container non-goal" "$missing_remote_boundary"
expect_output "remote/container non-commitment"

placeholder="$TMPDIR/placeholder.md"
cp "$valid" "$placeholder"
printf '\nTODO: fill later\n' >>"$placeholder"
expect_fail "placeholder is rejected" "$placeholder"
expect_output "unresolved placeholders"

missing_evidence_gate="$TMPDIR/missing-evidence-gate.md"
cp "$valid" "$missing_evidence_gate"
perl -0pi -e 's/3\. `\.\/scripts\/release-evidence-status\.sh` reports release evidence complete\.\n//' "$missing_evidence_gate"
expect_fail "missing release evidence decision rule" "$missing_evidence_gate"
expect_output "release evidence decision rule"

echo "Public alpha readiness assessment tests passed."
