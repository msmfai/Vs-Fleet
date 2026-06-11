#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

repo="$TMPDIR/repo"
output="$TMPDIR/public-tree-size.out"
mkdir -p "$repo/scripts" "$repo/crates/fleet-host/icons"
cp "$ROOT/scripts/check-public-tree-size.sh" "$repo/scripts/check-public-tree-size.sh"
chmod +x "$repo/scripts/check-public-tree-size.sh"

git -C "$repo" init -q
git -C "$repo" config user.email "release-test@example.invalid"
git -C "$repo" config user.name "Fleet Release Test"

write_bytes() {
  local file=$1
  local bytes=$2
  mkdir -p "$(dirname "$file")"
  dd if=/dev/zero of="$file" bs=1 count=0 seek="$bytes" 2>/dev/null
}

expect_pass() {
  local label=$1
  if ! (cd "$repo" && ./scripts/check-public-tree-size.sh) >"$output" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$output" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  if (cd "$repo" && ./scripts/check-public-tree-size.sh) >"$output" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$output" >&2
    exit 1
  fi
}

printf 'small\n' >"$repo/README.md"
write_bytes "$repo/crates/fleet-host/icons/icon.png" $((3 * 1024 * 1024))
git -C "$repo" add .
git -C "$repo" commit -q -m "allowed source icon"
expect_pass "source icon has a narrow larger allowance"

write_bytes "$repo/docs/screenshot.png" $((2 * 1024 * 1024))
git -C "$repo" add docs/screenshot.png
git -C "$repo" commit -q -m "large generated artifact"
expect_fail "non-icon large file is rejected"

git -C "$repo" rm -q docs/screenshot.png
write_bytes "$repo/crates/fleet-host/icons/icon.png" $((6 * 1024 * 1024))
git -C "$repo" add crates/fleet-host/icons/icon.png
git -C "$repo" commit -q -m "oversized source icon"
expect_fail "oversized source icon is rejected"

echo "Public tree size check tests passed."
