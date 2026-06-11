#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

write_owner_record() {
  local file=$1
  local status=$2
  local product=${3:-"Fleet"}
  local bundle=${4:-"dev.fleet.host"}
  local npm_names=${5:-"fleet-extension, fleet-bridge"}
  local publisher=${6:-"fleet-team"}
  local rust_prefix=${7:-"fleet-*"}
  cat >"$file" <<EOF
# Owner Decision Record

Decision record status: $status

## Required Before Public GitHub Visibility

### 3. Public Namespace

| Surface | Decision |
|---|---|
| GitHub org/user | smfmarin |
| GitHub repo name | vs-fleet |
| Product name | $product |
| Rust crate prefix | $rust_prefix |
| npm package names | $npm_names |
| VS Code Marketplace publisher | $publisher |
| Open VSX publisher | $publisher |
| macOS bundle id | $bundle |

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
  "productName": "Old Fleet",
  "identifier": "old.bundle.id"
}
EOF
  cat >"$root/crates/fleet-host/bundle.sh" <<'EOF'
echo '  <key>CFBundleName</key><string>Old Fleet</string>'
echo '  <key>CFBundleDisplayName</key><string>Old Fleet</string>'
echo '  <key>CFBundleIdentifier</key><string>old.bundle.id</string>'
EOF
  cat >"$root/crates/fleet-host/src/spawn.rs" <<'EOF'
fn fleet_bridge_installed(name: &str) -> bool {
    name.starts_with("fleet-team.fleet-bridge-")
}

#[test]
fn bridge_installed_detects_fleet_bridge_directory() {
    std::fs::create_dir_all(dir.join("fleet-team.fleet-bridge-0.2.0")).unwrap();
}
EOF
  cat >"$root/packages/fleet-bridge/package.json" <<'EOF'
{
  "name": "fleet-bridge",
  "displayName": "Fleet Bridge",
  "publisher": "old-team"
}
EOF
  cat >"$root/packages/extension/package.json" <<'EOF'
{
  "name": "fleet-extension",
  "displayName": "Fleet",
  "publisher": "old-team"
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

expect_fail() {
  local label=$1
  shift
  if "$ROOT/scripts/apply-namespace-decision.sh" "$@" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

owner="$TMPDIR/owner.md"
repo="$TMPDIR/repo"
write_owner_record "$owner" APPROVED "Fleet Alpha" "dev.fleet.alpha" "fleet-alpha-extension, fleet-alpha-bridge" "fleet-labs"
write_tree "$repo"

if ! "$ROOT/scripts/apply-namespace-decision.sh" "$owner" "$repo" >"$TMPDIR/apply.out" 2>&1; then
  echo "FAIL: expected namespace application to pass" >&2
  cat "$TMPDIR/apply.out" >&2
  exit 1
fi

test "$(jq -r '.productName' "$repo/crates/fleet-host/tauri.conf.json")" = "Fleet Alpha"
test "$(jq -r '.identifier' "$repo/crates/fleet-host/tauri.conf.json")" = "dev.fleet.alpha"
test "$(jq -r '.name' "$repo/packages/fleet-bridge/package.json")" = "fleet-alpha-bridge"
test "$(jq -r '.publisher' "$repo/packages/fleet-bridge/package.json")" = "fleet-labs"
test "$(jq -r '.displayName' "$repo/packages/fleet-bridge/package.json")" = "Fleet Alpha Bridge"
test "$(jq -r '.name' "$repo/packages/extension/package.json")" = "fleet-alpha-extension"
test "$(jq -r '.publisher' "$repo/packages/extension/package.json")" = "fleet-labs"
test "$(jq -r '.displayName' "$repo/packages/extension/package.json")" = "Fleet Alpha"
test "$(jq -r '.packages[""].name' "$repo/packages/fleet-bridge/package-lock.json")" = "fleet-alpha-bridge"
test "$(jq -r '.packages[""].name' "$repo/packages/extension/package-lock.json")" = "fleet-alpha-extension"
rg -q '<key>CFBundleName</key><string>Fleet Alpha</string>' "$repo/crates/fleet-host/bundle.sh"
rg -q '<key>CFBundleDisplayName</key><string>Fleet Alpha</string>' "$repo/crates/fleet-host/bundle.sh"
rg -q '<key>CFBundleIdentifier</key><string>dev.fleet.alpha</string>' "$repo/crates/fleet-host/bundle.sh"
rg -q 'starts_with\("fleet-labs\.fleet-alpha-bridge-"\)' "$repo/crates/fleet-host/src/spawn.rs"
rg -q 'fleet-labs\.fleet-alpha-bridge-0\.2\.0' "$repo/crates/fleet-host/src/spawn.rs"

if ! "$ROOT/scripts/check-namespace-decision.sh" "$owner" "$repo" >"$TMPDIR/check.out" 2>&1; then
  echo "FAIL: namespace checker should pass after application" >&2
  cat "$TMPDIR/check.out" >&2
  exit 1
fi

if ! "$ROOT/scripts/apply-namespace-decision.sh" "$owner" "$repo" >"$TMPDIR/reapply.out" 2>&1; then
  echo "FAIL: namespace application should be idempotent" >&2
  cat "$TMPDIR/reapply.out" >&2
  exit 1
fi
rg -q 'starts_with\("fleet-labs\.fleet-alpha-bridge-"\)' "$repo/crates/fleet-host/src/spawn.rs"

pending="$TMPDIR/pending.md"
pending_repo="$TMPDIR/pending-repo"
write_owner_record "$pending" PENDING
write_tree "$pending_repo"
expect_fail "pending owner record is rejected" "$pending" "$pending_repo"

bad_npm="$TMPDIR/bad-npm.md"
bad_npm_repo="$TMPDIR/bad-npm-repo"
write_owner_record "$bad_npm" APPROVED "Fleet" "dev.fleet.host" "fleet-one, fleet-two"
write_tree "$bad_npm_repo"
expect_fail "ambiguous npm package role mapping is rejected" "$bad_npm" "$bad_npm_repo"

bad_crate_owner="$TMPDIR/bad-crate-owner.md"
bad_crate="$TMPDIR/bad-crate"
write_owner_record "$bad_crate_owner" APPROVED "Fleet" "dev.fleet.host" "fleet-extension, fleet-bridge" "fleet-team" "other-*"
write_tree "$bad_crate"
expect_fail "Rust crate prefix rename is not automatic" "$bad_crate_owner" "$bad_crate"

echo "Namespace decision applier tests passed."
