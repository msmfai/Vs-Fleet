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
      - uses: actions/cache@v4
        with:
          key: ${{ runner.os }}-cargo-${{ hashFiles('**/Cargo.toml', '**/Cargo.lock') }}
      - run: cargo fmt --all -- --check
      - run: cargo clippy --workspace --all-targets --all-features -- -D warnings
      - run: cargo test --workspace --all-targets --all-features
  coverage:
    steps:
      - uses: actions/cache@v4
        with:
          key: ${{ runner.os }}-cov-${{ hashFiles('**/Cargo.toml', '**/Cargo.lock') }}
      - run: cargo llvm-cov report --fail-under-lines 80
      - run: cargo llvm-cov report --package fleet-protocol --package fleet-hub --fail-under-lines 85
  pnpm:
    steps:
      - uses: actions/cache@v4
        with:
          key: ${{ runner.os }}-pnpm-${{ hashFiles('**/package.json', 'pnpm-lock.yaml') }}
      - run: pnpm install --frozen-lockfile --ignore-scripts
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
      - run: ./scripts/test-lockfile-policy-check.sh
      - run: ./scripts/test-branding-decision-check.sh
      - run: ./scripts/test-github-publication-evidence-check.sh
      - run: ./scripts/test-dependency-review-runner.sh
      - run: ./scripts/check-owner-decisions.sh docs/release/OWNER_DECISION_RECORD.md
      - run: ./scripts/history-release-check.sh docs/release/OWNER_DECISION_RECORD.md
      - run: ./scripts/secret-release-check.sh
      - run: ./scripts/check-doc-links.sh
      - run: ./scripts/check-public-tree-size.sh
      - run: ./scripts/check-lockfile-policy.sh
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

no_pnpm_lock="$TMPDIR/no-pnpm-lock.yml"
write_ci "$no_pnpm_lock"
perl -0pi -e "s/, 'pnpm-lock\\.yaml'//" "$no_pnpm_lock"
expect_fail "CI workflow must hash pnpm lockfile" "$no_pnpm_lock" "$release"

pnpm_fallback="$TMPDIR/pnpm-fallback.yml"
write_ci "$pnpm_fallback"
perl -0pi -e 's/pnpm install --frozen-lockfile --ignore-scripts/pnpm install --frozen-lockfile --ignore-scripts || pnpm install --ignore-scripts/' "$pnpm_fallback"
expect_fail "CI workflow must not fall back from frozen pnpm install" "$pnpm_fallback" "$release"

no_cargo_lock="$TMPDIR/no-cargo-lock.yml"
write_ci "$no_cargo_lock"
perl -0pi -e "s/, '\\*\\*\\/Cargo\\.lock'//g" "$no_cargo_lock"
expect_fail "CI workflow must hash Cargo lockfiles" "$no_cargo_lock" "$release"

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

no_lockfile_gate="$TMPDIR/no-lockfile-gate.yml"
write_release "$no_lockfile_gate"
perl -0pi -e 's/\n      - run: \.\/scripts\/test-lockfile-policy-check\.sh\n//; s/\n      - run: \.\/scripts\/check-lockfile-policy\.sh\n//' "$no_lockfile_gate"
expect_fail "Release Readiness must keep lockfile policy checks" "$ci" "$no_lockfile_gate"

no_dependency_runner="$TMPDIR/no-dependency-runner.yml"
write_release "$no_dependency_runner"
perl -0pi -e 's/\n      - run: \.\/scripts\/test-dependency-review-runner\.sh\n//' "$no_dependency_runner"
expect_fail "Release Readiness must keep dependency review runner self-test" "$ci" "$no_dependency_runner"

echo "GitHub workflow check tests passed."
