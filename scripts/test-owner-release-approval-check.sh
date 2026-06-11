#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

sheet="$TMPDIR/OWNER_RELEASE_APPROVAL.md"
cp "$ROOT/docs/release/OWNER_RELEASE_APPROVAL.md" "$sheet"

if ! "$ROOT/scripts/check-owner-release-approval.sh" "$sheet" >"$TMPDIR/pass.out" 2>&1; then
  echo "FAIL: expected owner release approval sheet to pass" >&2
  cat "$TMPDIR/pass.out" >&2
  exit 1
fi

missing_warning="$TMPDIR/missing-warning.md"
cp "$sheet" "$missing_warning"
perl -0pi -e 's/Fleet is still too rough for a broad open-source launch, package announcement,/Fleet is ready for a broad launch,/' "$missing_warning"
if "$ROOT/scripts/check-owner-release-approval.sh" "$missing_warning" >"$TMPDIR/fail.out" 2>&1; then
  echo "FAIL: missing roughness warning should fail" >&2
  cat "$TMPDIR/fail.out" >&2
  exit 1
fi

missing_decision="$TMPDIR/missing-decision.md"
cp "$sheet" "$missing_decision"
perl -0pi -e 's/^\| Workflow supply chain \|.*\n//m' "$missing_decision"
if "$ROOT/scripts/check-owner-release-approval.sh" "$missing_decision" >"$TMPDIR/fail2.out" 2>&1; then
  echo "FAIL: missing workflow supply-chain decision should fail" >&2
  cat "$TMPDIR/fail2.out" >&2
  exit 1
fi

echo "Owner release approval sheet tests passed."
