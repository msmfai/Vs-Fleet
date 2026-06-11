#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

repo="$TMPDIR/repo"
mkdir -p "$repo/scripts" "$repo/docs/release"

for script in \
  generate-alpha-release-notes.sh \
  check-release-notes.sh \
  check-owner-decisions.sh \
  check-public-branch-evidence.sh \
  check-ci-evidence-decision.sh \
  check-github-publication-evidence.sh \
  check-dependency-review-decision.sh \
  draft-owner-decisions.sh \
  history-release-check.sh \
  secret-release-check.sh
do
  cp "$ROOT/scripts/$script" "$repo/scripts/$script"
done
chmod +x "$repo/scripts/"*.sh

git -C "$repo" init -q
git -C "$repo" config user.email "release-test@example.invalid"
git -C "$repo" config user.name "Fleet Release Test"

"$repo/scripts/draft-owner-decisions.sh" smfmarin vs-fleet \
  "$repo/docs/release/OWNER_DECISION_RECORD.md" >/dev/null
perl -0pi -e 's/^Decision record status: PENDING$/Decision record status: APPROVED/m' \
  "$repo/docs/release/OWNER_DECISION_RECORD.md"

printf '# Fleet fixture\n' >"$repo/README.md"
git -C "$repo" add .
git -C "$repo" commit -q -m "reviewed source"
source_commit="$(git -C "$repo" rev-parse HEAD)"

public_root="$(git -C "$repo" commit-tree HEAD^{tree} -m "Initial public alpha source snapshot")"
git -C "$repo" branch public-alpha "$public_root"

