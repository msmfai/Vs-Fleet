#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

COMMIT="0123456789abcdef0123456789abcdef01234567"

write_owner_record() {
  local file=$1
  local status=$2
  cat >"$file" <<EOF
# Owner Decision Record

Decision record status: $status

## Required Before Public GitHub Visibility

### 3. Public Namespace

| Surface | Decision |
|---|---|
| GitHub org/user | smfmarin |
| GitHub repo name | vs-fleet |
| Product name | Fleet |
| Rust crate prefix | fleet-* |
| npm package names | fleet-extension, fleet-bridge |
| VS Code Marketplace publisher | fleet-team |
| Open VSX publisher | fleet-team |
| macOS bundle id | dev.fleet.host |

### 4. Alpha Scope
EOF
}

write_evidence() {
  local file=$1
  local status=${2:-PASS}
  local repo=${3:-"https://github.com/smfmarin/vs-fleet"}
  local protection=${4:-"enabled"}
  local commit=${5:-"$COMMIT"}
  cat >"$file" <<EOF
# GitHub Publication Evidence

GitHub publication evidence status: $status

Commit: $commit
Release-control evidence file: docs/release/GITHUB_PUBLICATION_EVIDENCE.md
Repository: $repo
Default branch: public-alpha

## Visibility And Repository Settings

Visibility consequences reviewed: yes
Repository name matches namespace: yes
Issues setting: enabled per support commitment
Discussions setting: disabled
Wiki setting: disabled
Releases setting: source tags and release notes only
Packages setting: not used for source-only alpha
GitHub Actions setting: enabled

## Security Settings

Security reporting channel available: GitHub Private Vulnerability Reporting enabled
Secret scanning or accepted unavailable reason: enabled
Dependabot alerts or accepted unavailable reason: enabled

## Branch Protection

Default branch protection: $protection
Required source checks: CI source checks
Required release checks: Release Readiness
Linear history policy: preferred
Signed commit policy: not required
EOF
}

expect_pass() {
  local label=$1
  local owner=$2
  local evidence=$3
  if ! "$ROOT/scripts/check-github-publication-evidence.sh" "$owner" "$evidence" "$COMMIT" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  local owner=$2
  local evidence=$3
  if "$ROOT/scripts/check-github-publication-evidence.sh" "$owner" "$evidence" "$COMMIT" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

owner="$TMPDIR/owner.md"
evidence="$TMPDIR/evidence.md"
write_owner_record "$owner" APPROVED
write_evidence "$evidence"
expect_pass "complete GitHub publication evidence is accepted" "$owner" "$evidence"

deferred="$TMPDIR/deferred.md"
write_evidence "$deferred" PASS "https://github.com/smfmarin/vs-fleet" \
  "owner-approved deferred: branch protection will be enabled immediately after visibility flip"
expect_pass "owner-approved deferred branch protection rationale is accepted" "$owner" "$deferred"

pending="$TMPDIR/pending.md"
write_evidence "$pending" PENDING
expect_fail "pending evidence is rejected" "$owner" "$pending"

wrong_repo="$TMPDIR/wrong-repo.md"
write_evidence "$wrong_repo" PASS "https://github.com/other/vs-fleet"
expect_fail "repository URL must match namespace" "$owner" "$wrong_repo"

no_protection="$TMPDIR/no-protection.md"
write_evidence "$no_protection" PASS "https://github.com/smfmarin/vs-fleet" "none"
expect_fail "missing branch protection is rejected" "$owner" "$no_protection"

placeholder="$TMPDIR/placeholder.md"
write_evidence "$placeholder"
perl -0pi -e 's/Secret scanning or accepted unavailable reason: enabled/Secret scanning or accepted unavailable reason: TODO/' "$placeholder"
expect_fail "placeholder settings are rejected" "$owner" "$placeholder"

owner_pending="$TMPDIR/owner-pending.md"
write_owner_record "$owner_pending" PENDING
expect_fail "pending owner record is rejected" "$owner_pending" "$evidence"

todo_namespace="$TMPDIR/todo-namespace.md"
write_owner_record "$todo_namespace" APPROVED
perl -0pi -e 's/\| GitHub repo name \| vs-fleet \|/| GitHub repo name | `TODO` |/' "$todo_namespace"
expect_fail "namespace placeholders are rejected" "$todo_namespace" "$evidence"

repo="$TMPDIR/repo"
mkdir -p "$repo/docs/release"
git -C "$repo" init -q
git -C "$repo" config user.email "release-test@example.invalid"
git -C "$repo" config user.name "Fleet Release Test"

printf '# Fleet fixture\n' >"$repo/README.md"
write_owner_record "$repo/docs/release/OWNER_DECISION_RECORD.md" APPROVED
git -C "$repo" add .
git -C "$repo" commit -q -m "reviewed publication source"
reviewed_commit="$(git -C "$repo" rev-parse HEAD)"

write_evidence "$repo/docs/release/GITHUB_PUBLICATION_EVIDENCE.md" PASS \
  "https://github.com/smfmarin/vs-fleet" "enabled" "$reviewed_commit"

git -C "$repo" add docs/release/GITHUB_PUBLICATION_EVIDENCE.md
git -C "$repo" commit -q -m "record GitHub publication evidence"
evidence_commit="$(git -C "$repo" rev-parse HEAD)"

if ! (cd "$repo" && "$ROOT/scripts/check-github-publication-evidence.sh" \
  docs/release/OWNER_DECISION_RECORD.md \
  docs/release/GITHUB_PUBLICATION_EVIDENCE.md \
  "$evidence_commit") >"$TMPDIR/publication-evidence-commit.out" 2>&1; then
  echo "FAIL: publication evidence commit should pass when only the publication evidence file differs" >&2
  cat "$TMPDIR/publication-evidence-commit.out" >&2
  exit 1
fi

cat >"$repo/docs/release/PUBLIC_CI_EVIDENCE.md" <<'EOF'
# Public CI Evidence
Public CI evidence status: PASS
EOF
git -C "$repo" add docs/release/PUBLIC_CI_EVIDENCE.md
git -C "$repo" commit -q -m "record another release-control evidence file"
other_evidence_commit="$(git -C "$repo" rev-parse HEAD)"

if ! (cd "$repo" && "$ROOT/scripts/check-github-publication-evidence.sh" \
  docs/release/OWNER_DECISION_RECORD.md \
  docs/release/GITHUB_PUBLICATION_EVIDENCE.md \
  "$other_evidence_commit") >"$TMPDIR/publication-other-evidence-commit.out" 2>&1; then
  echo "FAIL: publication evidence should allow other release-control evidence files to differ" >&2
  cat "$TMPDIR/publication-other-evidence-commit.out" >&2
  exit 1
fi

printf 'unexpected publication-reviewed payload drift\n' >"$repo/README.md"
git -C "$repo" add README.md
git -C "$repo" commit -q -m "drift publication payload"
drift_commit="$(git -C "$repo" rev-parse HEAD)"

if (cd "$repo" && "$ROOT/scripts/check-github-publication-evidence.sh" \
  docs/release/OWNER_DECISION_RECORD.md \
  docs/release/GITHUB_PUBLICATION_EVIDENCE.md \
  "$drift_commit") >"$TMPDIR/publication-evidence-drift.out" 2>&1; then
  echo "FAIL: publication evidence check should reject payload drift outside the evidence file" >&2
  cat "$TMPDIR/publication-evidence-drift.out" >&2
  exit 1
fi

echo "GitHub publication evidence check tests passed."
