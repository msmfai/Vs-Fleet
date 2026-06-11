#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

repo="$TMPDIR/repo"
mkdir -p "$repo"
mkdir -p "$repo/scripts"

write_file() {
  local file=$1
  local body=${2:-}
  mkdir -p "$(dirname "$repo/$file")"
  printf '%s\n' "$body" >"$repo/$file"
}

write_script() {
  local file=$1
  local body=$2
  write_file "$file" "#!/usr/bin/env bash
set -euo pipefail
$body"
  chmod +x "$repo/$file"
}

cp "$ROOT/scripts/release-check.sh" "$repo/scripts/release-check.sh"

write_file "LICENSE" "MIT"
write_file "LICENSE-MIT" "MIT"
write_file "LICENSE-APACHE" "Apache-2.0"
write_file "Cargo.toml" '[workspace.package]
license = "MIT"'

for manifest in \
  crates/fleet-cli/Cargo.toml \
  crates/fleet-e2e/Cargo.toml \
  crates/fleet-host-core/Cargo.toml \
  crates/fleet-host/Cargo.toml \
  crates/fleet-hub/Cargo.toml \
  crates/fleet-protocol/Cargo.toml \
  crates/fleet-reporter/Cargo.toml
do
  write_file "$manifest" '[package]
name = "fleet-test"
license = "MIT"
publish = false'
done

for package in packages/fleet-bridge packages/extension; do
  write_file "$package/package.json" '{"name":"fleet-test","license":"MIT","private":true}'
  write_file "$package/package-lock.json" '{"packages":{"":{"license":"MIT"}}}'
done

for doc in \
  README.md \
  CONTRIBUTING.md \
  LAUNCH.md \
  PLAN.md \
  NORTH_STAR.md \
  SECURITY.md \
  LICENSE-MIT \
  LICENSE-APACHE \
  DCO.md \
  SUPPORT.md \
  CODE_OF_CONDUCT.md \
  docs/QUICKSTART.md \
  docs/ARCHITECTURE.md \
  docs/LOCAL_DATA_AND_UNINSTALL.md \
  docs/release/RELEASE_PROCESS.md \
  docs/release/PUBLIC_ALPHA_READINESS_ASSESSMENT.md \
  docs/release/DEPENDENCY_REVIEW.md \
  docs/release/DEPENDENCY_REVIEW_EVIDENCE.md \
  docs/release/GITHUB_PUBLICATION_RUNBOOK.md \
  docs/release/GITHUB_PUBLICATION_EVIDENCE.md \
  docs/release/PUBLIC_BRANCH_EVIDENCE.md \
  docs/release/PUBLIC_CI_EVIDENCE.md \
  docs/release/ASSET_PROVENANCE.md \
  docs/release/NAME_COLLISION_REVIEW.md \
  docs/release/WORKFLOW_SUPPLY_CHAIN.md \
  docs/release/LICENSE_INTENT.md \
  docs/release/OWNER_RELEASE_APPROVAL.md \
  docs/release/OWNER_DECISION_REPLY_TEMPLATE.md \
  docs/release/PUBLIC_ALPHA_OWNER_PROMPT.md \
  docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md \
  docs/release/ALPHA_RELEASE_CHECKLIST.md \
  docs/release/PUBLIC_ALPHA_DECISIONS.md \
  .github/dependabot.yml \
  docs/release/OWNER_DECISION_RECORD.md \
  .github/PULL_REQUEST_TEMPLATE.md \
  .github/ISSUE_TEMPLATE/config.yml \
  .github/ISSUE_TEMPLATE/bug_report.yml \
  .github/ISSUE_TEMPLATE/alpha_feedback.yml \
  .github/workflows/ci.yml \
  .github/workflows/release-readiness.yml
do
  write_file "$doc" "release-ready test fixture"
done

write_file "PLAN.md" "Private for now"
write_file "CONTRIBUTING.md" "until the project license has been chosen and applied"