cat >"$repo/docs/release/PUBLIC_BRANCH_EVIDENCE.md" <<EOF
# Public Branch Evidence
Public branch evidence status: PASS
Source commit: \`$source_commit\`
Public branch: \`public-alpha\`
Public root commit: \`$public_root\`
Release-control evidence file: \`docs/release/PUBLIC_BRANCH_EVIDENCE.md\`
History check command: \`./scripts/history-release-check.sh docs/release/OWNER_DECISION_RECORD.md public-alpha\`
History check result: \`PASS\`
Single root commit: \`yes\`
Public tree matches source commit tree: \`yes\`
Public branch contains no prior private history: \`yes\`
EOF

cat >"$repo/docs/release/PUBLIC_CI_EVIDENCE.md" <<EOF
# Public CI Evidence
Public CI evidence status: PASS
Commit: \`$source_commit\`
Release-control evidence file: \`docs/release/PUBLIC_CI_EVIDENCE.md\`
Branch: \`public-alpha\`
CI workflow run: \`https://github.com/smfmarin/vs-fleet/actions/runs/123456001\`
Release Readiness workflow run: \`https://github.com/smfmarin/vs-fleet/actions/runs/123456002\`
Local check transcript: \`not used\`
CI evidence path: \`not used\`
EOF

cat >"$repo/docs/release/GITHUB_PUBLICATION_EVIDENCE.md" <<EOF
# GitHub Publication Evidence
GitHub publication evidence status: PASS
Commit: \`$source_commit\`
Release-control evidence file: \`docs/release/GITHUB_PUBLICATION_EVIDENCE.md\`
Repository: \`https://github.com/smfmarin/vs-fleet\`
Default branch: \`public-alpha\`
Visibility consequences reviewed: \`yes\`
Repository name matches namespace: \`yes\`
Issues setting: \`enabled per support commitment\`
Discussions setting: \`disabled\`
Wiki setting: \`disabled\`
Releases setting: \`source tags and release notes only\`
Packages setting: \`not used for source-only alpha\`
GitHub Actions setting: \`enabled\`
Security reporting channel available: \`GitHub Private Vulnerability Reporting enabled\`
Secret scanning or accepted unavailable reason: \`enabled\`
Dependabot alerts or accepted unavailable reason: \`enabled\`
Default branch protection: \`enabled\`
Required source checks: \`CI source checks\`
Required release checks: \`Release Readiness\`
Linear history policy: \`preferred\`
Signed commit policy: \`not required\`
Release authority: \`single maintainer repository owner\`
Tag protection or accepted unavailable reason: \`owner-approved deferred: enable tag protection before first public tag if GitHub plan supports it\`
Release artifact custody: \`source tags and release notes only\`
Package publishing credentials: \`none for source-only alpha\`
Emergency removal owner: \`smfmarin\`
EOF

cat >"$repo/docs/release/DEPENDENCY_REVIEW_EVIDENCE.md" <<EOF
# Dependency Review Evidence
Dependency review status: PASS
Commit: \`$source_commit\`
Reviewed date: \`2026-06-11\`
Release-control evidence file: \`docs/release/DEPENDENCY_REVIEW_EVIDENCE.md\`
cargo tree: \`pass\`
cargo metadata --locked: \`pass\`
fleet-host cargo metadata --locked: \`pass\`
lockfile policy: \`pass\`
fleet-bridge npm audit: \`pass\`
extension npm audit: \`pass\`
generated artifact check: \`pass\`
Accepted findings: \`none\`
Accepted risk: \`not used\`
Dependency review evidence path: \`not used\`
EOF

git -C "$repo" add docs/release
git -C "$repo" commit -q -m "record release evidence"
evidence_commit="$(git -C "$repo" rev-parse HEAD)"

notes="$TMPDIR/release-notes.md"
if ! (cd "$repo" && FLEET_RELEASE_DATE=2026-06-11 ./scripts/generate-alpha-release-notes.sh \
  v0.1.0-alpha.1 "$evidence_commit" "$notes" \
  change="Added public alpha release evidence automation.") >"$TMPDIR/generate.out" 2>&1; then
  echo "FAIL: expected alpha release notes generation to pass" >&2
  cat "$TMPDIR/generate.out" >&2
  exit 1
fi

if ! "$ROOT/scripts/check-release-notes.sh" "$notes" "$public_root" >"$TMPDIR/check.out" 2>&1; then
  echo "FAIL: generated release notes should pass checker" >&2
  cat "$TMPDIR/check.out" >&2
  cat "$notes" >&2
  exit 1
fi

for pattern in \
  '^- Version: v0\.1\.0-alpha\.1$' \
  "^- Commit: $public_root$" \
  '^- Distribution: source-only$' \
  '^- Project license: MIT OR Apache-2.0$' \
  '^- Vulnerability reporting path: GitHub Private Vulnerability Reporting enabled$' \
  '^- History exposure audit: cleaned public history; public root commit ' \
  '^Fleet is too rough for a broad open-source launch, package announcement, binary$' \
  '^source-only alpha for technical review of the supported local macOS workflow\.$'
do
  if ! rg -q "$pattern" "$notes"; then
    echo "FAIL: generated notes missing pattern: $pattern" >&2
    cat "$notes" >&2
    exit 1
  fi
done

if (cd "$repo" && FLEET_RELEASE_DATE=2026-06-11 ./scripts/generate-alpha-release-notes.sh \
  v0.1.0-alpha.1 "$evidence_commit" "$notes") >"$TMPDIR/overwrite.out" 2>&1; then
  echo "FAIL: existing release notes should not be overwritten by default" >&2
  cat "$TMPDIR/overwrite.out" >&2
  exit 1
fi

if ! (cd "$repo" && FLEET_RELEASE_DATE=2026-06-11 FLEET_ALPHA_RELEASE_NOTES_FORCE=1 \
  ./scripts/generate-alpha-release-notes.sh v0.1.0-alpha.1 "$evidence_commit" "$notes") >"$TMPDIR/force.out" 2>&1; then
  echo "FAIL: forced release notes overwrite should pass" >&2
  cat "$TMPDIR/force.out" >&2
  exit 1
fi

perl -0pi -e 's/Public CI evidence status: PASS/Public CI evidence status: PENDING/' \
  "$repo/docs/release/PUBLIC_CI_EVIDENCE.md"
git -C "$repo" add docs/release/PUBLIC_CI_EVIDENCE.md
git -C "$repo" commit -q -m "make CI evidence pending"
pending_commit="$(git -C "$repo" rev-parse HEAD)"

if (cd "$repo" && FLEET_RELEASE_DATE=2026-06-11 FLEET_ALPHA_RELEASE_NOTES_FORCE=1 \
  ./scripts/generate-alpha-release-notes.sh v0.1.0-alpha.1 "$pending_commit" "$notes") >"$TMPDIR/pending.out" 2>&1; then
  echo "FAIL: pending CI evidence should block release notes generation" >&2
  cat "$TMPDIR/pending.out" >&2
  exit 1
fi

echo "Alpha release notes generator tests passed."
