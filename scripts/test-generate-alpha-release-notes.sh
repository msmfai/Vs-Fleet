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
  draft-owner-decisions.sh \
  history-release-check.sh
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

notes="$TMPDIR/release-notes.md"
if ! (cd "$repo" &&
  FLEET_RELEASE_DATE=2026-06-11 \
  FLEET_CI_RUN=https://github.com/smfmarin/vs-fleet/actions/runs/123456001 \
  FLEET_RELEASE_READINESS_RUN=https://github.com/smfmarin/vs-fleet/actions/runs/123456002 \
  FLEET_DEPENDENCY_REVIEW_DATE=2026-06-11 \
  FLEET_DEPENDENCY_ACCEPTED_FINDINGS=none \
  FLEET_SECURITY_CHANNEL='GitHub Private Vulnerability Reporting enabled' \
  ./scripts/generate-alpha-release-notes.sh \
    v0.1.0-alpha.1 "$source_commit" "$notes" \
    change="Added public alpha release controls.") >"$TMPDIR/generate.out" 2>&1; then
  echo "FAIL: expected alpha release notes generation to pass" >&2
  cat "$TMPDIR/generate.out" >&2
  exit 1
fi

if ! "$ROOT/scripts/check-release-notes.sh" "$notes" "$source_commit" >"$TMPDIR/check.out" 2>&1; then
  echo "FAIL: generated release notes should pass checker" >&2
  cat "$TMPDIR/check.out" >&2
  cat "$notes" >&2
  exit 1
fi

for pattern in \
  '^- Version: v0\.1\.0-alpha\.1$' \
  "^- Commit: $source_commit$" \
  '^- Distribution: source-only$' \
  '^- Project license: MIT$' \
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
  v0.1.0-alpha.1 "$source_commit" "$notes") >"$TMPDIR/overwrite.out" 2>&1; then
  echo "FAIL: existing release notes should not be overwritten by default" >&2
  cat "$TMPDIR/overwrite.out" >&2
  exit 1
fi

if ! (cd "$repo" && FLEET_RELEASE_DATE=2026-06-11 FLEET_ALPHA_RELEASE_NOTES_FORCE=1 \
  ./scripts/generate-alpha-release-notes.sh v0.1.0-alpha.1 "$source_commit" "$notes") >"$TMPDIR/force.out" 2>&1; then
  echo "FAIL: forced release notes overwrite should pass" >&2
  cat "$TMPDIR/force.out" >&2
  exit 1
fi

perl -0pi -e 's/^Decision record status: APPROVED$/Decision record status: PENDING/m' \
  "$repo/docs/release/OWNER_DECISION_RECORD.md"
git -C "$repo" add docs/release/OWNER_DECISION_RECORD.md
git -C "$repo" commit -q -m "make owner record pending"
pending_commit="$(git -C "$repo" rev-parse HEAD)"

if (cd "$repo" && FLEET_RELEASE_DATE=2026-06-11 FLEET_ALPHA_RELEASE_NOTES_FORCE=1 \
  ./scripts/generate-alpha-release-notes.sh v0.1.0-alpha.1 "$pending_commit" "$notes") >"$TMPDIR/pending.out" 2>&1; then
  echo "FAIL: pending owner decision record should block release notes generation" >&2
  cat "$TMPDIR/pending.out" >&2
  exit 1
fi

echo "Alpha release notes generator tests passed."
