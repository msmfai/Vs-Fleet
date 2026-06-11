#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

write_ci() {
  local file=$1
  cat >"$file" <<'EOF'
name: CI

on:
  push:
  pull_request:

jobs:
  rust:
    steps:
      - run: cargo fmt --all -- --check
      - run: cargo clippy --workspace --all-targets --all-features -- -D warnings
      - run: cargo test --workspace --all-targets --all-features
  coverage:
    steps:
      - run: cargo llvm-cov report --fail-under-lines 80
      - run: cargo llvm-cov report --package fleet-protocol --package fleet-hub --fail-under-lines 85
  pnpm:
    steps:
      - run: pnpm -r build
      - run: pnpm -r test
EOF
}

write_release() {
  local file=$1
  cat >"$file" <<'EOF'
name: Release Readiness

on:
  workflow_dispatch:

jobs:
  release-gate:
    steps:
      - run: ./scripts/test-release-check.sh
      - run: ./scripts/test-dependabot-config-check.sh
      - run: ./scripts/test-secret-release-check.sh
      - run: ./scripts/test-doc-link-check.sh
      - run: ./scripts/test-public-tree-size-check.sh
      - run: ./scripts/check-owner-decisions.sh docs/release/OWNER_DECISION_RECORD.md
      - run: ./scripts/history-release-check.sh docs/release/OWNER_DECISION_RECORD.md
      - run: ./scripts/secret-release-check.sh
      - run: ./scripts/check-doc-links.sh
      - run: ./scripts/check-public-tree-size.sh
      - run: ./scripts/release-check.sh
  source-checks:
    steps:
      - run: cargo clippy --workspace --all-targets --all-features -- -D warnings
      - run: cargo test --workspace --all-targets --all-features
      - run: |
          cd crates/fleet-host
          ./bundle.sh release
      - run: npm run build
      - run: npm test
EOF
}

expect_pass() {
  local label=$1
  local ci=$2
  local release=$3
  if ! "$ROOT/scripts/check-github-workflows.sh" "$ci" "$release" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  local ci=$2
  local release=$3
  if "$ROOT/scripts/check-github-workflows.sh" "$ci" "$release" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

ci="$TMPDIR/ci.yml"
release="$TMPDIR/release-readiness.yml"
write_ci "$ci"
write_release "$release"
expect_pass "required workflows are accepted" "$ci" "$release"

missing_ci="$TMPDIR/missing-ci.yml"
expect_fail "missing CI workflow is rejected" "$missing_ci" "$release"

no_pnpm_test="$TMPDIR/no-pnpm-test.yml"
write_ci "$no_pnpm_test"
perl -0pi -e 's/\n      - run: pnpm -r test\n/\n/' "$no_pnpm_test"
expect_fail "CI workflow must keep package tests" "$no_pnpm_test" "$release"

no_dispatch="$TMPDIR/no-dispatch.yml"
write_release "$no_dispatch"
perl -0pi -e 's/workflow_dispatch:/push:/' "$no_dispatch"
expect_fail "Release Readiness must stay manually dispatchable" "$ci" "$no_dispatch"

no_bundle="$TMPDIR/no-bundle.yml"
write_release "$no_bundle"
perl -0pi -e 's/\n          \.\/bundle\.sh release\n/\n/' "$no_bundle"
expect_fail "Release Readiness must keep host bundle verification" "$ci" "$no_bundle"

no_secret_gate="$TMPDIR/no-secret-gate.yml"
write_release "$no_secret_gate"
perl -0pi -e 's/\n      - run: \.\/scripts\/test-secret-release-check\.sh\n//; s/\n      - run: \.\/scripts\/secret-release-check\.sh\n//' "$no_secret_gate"
expect_fail "Release Readiness must keep secret exposure checks" "$ci" "$no_secret_gate"

no_doc_links="$TMPDIR/no-doc-links.yml"
write_release "$no_doc_links"
perl -0pi -e 's/\n      - run: \.\/scripts\/test-doc-link-check\.sh\n//; s/\n      - run: \.\/scripts\/check-doc-links\.sh\n//' "$no_doc_links"
expect_fail "Release Readiness must keep documentation link checks" "$ci" "$no_doc_links"

no_tree_size="$TMPDIR/no-tree-size.yml"
write_release "$no_tree_size"
perl -0pi -e 's/\n      - run: \.\/scripts\/test-public-tree-size-check\.sh\n//; s/\n      - run: \.\/scripts\/check-public-tree-size\.sh\n//' "$no_tree_size"
expect_fail "Release Readiness must keep public tree size checks" "$ci" "$no_tree_size"

echo "GitHub workflow check tests passed."