for script in \
  scripts/check-owner-decisions.sh \
  scripts/check-owner-release-approval.sh \
  scripts/check-owner-reply-template.sh \
  scripts/check-public-alpha-readiness-assessment.sh \
  scripts/draft-owner-decisions.sh \
  scripts/public-alpha-decision-packet.sh \
  scripts/history-release-check.sh \
  scripts/check-public-branch-evidence.sh \
  scripts/generate-public-branch-evidence.sh \
  scripts/check-public-release-branch.sh \
  scripts/prepare-public-branch.sh \
  scripts/secret-release-check.sh \
  scripts/check-doc-links.sh \
  scripts/check-license-intent.sh \
  scripts/check-public-tree-size.sh \
  scripts/check-lockfile-policy.sh \
  scripts/check-alpha-scope-decision.sh \
  scripts/check-editor-server-boundary-decision.sh \
  scripts/check-distribution-decision.sh \
  scripts/check-security-reporting-decision.sh \
  scripts/check-contribution-decision.sh \
  scripts/check-github-intake-templates.sh \
  scripts/check-ci-evidence-decision.sh \
  scripts/generate-public-ci-evidence.sh \
  scripts/release-evidence-status.sh \
  scripts/check-github-publication-evidence.sh \
  scripts/check-github-workflows.sh \
  scripts/generate-github-publication-evidence.sh \
  scripts/check-privacy-decision.sh \
  scripts/check-dependency-review-decision.sh \
  scripts/run-dependency-review.sh \
  scripts/check-dependabot-config.sh \
  scripts/check-support-decision.sh \
  scripts/check-branding-decision.sh \
  scripts/check-versioning-decision.sh \
  scripts/check-community-intake-decision.sh \
  scripts/check-release-custody-decision.sh \
  scripts/check-ai-contribution-decision.sh \
  scripts/check-platform-support-decision.sh \
  scripts/check-roadmap-decision.sh \
  scripts/check-name-collision-decision.sh \
  scripts/check-local-data-decision.sh \
  scripts/check-workflow-supply-chain-decision.sh \
  scripts/check-dco-signoff.sh \
  scripts/test-owner-decision-gate.sh \
  scripts/test-owner-release-approval-check.sh \
  scripts/test-owner-reply-template-check.sh \
  scripts/test-public-alpha-readiness-assessment-check.sh \
  scripts/test-draft-owner-decisions.sh \
  scripts/test-public-alpha-decision-packet.sh \
  scripts/test-history-release-check.sh \
  scripts/test-public-branch-evidence-check.sh \
  scripts/test-generate-public-branch-evidence.sh \
  scripts/test-check-public-release-branch.sh \
  scripts/test-prepare-public-branch.sh \
  scripts/test-secret-release-check.sh \
  scripts/test-doc-link-check.sh \
  scripts/test-license-intent-check.sh \
  scripts/test-public-tree-size-check.sh \
  scripts/test-lockfile-policy-check.sh \
  scripts/apply-license-decision.sh \
  scripts/test-apply-license-decision.sh \
  scripts/apply-namespace-decision.sh \
  scripts/test-apply-namespace-decision.sh \
  scripts/test-license-decision-check.sh \
  scripts/test-namespace-decision-check.sh \
  scripts/test-alpha-scope-decision-check.sh \
  scripts/test-editor-server-boundary-decision-check.sh \
  scripts/test-distribution-decision-check.sh \
  scripts/test-security-reporting-decision-check.sh \
  scripts/test-contribution-decision-check.sh \
  scripts/test-github-intake-templates-check.sh \
  scripts/test-ci-evidence-decision-check.sh \
  scripts/test-generate-public-ci-evidence.sh \
  scripts/test-release-evidence-status.sh \
  scripts/test-github-publication-evidence-check.sh \
  scripts/test-generate-github-publication-evidence.sh \
  scripts/test-github-workflows-check.sh \
  scripts/test-privacy-decision-check.sh \
  scripts/test-dependency-review-decision-check.sh \
  scripts/test-dependency-review-runner.sh \
  scripts/test-dependabot-config-check.sh \
  scripts/test-support-decision-check.sh \
  scripts/test-branding-decision-check.sh \
  scripts/test-versioning-decision-check.sh \
  scripts/test-community-intake-decision-check.sh \
  scripts/test-release-custody-decision-check.sh \
  scripts/test-ai-contribution-decision-check.sh \
  scripts/test-platform-support-decision-check.sh \
  scripts/test-roadmap-decision-check.sh \
  scripts/test-name-collision-decision-check.sh \
  scripts/test-local-data-decision-check.sh \
  scripts/test-workflow-supply-chain-decision-check.sh \
  scripts/test-dco-signoff.sh \
  scripts/test-release-check.sh \
  scripts/check-release-notes.sh \
  scripts/generate-alpha-release-notes.sh \
  scripts/test-generate-alpha-release-notes.sh \
  scripts/test-release-notes-check.sh
