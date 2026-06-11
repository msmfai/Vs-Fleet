#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

repo="$TMPDIR/repo"
mkdir "$repo"
git -C "$repo" init -q
git -C "$repo" config user.name "Fleet Test"
git -C "$repo" config user.email "fleet-test@example.com"

printf 'base\n' >"$repo/file.txt"
git -C "$repo" add file.txt
git -C "$repo" commit -q -m "base"
base="$(git -C "$repo" rev-parse HEAD)"

printf 'signed\n' >"$repo/file.txt"
git -C "$repo" add file.txt
git -C "$repo" commit -q -m $'signed change\n\nSigned-off-by: Fleet Test <fleet-test@example.com>'

if ! (cd "$repo" && "$ROOT/scripts/check-dco-signoff.sh" "$base..HEAD") >"$TMPDIR/pass.out" 2>&1; then
  echo "FAIL: expected signed commit to pass" >&2
  cat "$TMPDIR/pass.out" >&2
  exit 1
fi

printf 'unsigned\n' >"$repo/file.txt"
git -C "$repo" add file.txt
git -C "$repo" commit -q -m "unsigned change"

if (cd "$repo" && "$ROOT/scripts/check-dco-signoff.sh" "$base..HEAD") >"$TMPDIR/fail.out" 2>&1; then
  echo "FAIL: expected unsigned commit to fail" >&2
  cat "$TMPDIR/fail.out" >&2
  exit 1
fi

if ! rg -q 'missing DCO Signed-off-by' "$TMPDIR/fail.out"; then
  echo "FAIL: unsigned failure should explain the missing sign-off" >&2
  cat "$TMPDIR/fail.out" >&2
  exit 1
fi

if ! "$ROOT/scripts/check-dco-signoff.sh" >"$TMPDIR/skip.out" 2>&1; then
  echo "FAIL: local no-range DCO run should skip cleanly" >&2
  cat "$TMPDIR/skip.out" >&2
  exit 1
fi

if ! rg -q 'skipped' "$TMPDIR/skip.out"; then
  echo "FAIL: local no-range DCO run should say it skipped" >&2
  cat "$TMPDIR/skip.out" >&2
  exit 1
fi

echo "DCO sign-off tests passed."
