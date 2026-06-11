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

### 11. Dependency Review Evidence

- [$([ "$checked" = "run" ] && echo x || echo ' ')] Run the dependency review commands in \`docs/release/DEPENDENCY_REVIEW.md\`
  and record findings in the release notes.
- [$([ "$checked" = "skip" ] && echo x || echo ' ')] Publish the first source alpha without dependency review and accept
  advisory/license review risk.
- [$([ "$checked" = "other" ] && echo x || echo ' ')] Other: \`External dependency review report\`

### 12. Support Commitment

- [x] Best-effort alpha support only. Breaking changes are expected; there are
  no production support guarantees, response SLAs, paid support terms, or stable
  release lines.
- [ ] Define a public triage or response target in \`SUPPORT.md\`.
- [ ] Other: \`TODO\`

## Required Before Binary Distribution
EOF
}

write_evidence() {
  local file=$1
  local status=$2
  local commit=$3
  local body=$4
  cat >"$file" <<EOF
# Dependency Review Evidence

Dependency review status: $status
Commit: $commit
$body
EOF
}

expect_pass() {
  local label=$1
  local owner=$2
  local evidence=$3
  if ! "$ROOT/scripts/check-dependency-review-decision.sh" "$owner" "$evidence" "$COMMIT" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  local owner=$2
  local evidence=$3
  if "$ROOT/scripts/check-dependency-review-decision.sh" "$owner" "$evidence" "$COMMIT" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

owner_run="$TMPDIR/owner-run.md"
evidence_run="$TMPDIR/evidence-run.md"
write_owner_record "$owner_run" APPROVED run
write_evidence "$evidence_run" PASS "$COMMIT" \
  "Reviewed date: 2026-06-11
cargo tree: pass
cargo metadata --locked: pass
fleet-host cargo metadata --locked: pass
lockfile policy: pass
fleet-bridge npm audit: pass
extension npm audit: pass
generated artifact check: pass
Accepted findings: none"
expect_pass "full dependency review evidence is accepted" "$owner_run" "$evidence_run"

evidence_pending="$TMPDIR/evidence-pending.md"
write_evidence "$evidence_pending" PENDING "$COMMIT" \
  "Reviewed date: TODO
cargo tree: TODO"
expect_fail "placeholder dependency evidence is rejected" "$owner_run" "$evidence_pending"

evidence_wrong_commit="$TMPDIR/evidence-wrong-commit.md"
write_evidence "$evidence_wrong_commit" PASS "$OTHER_COMMIT" \
  "Reviewed date: 2026-06-11
cargo tree: pass
cargo metadata --locked: pass
fleet-bridge npm audit: pass
extension npm audit: pass
generated artifact check: pass
Accepted findings: none"
expect_fail "wrong commit evidence is rejected" "$owner_run" "$evidence_wrong_commit"

evidence_missing_audit="$TMPDIR/evidence-missing-audit.md"
write_evidence "$evidence_missing_audit" PASS "$COMMIT" \
  "Reviewed date: 2026-06-11
cargo tree: pass
cargo metadata --locked: pass
fleet-bridge npm audit: pass
generated artifact check: pass
Accepted findings: none"
expect_fail "missing package audit evidence is rejected" "$owner_run" "$evidence_missing_audit"

owner_skip="$TMPDIR/owner-skip.md"
evidence_skip="$TMPDIR/evidence-skip.md"
write_owner_record "$owner_skip" APPROVED skip
write_evidence "$evidence_skip" SKIPPED_ACCEPTED_RISK "$COMMIT" \
  "Accepted risk: first alpha is invite-only and dependency review will run before broad announcement"
expect_pass "explicit skipped-review risk is accepted" "$owner_skip" "$evidence_skip"

owner_other="$TMPDIR/owner-other.md"
evidence_other="$TMPDIR/evidence-other.md"
write_owner_record "$owner_other" APPROVED other
write_evidence "$evidence_other" PASS "$COMMIT" \
  "Dependency review evidence path: docs/release/external-dependency-review-alpha.1.md"
expect_pass "concrete Other dependency review evidence is accepted" "$owner_other" "$evidence_other"

owner_pending="$TMPDIR/owner-pending.md"
write_owner_record "$owner_pending" PENDING run
expect_fail "pending owner record is rejected" "$owner_pending" "$evidence_run"

repo="$TMPDIR/repo"
mkdir -p "$repo/docs/release"
git -C "$repo" init -q
git -C "$repo" config user.email "release-test@example.invalid"
git -C "$repo" config user.name "Fleet Release Test"

printf '# Fleet fixture\n' >"$repo/README.md"
write_owner_record "$repo/docs/release/OWNER_DECISION_RECORD.md" APPROVED run
git -C "$repo" add .
git -C "$repo" commit -q -m "reviewed source"
reviewed_commit="$(git -C "$repo" rev-parse HEAD)"

write_evidence "$repo/docs/release/DEPENDENCY_REVIEW_EVIDENCE.md" PASS "$reviewed_commit" \
  "Reviewed date: 2026-06-11
Release-control evidence file: docs/release/DEPENDENCY_REVIEW_EVIDENCE.md
cargo tree: pass
cargo metadata --locked: pass
fleet-host cargo metadata --locked: pass
lockfile policy: pass
fleet-bridge npm audit: pass
extension npm audit: pass
generated artifact check: pass
Accepted findings: none"

git -C "$repo" add docs/release/DEPENDENCY_REVIEW_EVIDENCE.md
git -C "$repo" commit -q -m "record dependency review evidence"
evidence_commit="$(git -C "$repo" rev-parse HEAD)"

if ! (cd "$repo" && "$ROOT/scripts/check-dependency-review-decision.sh" \
  docs/release/OWNER_DECISION_RECORD.md \
  docs/release/DEPENDENCY_REVIEW_EVIDENCE.md \
  "$evidence_commit") >"$TMPDIR/evidence-commit.out" 2>&1; then
  echo "FAIL: evidence commit should pass when only the dependency evidence file differs" >&2
  cat "$TMPDIR/evidence-commit.out" >&2
  exit 1
fi

cat >"$repo/docs/release/PUBLIC_BRANCH_EVIDENCE.md" <<'EOF'
# Public Branch Evidence
Public branch evidence status: PASS
EOF
git -C "$repo" add docs/release/PUBLIC_BRANCH_EVIDENCE.md
git -C "$repo" commit -q -m "record another release-control evidence file"
other_evidence_commit="$(git -C "$repo" rev-parse HEAD)"

if ! (cd "$repo" && "$ROOT/scripts/check-dependency-review-decision.sh" \
  docs/release/OWNER_DECISION_RECORD.md \
  docs/release/DEPENDENCY_REVIEW_EVIDENCE.md \
  "$other_evidence_commit") >"$TMPDIR/other-evidence-commit.out" 2>&1; then
  echo "FAIL: dependency evidence should allow other release-control evidence files to differ" >&2
  cat "$TMPDIR/other-evidence-commit.out" >&2
  exit 1
fi

printf 'unexpected dependency-reviewed payload drift\n' >"$repo/README.md"
git -C "$repo" add README.md
git -C "$repo" commit -q -m "drift reviewed payload"
drift_commit="$(git -C "$repo" rev-parse HEAD)"

if (cd "$repo" && "$ROOT/scripts/check-dependency-review-decision.sh" \
  docs/release/OWNER_DECISION_RECORD.md \
  docs/release/DEPENDENCY_REVIEW_EVIDENCE.md \
  "$drift_commit") >"$TMPDIR/evidence-drift.out" 2>&1; then
  echo "FAIL: evidence check should reject reviewed payload drift outside the evidence file" >&2
  cat "$TMPDIR/evidence-drift.out" >&2
  exit 1
fi

echo "Dependency review decision check tests passed."
