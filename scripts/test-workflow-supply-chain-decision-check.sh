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

### 22. GitHub Actions Supply-Chain Posture

- [$([ "$checked" = "tagged" ] && echo x || echo ' ')] Tagged third-party GitHub Actions are accepted for source alpha, but workflows must use read-only \`GITHUB_TOKEN\` permissions, no repository secrets, and no package/release publishing credentials.
- [$([ "$checked" = "sha" ] && echo x || echo ' ')] Require every third-party GitHub Action to be pinned by full commit SHA before public visibility.
- [$([ "$checked" = "other" ] && echo x || echo ' ')] Other: \`Private workflow policy\`

## Required Before Binary Distribution
EOF
}

write_workflows() {
  local root=$1
  local checkout_ref=${2:-v4}
  local cache_ref=${3:-v4}
  mkdir -p "$root/.github/workflows" "$root/docs/release"
  cat >"$root/.github/workflows/ci.yml" <<EOF
name: CI
on:
  push:
  pull_request:
permissions:
  contents: read
jobs:
  test:
    steps:
      - uses: actions/checkout@$checkout_ref
      - uses: actions/cache@$cache_ref
      - run: cargo test
EOF
  cat >"$root/.github/workflows/release-readiness.yml" <<EOF
name: Release Readiness
on:
  workflow_dispatch:
permissions:
  contents: read
jobs:
  gate:
    steps:
      - uses: actions/checkout@$checkout_ref
      - run: ./scripts/release-check.sh
EOF
}

write_policy_docs() {
  local root=$1
  mkdir -p "$root/docs/release"
  cat >"$root/docs/release/WORKFLOW_SUPPLY_CHAIN.md" <<'EOF'
# GitHub Actions Supply-Chain Posture

Tagged third-party GitHub Actions are accepted for source alpha.
`GITHUB_TOKEN` permissions are read-only: `contents: read`.
Workflows must not reference repository secrets.
Workflows must not publish packages, create releases, upload release assets,
or push tags.
Future policy can pin by full commit SHA.
EOF
  cat >"$root/docs/release/PUBLIC_ALPHA_DECISIONS.md" <<'EOF'
| GitHub Actions supply-chain posture | Tagged Actions with read-only token. |
EOF
  cat >"$root/docs/release/GITHUB_PUBLICATION_RUNBOOK.md" <<'EOF'
GitHub Actions workflows use the approved supply-chain posture: read-only
`GITHUB_TOKEN` permissions, no repository secrets, and no publishing credentials.
EOF
  cat >"$root/docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" <<'EOF'
# Notes

## Workflow Supply Chain

Source-alpha GitHub Actions use read-only `GITHUB_TOKEN` permissions.
Release-critical workflows do not use repository secrets or publishing
credentials.
EOF
}

expect_pass() {
  local label=$1
  local owner=$2
  local root=$3
  if ! "$ROOT/scripts/check-workflow-supply-chain-decision.sh" "$owner" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  local owner=$2
  local root=$3
  if "$ROOT/scripts/check-workflow-supply-chain-decision.sh" "$owner" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

owner_tagged="$TMPDIR/owner-tagged.md"
tagged_root="$TMPDIR/tagged-root"
write_owner_record "$owner_tagged" APPROVED tagged
write_workflows "$tagged_root"
write_policy_docs "$tagged_root"
expect_pass "tagged Actions with read-only token are accepted" "$owner_tagged" "$tagged_root"

missing_permissions="$TMPDIR/missing-permissions"
write_workflows "$missing_permissions"
write_policy_docs "$missing_permissions"
perl -0pi -e 's/permissions:\n  contents: read\n//g' "$missing_permissions/.github/workflows/ci.yml"
expect_fail "missing read-only permissions are rejected" "$owner_tagged" "$missing_permissions"

secret_reference="$TMPDIR/secret-reference"
write_workflows "$secret_reference"
write_policy_docs "$secret_reference"
printf '      - run: echo "${{ secrets.PUBLISH_TOKEN }}"\n' >>"$secret_reference/.github/workflows/ci.yml"
expect_fail "workflow secret references are rejected" "$owner_tagged" "$secret_reference"

owner_sha="$TMPDIR/owner-sha.md"
sha_root="$TMPDIR/sha-root"
sha1=0123456789abcdef0123456789abcdef01234567
sha2=89abcdef0123456789abcdef0123456789abcdef
write_owner_record "$owner_sha" APPROVED sha
write_workflows "$sha_root" "$sha1" "$sha2"
write_policy_docs "$sha_root"
printf '\nFull SHA pinning: required\n' >>"$sha_root/docs/release/WORKFLOW_SUPPLY_CHAIN.md"
expect_pass "full SHA pinning policy is accepted when all Actions are pinned" "$owner_sha" "$sha_root"

sha_bad="$TMPDIR/sha-bad"
write_workflows "$sha_bad"
write_policy_docs "$sha_bad"
printf '\nFull SHA pinning: required\n' >>"$sha_bad/docs/release/WORKFLOW_SUPPLY_CHAIN.md"
expect_fail "full SHA policy rejects tagged Actions" "$owner_sha" "$sha_bad"

owner_other="$TMPDIR/owner-other.md"
other_root="$TMPDIR/other-root"
write_owner_record "$owner_other" APPROVED other
write_workflows "$other_root"
write_policy_docs "$other_root"
printf 'Owner decision: private workflow policy\n' >"$other_root/docs/release/WORKFLOW_SUPPLY_CHAIN.md"
expect_pass "concrete Other workflow policy is accepted" "$owner_other" "$other_root"

owner_pending="$TMPDIR/owner-pending.md"
write_owner_record "$owner_pending" PENDING tagged
expect_fail "pending owner record is rejected" "$owner_pending" "$tagged_root"

echo "Workflow supply-chain decision check tests passed."
