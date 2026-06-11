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
- Branding: Fleet name and icon are alpha placeholders
- Owner decision record: docs/release/OWNER_DECISION_RECORD.md at this commit

## Alpha Readiness Warning

Fleet is too rough for a broad open-source launch, package announcement, binary
distribution, or stable-project presentation. This release is a narrow
source-only alpha for technical review of the supported local macOS workflow.

## Alpha Scope

This alpha is intended for local macOS source builds and local code serve-web
sessions.

## Supported Platform And Toolchain

- macOS source build only.
- Rust 1.78 or newer.
- Node.js 20 and npm.
- user-provided VS Code `code` CLI.

## Roadmap And Non-Goals

- No public roadmap commitments are made during alpha.
- Issues, labels, and milestones are triage hints only.

## Naming And Trademark Posture

- `Fleet` is a provisional source-alpha working name.
- This release makes no trademark claim.

## Local Data And Cleanup

- Runtime data lives under `~/.fleet/run` and `~/.fleet/mux`.
- Manual cleanup after closing Fleet-spawned servers: `rm -rf ~/.fleet/run ~/.fleet/mux`.

## Workflow Supply Chain

- GitHub Actions use read-only `GITHUB_TOKEN` permissions.
- Workflows use no repository secrets or publishing credentials.

## What Changed

- Added release readiness gates.

## Verification

- GitHub CI on exact commit: https://example.invalid/actions/runs/1
- Release readiness workflow: https://example.invalid/actions/runs/2
- Rust workspace checks: passed locally.
- Fleet host checks: passed locally.
- JavaScript/package checks: passed locally.
- Lockfile policy: passed.
- Dependency review: completed, no accepted findings.
- Documentation link audit: passed.
- Public tree size audit: passed.
- History exposure audit: cleaned public history.
- Secret exposure audit: passed.
- Release hygiene gate: passed.

## Dependency And License Review

- Project license: MIT
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

branding_choice="$TMPDIR/branding-choice.md"
write_valid_notes "$branding_choice"
perl -0pi -e 's/Branding: Fleet name and icon are alpha placeholders/Branding: `[alpha placeholders | name stable]`/' "$branding_choice"
expect_fail "unresolved branding choice is rejected" "$branding_choice"

missing="$TMPDIR/missing.md"
write_valid_notes "$missing"
perl -0pi -e 's/\n## Verification\n/\n/' "$missing"
expect_fail "missing required section is rejected" "$missing"

missing_warning="$TMPDIR/missing-warning.md"
write_valid_notes "$missing_warning"
perl -0pi -e 's/\n## Alpha Readiness Warning\n.*?\n## Alpha Scope\n/\n## Alpha Scope\n/s' "$missing_warning"
expect_fail "missing alpha readiness warning is rejected" "$missing_warning"

missing_secret="$TMPDIR/missing-secret.md"
write_valid_notes "$missing_secret"
perl -0pi -e 's/\n- Secret exposure audit: passed\.\n/\n/' "$missing_secret"
expect_fail "missing secret exposure audit is rejected" "$missing_secret"

missing_doc_links="$TMPDIR/missing-doc-links.md"
write_valid_notes "$missing_doc_links"
perl -0pi -e 's/\n- Documentation link audit: passed\.\n/\n/' "$missing_doc_links"
expect_fail "missing documentation link audit is rejected" "$missing_doc_links"

missing_lockfile="$TMPDIR/missing-lockfile.md"
write_valid_notes "$missing_lockfile"
perl -0pi -e 's/\n- Lockfile policy: passed\.\n/\n/' "$missing_lockfile"
expect_fail "missing lockfile policy is rejected" "$missing_lockfile"

missing_tree_size="$TMPDIR/missing-tree-size.md"
write_valid_notes "$missing_tree_size"
perl -0pi -e 's/\n- Public tree size audit: passed\.\n/\n/' "$missing_tree_size"
expect_fail "missing public tree size audit is rejected" "$missing_tree_size"

missing_platform="$TMPDIR/missing-platform.md"
write_valid_notes "$missing_platform"
perl -0pi -e 's/\n## Supported Platform And Toolchain\n.*?\n## Roadmap And Non-Goals\n/\n## Roadmap And Non-Goals\n/s' "$missing_platform"
expect_fail "missing platform/toolchain boundary is rejected" "$missing_platform"

missing_name="$TMPDIR/missing-name.md"
write_valid_notes "$missing_name"
perl -0pi -e 's/`Fleet` is a provisional source-alpha working name\./Fleet is stable./' "$missing_name"
expect_fail "missing provisional name posture is rejected" "$missing_name"

missing_local_data="$TMPDIR/missing-local-data.md"
write_valid_notes "$missing_local_data"
perl -0pi -e 's/\n- Runtime data lives under `~\/\.fleet\/run` and `~\/\.fleet\/mux`\.\n/\n/' "$missing_local_data"
perl -0pi -e 's/\n- Manual cleanup after closing Fleet-spawned servers: `rm -rf ~\/\.fleet\/run ~\/\.fleet\/mux`\.\n/\n/' "$missing_local_data"
expect_fail "missing local data locations are rejected" "$missing_local_data"

missing_supply_chain="$TMPDIR/missing-supply-chain.md"
write_valid_notes "$missing_supply_chain"
perl -0pi -e 's/GitHub Actions use read-only `GITHUB_TOKEN` permissions\./GitHub Actions are configured./; s/Workflows use no repository secrets or publishing credentials\./Workflow details are recorded./' "$missing_supply_chain"
expect_fail "missing workflow supply-chain posture is rejected" "$missing_supply_chain"

exception="$TMPDIR/exception.md"
write_valid_notes "$exception"
printf '\n- Dependency review: owner-approved skip\n' >>"$exception"
expect_fail "exception shorthand is rejected" "$exception"

echo "Release notes check tests passed."
