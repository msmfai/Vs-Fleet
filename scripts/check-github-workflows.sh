#!/usr/bin/env bash
set -euo pipefail

ci="${1:-.github/workflows/ci.yml}"
release="${2:-.github/workflows/release-readiness.yml}"

require_file() {
  local file=$1
  if [ ! -f "$file" ]; then
    echo "FAIL: missing workflow file: $file"
    exit 1
  fi
}

require_text() {
  local file=$1
  local pattern=$2
  local description=$3
  if ! rg -q "$pattern" "$file"; then
    echo "FAIL: $file must contain $description"
    exit 1
  fi
}

require_file "$ci"
require_file "$release"

require_text "$ci" '^name:[[:space:]]*CI$' "workflow name CI"
require_text "$ci" 'pull_request:' "pull_request trigger"
require_text "$ci" 'push:' "push trigger"
require_text "$ci" '^permissions:$' "top-level workflow permissions"
require_text "$ci" '^[[:space:]]+contents:[[:space:]]*read$' "read-only contents permission"
require_text "$ci" 'DCO sign-off' "DCO sign-off job"
require_text "$ci" "github.event_name == 'pull_request'" "PR-only DCO job condition"
require_text "$ci" 'fetch-depth:[[:space:]]*0' "full checkout history for DCO checks"
require_text "$ci" './scripts/check-dco-signoff\.sh' "DCO sign-off check"
require_text "$ci" 'cargo fmt --all -- --check' "Rust formatting check"
require_text "$ci" 'cargo clippy --workspace --all-targets --all-features -- -D warnings' \
  "workspace clippy with warnings denied"
require_text "$ci" 'cargo test --workspace --all-targets --all-features' \
  "workspace test command"
require_text "$ci" "\\*\\*/Cargo\\.lock" "Cargo cache keys include lockfiles"
require_text "$ci" 'cargo llvm-cov report --fail-under-lines 80' \
  "workspace coverage floor"
require_text "$ci" 'cargo llvm-cov report --package fleet-protocol --package fleet-hub --fail-under-lines 85' \
  "protocol and hub coverage floor"
require_text "$ci" 'pnpm -r build' "recursive package build"
require_text "$ci" 'pnpm -r test' "recursive package test"
require_text "$ci" 'pnpm install --frozen-lockfile --ignore-scripts' \
  "strict frozen pnpm install"
if rg -q 'pnpm install --frozen-lockfile --ignore-scripts[[:space:]]*\|\|' "$ci"; then
  echo "FAIL: $ci must not fall back from frozen pnpm install"
  exit 1
fi
require_text "$ci" 'pnpm-lock\.yaml' "pnpm cache key includes lockfile"

require_text "$release" '^name:[[:space:]]*Release Readiness$' "workflow name Release Readiness"
require_text "$release" 'workflow_dispatch:' "manual workflow_dispatch trigger"
require_text "$release" '^permissions:$' "top-level workflow permissions"
require_text "$release" '^[[:space:]]+contents:[[:space:]]*read$' "read-only contents permission"
require_text "$release" './scripts/test-release-check.sh' "release-check self-test"
require_text "$release" './scripts/test-owner-release-approval-check.sh' \
  "owner release approval sheet self-test"
require_text "$release" './scripts/test-license-intent-check.sh' "license intent self-test"
require_text "$release" './scripts/test-dco-signoff.sh' "DCO sign-off self-test"
require_text "$release" './scripts/test-dependabot-config-check.sh' "Dependabot config self-test"
require_text "$release" './scripts/test-secret-release-check.sh' "secret exposure self-test"
require_text "$release" './scripts/test-doc-link-check.sh' "documentation link self-test"
require_text "$release" './scripts/test-public-tree-size-check.sh' "public tree size self-test"
require_text "$release" './scripts/test-lockfile-policy-check.sh' "lockfile policy self-test"
require_text "$release" './scripts/test-branding-decision-check.sh' \
  "branding decision self-test"
require_text "$release" './scripts/test-versioning-decision-check.sh' \
  "versioning decision self-test"
require_text "$release" './scripts/test-community-intake-decision-check.sh' \
  "community intake decision self-test"
require_text "$release" './scripts/test-release-custody-decision-check.sh' \
  "release custody decision self-test"
require_text "$release" './scripts/test-ai-contribution-decision-check.sh' \
  "AI contribution decision self-test"
require_text "$release" './scripts/test-platform-support-decision-check.sh' \
  "platform support decision self-test"
require_text "$release" './scripts/test-roadmap-decision-check.sh' \
  "roadmap decision self-test"
require_text "$release" './scripts/test-name-collision-decision-check.sh' \
  "name collision decision self-test"
require_text "$release" './scripts/test-local-data-decision-check.sh' \
  "local data decision self-test"
require_text "$release" './scripts/test-workflow-supply-chain-decision-check.sh' \
  "workflow supply-chain decision self-test"
require_text "$release" './scripts/test-github-publication-evidence-check.sh' \
  "GitHub publication evidence self-test"
require_text "$release" './scripts/test-public-branch-evidence-check.sh' \
  "public branch evidence self-test"
require_text "$release" './scripts/test-dependency-review-runner.sh' \
  "dependency review runner self-test"
require_text "$release" './scripts/check-owner-decisions.sh docs/release/OWNER_DECISION_RECORD.md' \
  "owner decision gate"
require_text "$release" './scripts/check-owner-release-approval.sh docs/release/OWNER_RELEASE_APPROVAL.md' \
  "owner release approval sheet gate"
require_text "$release" './scripts/history-release-check.sh docs/release/OWNER_DECISION_RECORD.md' \
  "history exposure gate"
require_text "$release" './scripts/check-public-branch-evidence.sh docs/release/OWNER_DECISION_RECORD.md docs/release/PUBLIC_BRANCH_EVIDENCE.md' \
  "public branch evidence gate"
require_text "$release" './scripts/secret-release-check.sh' "secret exposure gate"
require_text "$release" './scripts/check-doc-links.sh' "documentation link gate"
require_text "$release" './scripts/check-license-intent.sh' "license intent gate"
require_text "$release" './scripts/check-public-tree-size.sh' "public tree size gate"
require_text "$release" './scripts/check-lockfile-policy.sh' "lockfile policy gate"
require_text "$release" './scripts/release-check.sh' "release hygiene gate"
require_text "$release" 'cargo clippy --workspace --all-targets --all-features -- -D warnings' \
  "source alpha clippy check"
require_text "$release" 'cargo test --workspace --all-targets --all-features' \
  "source alpha Rust tests"
require_text "$release" 'cd crates/fleet-host' "standalone Fleet host check"
require_text "$release" './bundle.sh release' "Fleet host bundle verification"
require_text "$release" 'npm run build' "npm build checks"
require_text "$release" 'npm test' "extension npm tests"

echo "GitHub workflow check passed."