do
  write_script "$script" 'exit 0'
done

write_script "scripts/check-license-decision.sh" 'echo LICENSE_FAIL
exit 1'
write_script "scripts/check-namespace-decision.sh" 'echo NAMESPACE_FAIL
exit 1'

(
  cd "$repo"
  git init -q
  git config user.email fleet@example.invalid
  git config user.name Fleet
  git add .
  git commit -qm fixture
)

output="$TMPDIR/release-check.out"
if (cd "$repo" && ./scripts/release-check.sh) >"$output" 2>&1; then
  echo "FAIL: release check should fail when specialized gates fail" >&2
  cat "$output" >&2
  exit 1
fi

if ! rg -q 'LICENSE_FAIL' "$output"; then
  echo "FAIL: release check did not report the license gate failure" >&2
  cat "$output" >&2
  exit 1
fi

if ! rg -q 'NAMESPACE_FAIL' "$output"; then
  echo "FAIL: release check stopped before reporting the namespace gate failure" >&2
  cat "$output" >&2
  exit 1
fi

if ! rg -q 'stale private-license or broad-scope planning claims' "$output"; then
  echo "FAIL: release check did not reject stale root planning claims" >&2
  cat "$output" >&2
  exit 1
fi

if ! rg -q 'unresolved alpha blockers' "$output"; then
  echo "FAIL: release check did not reject stale contribution licensing language" >&2
  cat "$output" >&2
  exit 1
fi

write_script "scripts/check-owner-decisions.sh" 'echo OWNER_FAIL
exit 1'
write_script "scripts/history-release-check.sh" 'echo "HISTORY_ARGS:$*"
echo HISTORY_FAIL
exit 1'
write_script "scripts/secret-release-check.sh" 'echo "SECRET_ARGS:$*"
exit 0'

output_owner_pending="$TMPDIR/release-check-owner-pending.out"
if (cd "$repo" && FLEET_RELEASE_HISTORY_REF=public-alpha ./scripts/release-check.sh) >"$output_owner_pending" 2>&1; then
  echo "FAIL: release check should fail when owner and history gates fail" >&2
  cat "$output_owner_pending" >&2
  exit 1
fi

if ! rg -q 'OWNER_FAIL' "$output_owner_pending"; then
  echo "FAIL: release check did not report the owner gate failure" >&2
  cat "$output_owner_pending" >&2
  exit 1
fi

if ! rg -q 'HISTORY_FAIL' "$output_owner_pending"; then
  echo "FAIL: release check did not run history gate while owner record was unapproved" >&2
  cat "$output_owner_pending" >&2
  exit 1
fi

if ! rg -q 'HISTORY_ARGS:docs/release/OWNER_DECISION_RECORD.md public-alpha' "$output_owner_pending"; then
  echo "FAIL: release check did not pass the requested history ref to the history gate" >&2
  cat "$output_owner_pending" >&2
  exit 1
fi

if ! rg -q 'SECRET_ARGS:public-alpha' "$output_owner_pending"; then
  echo "FAIL: release check did not pass the requested history ref to the secret gate" >&2
  cat "$output_owner_pending" >&2
  exit 1
fi

if rg -q 'LICENSE_FAIL|NAMESPACE_FAIL' "$output_owner_pending"; then
  echo "FAIL: release check ran owner-dependent gates before owner approval" >&2
  cat "$output_owner_pending" >&2
  exit 1
fi

echo "Release check aggregation tests passed."
