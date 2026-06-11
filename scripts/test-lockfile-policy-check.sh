#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

repo="$TMPDIR/repo"
output="$TMPDIR/lockfile-policy.out"
mkdir -p "$repo/scripts" "$repo/crates/fleet-host" "$repo/packages/fleet-bridge" "$repo/packages/extension"
cp "$ROOT/scripts/check-lockfile-policy.sh" "$repo/scripts/check-lockfile-policy.sh"
chmod +x "$repo/scripts/check-lockfile-policy.sh"

git -C "$repo" init -q
git -C "$repo" config user.email "release-test@example.invalid"
git -C "$repo" config user.name "Fleet Release Test"

write_lockfiles() {
  printf '# root cargo lock\n' >"$repo/Cargo.lock"
  printf '# host cargo lock\n' >"$repo/crates/fleet-host/Cargo.lock"
  printf 'lockfileVersion: "9.0"\n' >"$repo/pnpm-lock.yaml"
  printf '{"lockfileVersion":3}\n' >"$repo/packages/fleet-bridge/package-lock.json"
  printf '{"lockfileVersion":3}\n' >"$repo/packages/extension/package-lock.json"
}

expect_pass() {
  local label=$1
  if ! (cd "$repo" && ./scripts/check-lockfile-policy.sh) >"$output" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$output" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  if (cd "$repo" && ./scripts/check-lockfile-policy.sh) >"$output" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$output" >&2
    exit 1
  fi
}

write_lockfiles
git -C "$repo" add .
git -C "$repo" commit -q -m "tracked lockfiles"
expect_pass "tracked lockfiles are accepted"

git -C "$repo" rm -q pnpm-lock.yaml
git -C "$repo" commit -q -m "remove pnpm lockfile"
expect_fail "missing tracked pnpm lockfile is rejected"

write_lockfiles
git -C "$repo" add .
git -C "$repo" commit -q -m "restore lockfiles"
printf 'Cargo.lock\n' >"$repo/.gitignore"
git -C "$repo" add .gitignore
git -C "$repo" commit -q -m "ignore cargo lockfile"
expect_fail "ignored required lockfile is rejected"

echo "Lockfile policy check tests passed."
