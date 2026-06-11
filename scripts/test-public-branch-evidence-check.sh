#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

repo="$TMPDIR/repo"
mkdir "$repo"
git -C "$repo" init -q
git -C "$repo" config user.email "release-test@example.invalid"
git -C "$repo" config user.name "Fleet Release Test"

printf '# Fleet fixture\n' >"$repo/README.md"
git -C "$repo" add README.md
git -C "$repo" commit -q -m "clean start"

mkdir -p "$repo/artifacts"
fixture_path="/""Users/history-check/private-project"
printf '{"path":"%s"}\n' "$fixture_path" >"$repo/artifacts/raw.json"
git -C "$repo" add artifacts/raw.json
git -C "$repo" commit -q -m "add private artifact"

rm "$repo/artifacts/raw.json"
rmdir "$repo/artifacts"
printf 'public tree\n' >"$repo/README.md"
git -C "$repo" add -A
git -C "$repo" commit -q -m "remove private artifact"

source_commit="$(git -C "$repo" rev-parse HEAD)"
public_commit="$(git -C "$repo" commit-tree HEAD^{tree} -m "clean public snapshot")"
git -C "$repo" branch public-alpha "$public_commit"

write_owner_record() {
  local file=$1
  local status=$2
  local checked=$3
  cat >"$file" <<EOF
# Owner Decision Record

Decision record status: $status

## Required Before Public GitHub Visibility

### 2. Public History

- [$([ "$checked" = "current" ] && echo x || echo ' ')] Publish the current branch history and accept that old commits may contain
  removed local artifacts or failed eval evidence.
- [$([ "$checked" = "clean" ] && echo x || echo ' ')] Publish a cleaned/squashed history for the first public branch.

### 3. Public Namespace
EOF
}

write_evidence() {
  local file=$1
  local status=$2
  local source=$3
  local branch=$4
  local public=$5
  cat >"$file" <<EOF
# Public Branch Evidence

Public branch evidence status: $status
Source commit: $source
Public branch: $branch
Public root commit: $public
History check command: ./scripts/history-release-check.sh docs/release/OWNER_DECISION_RECORD.md $branch
History check result: PASS

## Required Facts

Single root commit: yes
Public tree matches source commit tree: yes
Public branch contains no prior private history: yes
EOF
}

expect_pass() {
  local label=$1
  local owner=$2
  local evidence=$3
  if ! (cd "$repo" && "$ROOT/scripts/check-public-branch-evidence.sh" "$owner" "$evidence" "$source_commit") >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  local owner=$2
  local evidence=$3
  if (cd "$repo" && "$ROOT/scripts/check-public-branch-evidence.sh" "$owner" "$evidence" "$source_commit") >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

owner_clean="$TMPDIR/owner-clean.md"
evidence_pass="$TMPDIR/public-branch-pass.md"
write_owner_record "$owner_clean" APPROVED clean
write_evidence "$evidence_pass" PASS "$source_commit" public-alpha "$public_commit"
expect_pass "clean public branch evidence is accepted" "$owner_clean" "$evidence_pass"

evidence_pending="$TMPDIR/public-branch-pending.md"
write_evidence "$evidence_pending" PENDING "$source_commit" public-alpha "$public_commit"
expect_fail "pending public branch evidence is rejected" "$owner_clean" "$evidence_pending"

other_commit="0123456789abcdef0123456789abcdef01234567"
evidence_wrong_source="$TMPDIR/public-branch-wrong-source.md"
write_evidence "$evidence_wrong_source" PASS "$other_commit" public-alpha "$public_commit"
expect_fail "wrong source commit is rejected" "$owner_clean" "$evidence_wrong_source"

evidence_private_branch="$TMPDIR/public-branch-private.md"
write_evidence "$evidence_private_branch" PASS "$source_commit" HEAD "$source_commit"
expect_fail "multi-commit private branch is rejected" "$owner_clean" "$evidence_private_branch"

owner_current="$TMPDIR/owner-current.md"
missing_evidence="$TMPDIR/missing.md"
write_owner_record "$owner_current" APPROVED current
expect_pass "current history decision does not require public branch evidence" "$owner_current" "$missing_evidence"

owner_pending="$TMPDIR/owner-pending.md"
write_owner_record "$owner_pending" PENDING clean
expect_fail "pending owner record is rejected" "$owner_pending" "$evidence_pass"

echo "Public branch evidence check tests passed."
