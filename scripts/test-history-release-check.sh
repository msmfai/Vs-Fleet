#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

repo="$TMPDIR/repo"
output="$TMPDIR/history-release-check.out"
mkdir "$repo"
git -C "$repo" init -q
git -C "$repo" config user.email "release-test@example.invalid"
git -C "$repo" config user.name "Fleet Release Test"

cat >"$repo/README.md" <<'EOF'
# Clean Fixture
EOF
git -C "$repo" add README.md
git -C "$repo" commit -q -m "clean fixture"

expect_pass() {
  local label=$1
  shift
  if ! (cd "$repo" && "$ROOT/scripts/history-release-check.sh" "$@") >"$output" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$output" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  shift
  if (cd "$repo" && "$ROOT/scripts/history-release-check.sh" "$@") >"$output" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$output" >&2
    exit 1
  fi
}

expect_pass "clean history"

mkdir -p "$repo/artifacts"
cat >"$repo/artifacts/raw.json" <<'EOF'
{"path": "/Users/release-test/private-project"}
EOF
git -C "$repo" add artifacts/raw.json
git -C "$repo" commit -q -m "add raw artifact"
rm "$repo/artifacts/raw.json"
git -C "$repo" add -u
git -C "$repo" commit -q -m "remove raw artifact"

expect_fail "removed artifact remains in history"

owner="$TMPDIR/OWNER_DECISION_RECORD.md"
cat >"$owner" <<'EOF'
# Owner Decision Record

Decision record status: APPROVED

## Required Before Public GitHub Visibility

### 2. Public History

- [x] Publish the current branch history and accept that old commits may contain removed local artifacts or failed eval evidence.
EOF

expect_pass "accepted current history exposure" "$owner"

echo "History release gate tests passed."
