#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

write_owner_record() {
  local file=$1
  local status=$2
  local checked=$3
  cat >"$file" <<EOF
# Owner Decision Record

Decision record status: $status

## Required Before Public GitHub Visibility

### 1. License

- [$([ "$checked" = "dual" ] && echo x || echo ' ')] MIT OR Apache-2.0 dual license.
- [$([ "$checked" = "mit" ] && echo x || echo ' ')] MIT only.
- [$([ "$checked" = "apache" ] && echo x || echo ' ')] Apache-2.0 only.
- [$([ "$checked" = "agpl" ] && echo x || echo ' ')] AGPL-3.0-only.
- [ ] Other: \`TODO\`

### 2. Public History
EOF
}

write_tree() {
  local root=$1
  local license=$2
  mkdir -p "$root/crates/fleet-host" "$root/packages/fleet-bridge" "$root/packages/extension"
  printf 'License text for %s\n' "$license" >"$root/LICENSE"
  cat >"$root/Cargo.toml" <<EOF
[workspace.package]
license = "$license"
EOF
  cat >"$root/crates/fleet-host/Cargo.toml" <<EOF
[package]
license = "$license"
EOF
  cat >"$root/packages/fleet-bridge/package.json" <<EOF
{"license":"$license"}
EOF
  cat >"$root/packages/extension/package.json" <<EOF
{"license":"$license"}
EOF
  cat >"$root/packages/fleet-bridge/package-lock.json" <<EOF
{"packages":{"":{"license":"$license"}}}
EOF
  cat >"$root/packages/extension/package-lock.json" <<EOF
{"packages":{"":{"license":"$license"}}}
EOF
}

expect_pass() {
  local label=$1
  local owner=$2
  local root=$3
  if ! "$ROOT/scripts/check-license-decision.sh" "$owner" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  local owner=$2
  local root=$3
  if "$ROOT/scripts/check-license-decision.sh" "$owner" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

valid_root="$TMPDIR/valid-root"
owner_dual="$TMPDIR/owner-dual.md"
write_owner_record "$owner_dual" APPROVED dual
write_tree "$valid_root" "MIT OR Apache-2.0"
expect_pass "dual license metadata matches" "$owner_dual" "$valid_root"

mit_root="$TMPDIR/mit-root"
owner_mit="$TMPDIR/owner-mit.md"
write_owner_record "$owner_mit" APPROVED mit
write_tree "$mit_root" "MIT"
expect_pass "MIT license metadata matches" "$owner_mit" "$mit_root"

bad_cargo="$TMPDIR/bad-cargo"
write_tree "$bad_cargo" "MIT OR Apache-2.0"
printf '[workspace.package]\nlicense = "MIT"\n' >"$bad_cargo/Cargo.toml"
expect_fail "Cargo metadata mismatch is rejected" "$owner_dual" "$bad_cargo"

bad_lock="$TMPDIR/bad-lock"
write_tree "$bad_lock" "MIT OR Apache-2.0"
printf '{"packages":{"":{"license":"MIT"}}}\n' >"$bad_lock/packages/fleet-bridge/package-lock.json"
expect_fail "package lock root license mismatch is rejected" "$owner_dual" "$bad_lock"

missing_license="$TMPDIR/missing-license"
write_tree "$missing_license" "MIT OR Apache-2.0"
rm "$missing_license/LICENSE"
expect_fail "missing root LICENSE is rejected" "$owner_dual" "$missing_license"

pending_owner="$TMPDIR/pending-owner.md"
write_owner_record "$pending_owner" PENDING dual
expect_fail "pending owner record is rejected" "$pending_owner" "$valid_root"

echo "License decision check tests passed."
