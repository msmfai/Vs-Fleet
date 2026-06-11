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
  mkdir -p "$root/crates/fleet-cli" "$root/crates/fleet-host/src" "$root/packages/fleet-bridge" "$root/packages/extension"
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
  cat >"$root/crates/fleet-host/bundle.sh" <<'EOF'
echo '  <key>CFBundleName</key><string>Fleet</string>'
echo '  <key>CFBundleDisplayName</key><string>Fleet</string>'
echo '  <key>CFBundleIdentifier</key><string>dev.fleet.host</string>'
EOF
  cat >"$root/crates/fleet-host/src/spawn.rs" <<'EOF'
fn fleet_bridge_installed(name: &str) -> bool {
    name.starts_with("fleet-team.fleet-bridge-")
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
  cat >"$root/packages/fleet-bridge/package-lock.json" <<'EOF'
{
  "packages": {
    "": {
      "name": "fleet-bridge"
    }
  }
}
EOF
  cat >"$root/packages/extension/package-lock.json" <<'EOF'
{
  "packages": {
    "": {
      "name": "fleet-extension"
    }
  }
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

bad_bundle_script="$TMPDIR/bad-bundle-script"
write_tree "$bad_bundle_script"
perl -0pi -e 's/dev\.fleet\.host/other.bundle.id/' "$bad_bundle_script/crates/fleet-host/bundle.sh"
expect_fail "bundle script namespace mismatch is rejected" "$owner" "$bad_bundle_script"

bad_npm_owner="$TMPDIR/bad-npm-owner.md"
write_owner_record "$bad_npm_owner" APPROVED "fleet-bridge, other-extension"
expect_fail "npm package name omission is rejected" "$bad_npm_owner" "$valid_root"

bad_lock="$TMPDIR/bad-lock"
write_tree "$bad_lock"
jq '.packages[""].name = "other-bridge"' "$bad_lock/packages/fleet-bridge/package-lock.json" >"$bad_lock/tmp.json"
mv "$bad_lock/tmp.json" "$bad_lock/packages/fleet-bridge/package-lock.json"
expect_fail "npm package-lock root name mismatch is rejected" "$owner" "$bad_lock"

bad_prefix="$TMPDIR/bad-prefix"
write_tree "$bad_prefix"
perl -0pi -e 's/fleet-team\.fleet-bridge-/other-team.fleet-bridge-/' "$bad_prefix/crates/fleet-host/src/spawn.rs"
expect_fail "bridge extension detection prefix mismatch is rejected" "$owner" "$bad_prefix"

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

bad_github_owner="$TMPDIR/bad-github-owner.md"
cp "$owner" "$bad_github_owner"
perl -0pi -e 's/\| GitHub org\/user \| smfmarin \|/\| GitHub org\/user \| bad owner \|/' "$bad_github_owner"
expect_fail "invalid GitHub owner syntax is rejected" "$bad_github_owner" "$valid_root"

bad_github_repo="$TMPDIR/bad-github-repo.md"
cp "$owner" "$bad_github_repo"
perl -0pi -e 's/\| GitHub repo name \| vs-fleet \|/\| GitHub repo name \| bad\/repo \|/' "$bad_github_repo"
expect_fail "invalid GitHub repo syntax is rejected" "$bad_github_repo" "$valid_root"

bad_npm_syntax="$TMPDIR/bad-npm-syntax.md"
write_owner_record "$bad_npm_syntax" APPROVED "Fleet-extension, fleet-bridge"
expect_fail "invalid npm package name syntax is rejected" "$bad_npm_syntax" "$valid_root"

bad_publisher="$TMPDIR/bad-publisher.md"
cp "$owner" "$bad_publisher"
perl -0pi -e 's/\| VS Code Marketplace publisher \| fleet-team \|/\| VS Code Marketplace publisher \| fleet team \|/' "$bad_publisher"
expect_fail "invalid publisher syntax is rejected" "$bad_publisher" "$valid_root"

bad_bundle_id="$TMPDIR/bad-bundle-id.md"
cp "$owner" "$bad_bundle_id"
perl -0pi -e 's/\| macOS bundle id \| dev\.fleet\.host \|/\| macOS bundle id \| dev fleet host \|/' "$bad_bundle_id"
expect_fail "invalid bundle id syntax is rejected" "$bad_bundle_id" "$valid_root"

echo "Namespace decision check tests passed."
