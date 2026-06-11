#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

repo="$TMPDIR/repo"
mkdir -p "$repo/scripts" "$repo/docs/release"
cp "$ROOT/scripts/generate-public-branch-evidence.sh" "$repo/scripts/generate-public-branch-evidence.sh"
cp "$ROOT/scripts/check-public-branch-evidence.sh" "$repo/scripts/check-public-branch-evidence.sh"
cp "$ROOT/scripts/history-release-check.sh" "$repo/scripts/history-release-check.sh"
chmod +x "$repo/scripts/"*.sh

git -C "$repo" init -q
git -C "$repo" config user.email "release-test@example.invalid"
git -C "$repo" config user.name "Fleet Release Test"

cat >"$repo/docs/release/OWNER_DECISION_RECORD.md" <<'EOF'
# Owner Decision Record

Decision record status: APPROVED

## Required Before Public GitHub Visibility

### 2. Public History

- [ ] Publish the current branch history and accept that old commits may contain
  removed local artifacts or failed eval evidence.
- [x] Publish a cleaned/squashed history for the first public branch.

### 3. Public Namespace
EOF

printf '# Fleet fixture\n' >"$repo/README.md"
git -C "$repo" add .
git -C "$repo" commit -q -m "clean start"

mkdir "$repo/artifacts"
fixture_path="/""Users/evidence/private-project"
printf '{"path":"%s"}\n' "$fixture_path" >"$repo/artifacts/raw.json"
git -C "$repo" add artifacts/raw.json
git -C "$repo" commit -q -m "add private artifact"

rm "$repo/artifacts/raw.json"
rmdir "$repo/artifacts"
printf 'public tree\n' >"$repo/README.md"
git -C "$repo" add -A
git -C "$repo" commit -q -m "remove private artifact"

source_commit="$(git -C "$repo" rev-parse HEAD)"
public_commit="$(git -C "$repo" commit-tree HEAD^{tree} -m "clean public snapshot")"
git -C "$repo" branch public-alpha "$public_commit"

evidence="$repo/docs/release/PUBLIC_BRANCH_EVIDENCE.md"
if ! (cd "$repo" && ./scripts/generate-public-branch-evidence.sh public-alpha HEAD "$evidence") >"$TMPDIR/generate.out" 2>&1; then
  echo "FAIL: expected evidence generation to pass" >&2
  cat "$TMPDIR/generate.out" >&2
  exit 1
fi

if ! rg -q "Source commit: \`$source_commit\`" "$evidence"; then
  echo "FAIL: generated evidence did not record source commit" >&2
  cat "$evidence" >&2
  exit 1
fi

if ! (cd "$repo" && ./scripts/check-public-branch-evidence.sh docs/release/OWNER_DECISION_RECORD.md "$evidence" "$source_commit") >"$TMPDIR/check.out" 2>&1; then
  echo "FAIL: generated evidence should pass checker" >&2
  cat "$TMPDIR/check.out" >&2
  exit 1
fi

if (cd "$repo" && ./scripts/generate-public-branch-evidence.sh public-alpha HEAD "$evidence") >"$TMPDIR/overwrite.out" 2>&1; then
  echo "FAIL: existing evidence file should not be overwritten by default" >&2
  cat "$TMPDIR/overwrite.out" >&2
  exit 1
fi

if ! (cd "$repo" && FLEET_PUBLIC_BRANCH_EVIDENCE_FORCE=1 ./scripts/generate-public-branch-evidence.sh public-alpha HEAD "$evidence") >"$TMPDIR/force.out" 2>&1; then
  echo "FAIL: forced evidence overwrite should pass" >&2
  cat "$TMPDIR/force.out" >&2
  exit 1
fi

if (cd "$repo" && ./scripts/generate-public-branch-evidence.sh HEAD HEAD -) >"$TMPDIR/private.out" 2>&1; then
  echo "FAIL: multi-commit private branch evidence should fail" >&2
  cat "$TMPDIR/private.out" >&2
  exit 1
fi

echo "Public branch evidence generator tests passed."
