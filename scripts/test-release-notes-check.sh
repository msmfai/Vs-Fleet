#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT
COMMIT="0123456789abcdef0123456789abcdef01234567"
OTHER_COMMIT="abcdef0123456789abcdef0123456789abcdef01"

write_valid_notes() {
  local file=$1
  cat >"$file" <<'EOF'
# Fleet v0.1.0-alpha.1

## Release

- Version: v0.1.0-alpha.1
- Commit: 0123456789abcdef0123456789abcdef01234567
- Date: 2026-06-11
- Distribution: source-only
- Owner decision record: docs/release/OWNER_DECISION_RECORD.md at this commit

## Alpha Scope

This alpha is intended for local macOS source builds and local code serve-web
sessions.

## What Changed

- Added release readiness gates.

## Verification

- GitHub CI on exact commit: https://example.invalid/actions/runs/1
- Release readiness workflow: https://example.invalid/actions/runs/2
- Rust workspace checks: passed locally.
- Fleet host checks: passed locally.
- JavaScript/package checks: passed locally.
- Dependency review: completed, no accepted findings.
- History exposure audit: cleaned public history.
- Secret exposure audit: passed.
- Release hygiene gate: passed.

## Dependency And License Review

- Project license: MIT OR Apache-2.0
- Third-party dependency review date: 2026-06-11
- Accepted advisory/license findings: none
- Package publication: none for source-only alpha

## Security And Privacy Notes

- Vulnerability reporting path: GitHub Private Vulnerability Reporting.

## Known Rough Edges

- Remote deployment is not supported as a public alpha path.

## Upgrade And Rollback

- No stable upgrade path is promised during alpha.
EOF
}

expect_pass() {
  local label=$1
  local file=$2
  shift 2
  if ! "$ROOT/scripts/check-release-notes.sh" "$file" "$@" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  local file=$2
  shift 2
  if "$ROOT/scripts/check-release-notes.sh" "$file" "$@" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

valid="$TMPDIR/valid.md"
write_valid_notes "$valid"
expect_pass "filled release notes" "$valid" "$COMMIT"

placeholder="$TMPDIR/placeholder.md"
write_valid_notes "$placeholder"
printf '\n- Commit: `[full commit SHA]`\n' >>"$placeholder"
expect_fail "placeholder is rejected" "$placeholder"

wrong_commit="$TMPDIR/wrong-commit.md"
write_valid_notes "$wrong_commit"
expect_fail "wrong expected commit is rejected" "$wrong_commit" "$OTHER_COMMIT"

bad_commit="$TMPDIR/bad-commit.md"
write_valid_notes "$bad_commit"
perl -0pi -e 's/Commit: 0123456789abcdef0123456789abcdef01234567/Commit: short-sha/' "$bad_commit"
expect_fail "malformed commit is rejected" "$bad_commit"

bad_date="$TMPDIR/bad-date.md"
write_valid_notes "$bad_date"
perl -0pi -e 's/Date: 2026-06-11/Date: June 11 2026/' "$bad_date"
expect_fail "malformed date is rejected" "$bad_date"

bad_version="$TMPDIR/bad-version.md"
write_valid_notes "$bad_version"
perl -0pi -e 's/Version: v0.1.0-alpha.1/Version: alpha one/' "$bad_version"
expect_fail "malformed version is rejected" "$bad_version"

choice="$TMPDIR/choice.md"
write_valid_notes "$choice"
printf '\n- Distribution: `[source-only | source plus approved binary scope]`\n' >>"$choice"
expect_fail "unresolved choice list is rejected" "$choice"

missing="$TMPDIR/missing.md"
write_valid_notes "$missing"
perl -0pi -e 's/\n## Verification\n/\n/' "$missing"
expect_fail "missing required section is rejected" "$missing"

missing_secret="$TMPDIR/missing-secret.md"
write_valid_notes "$missing_secret"
perl -0pi -e 's/\n- Secret exposure audit: passed\.\n/\n/' "$missing_secret"
expect_fail "missing secret exposure audit is rejected" "$missing_secret"

exception="$TMPDIR/exception.md"
write_valid_notes "$exception"
printf '\n- Dependency review: owner-approved skip\n' >>"$exception"
expect_fail "exception shorthand is rejected" "$exception"

echo "Release notes check tests passed."
