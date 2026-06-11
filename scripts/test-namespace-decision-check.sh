#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

write_owner_record() {
  local file=$1
  local status=$2
  local npm_names=${3:-"fleet-extension, fleet-bridge"}
  cat >"$file" <<EOF
# Owner Decision Record

Decision record status: $status

## Required Before Public GitHub Visibility

### 3. Public Namespace

| Surface | Decision |
|---|---|
| GitHub org/user | smfmarin |
| GitHub repo name | vs-fleet |
| Product name | Fleet |
| Rust crate prefix | fleet-* |
| npm package names | $npm_names |
| VS Code Marketplace publisher | fleet-team |
| Open VSX publisher | fleet-team |
| macOS bundle id | dev.fleet.host |

### 4. Alpha Scope
EOF
}

write_tree() {
  local root=$1
  mkdir -p "$root/crates/fleet-cli" "$root/crates/fleet-host" "$root/packages/fleet-bridge" "$root/packages/extension"
  cat >"$root/crates/fleet-cli/Cargo.toml" <<'EOF'
[package]
name = "fleet-cli"
EOF
  cat >"$root/crates/fleet-host/Cargo.toml" <<'EOF'
[package]
name = "fleet-host"
EOF
  cat >"$root/crates/fleet-host/tauri.conf.json" <<'EOF'
{
  "productName": "Fleet",
  "identifier": "dev.fleet.host"
}
EOF
  cat >"$root/packages/fleet-bridge/package.json" <<'EOF'
{
  "name": "fleet-bridge",
  "publisher": "fleet-team"
}
EOF
  cat >"$root/packages/extension/package.json" <<'EOF'
{
  "name": "fleet-extension",
  "publisher": "fleet-team"
}
EOF
}

expect_pass() {
  local label=$1
  local owner=$2
  local root=$3
  if ! "$ROOT/scripts/check-namespace-decision.sh" "$owner" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  local owner=$2
  local root=$3
  if "$ROOT/scripts/check-namespace-decision.sh" "$owner" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

owner="$TMPDIR/owner.md"
valid_root="$TMPDIR/valid-root"
write_owner_record "$owner" APPROVED
write_tree "$valid_root"
expect_pass "namespace metadata matches" "$owner" "$valid_root"

pending="$TMPDIR/pending.md"
write_owner_record "$pending" PENDING
expect_fail "pending owner record is rejected" "$pending" "$valid_root"

bad_product="$TMPDIR/bad-product"
write_tree "$bad_product"
jq '.productName = "OtherProduct"' "$bad_product/crates/fleet-host/tauri.conf.json" >"$bad_product/tmp.json"
mv "$bad_product/tmp.json" "$bad_product/crates/fleet-host/tauri.conf.json"
expect_fail "product name mismatch is rejected" "$owner" "$bad_product"

bad_npm_owner="$TMPDIR/bad-npm-owner.md"
write_owner_record "$bad_npm_owner" APPROVED "fleet-bridge, other-extension"
expect_fail "npm package name omission is rejected" "$bad_npm_owner" "$valid_root"

bad_crate="$TMPDIR/bad-crate"
write_tree "$bad_crate"
cat >"$bad_crate/crates/fleet-cli/Cargo.toml" <<'EOF'
[package]
name = "other-cli"
EOF
expect_fail "Rust crate prefix mismatch is rejected" "$owner" "$bad_crate"

todo_owner="$TMPDIR/todo-owner.md"
write_owner_record "$todo_owner" APPROVED "fleet-extension, fleet-bridge or TODO"
expect_fail "non-concrete namespace decision is rejected" "$todo_owner" "$valid_root"

echo "Namespace decision check tests passed."
