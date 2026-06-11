#!/usr/bin/env bash
set -euo pipefail

owner_record="${1:-docs/release/OWNER_DECISION_RECORD.md}"
root="${2:-.}"
fail=0

if [ ! -f "$owner_record" ]; then
  echo "FAIL: missing owner decision record: $owner_record"
  exit 1
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "FAIL: jq is required for namespace manifest checks"
  exit 1
fi

if ! rg -q '^Decision record status: APPROVED$' "$owner_record"; then
  echo "FAIL: owner decision record is not approved"
  exit 1
fi

namespace_block="$(
  sed -n '/^### 3\. Public Namespace$/,/^### 4\. Alpha Scope$/p' "$owner_record"
)"

decision_for() {
  local surface=$1
  printf '%s\n' "$namespace_block" |
    awk -F'|' -v surface="$surface" '
      function trim(s) {
        gsub(/^[[:space:]]+|[[:space:]]+$/, "", s)
        return s
      }
      trim($2) == surface {
        value = trim($3)
        gsub(/^`|`$/, "", value)
        print value
        found = 1
        exit
      }
      END { if (!found) exit 1 }
    '
}

load_decision() {
  local surface=$1
  local var_name=$2
  local value
  if ! value="$(decision_for "$surface")"; then
    echo "FAIL: Public Namespace table missing decision for $surface"
    fail=1
    return
  fi
  if [ -z "$value" ] || [[ "$value" == *TODO* ]] || [[ "$value" == *" or "* ]]; then
    echo "FAIL: Public Namespace decision for $surface is not concrete: $value"
    fail=1
    return
  fi
  printf -v "$var_name" '%s' "$value"
}

github_org=""
github_repo=""
product_name=""
rust_prefix=""
npm_names=""
marketplace_publisher=""
openvsx_publisher=""
macos_bundle_id=""

load_decision "GitHub org/user" github_org
load_decision "GitHub repo name" github_repo
load_decision "Product name" product_name
load_decision "Rust crate prefix" rust_prefix
load_decision "npm package names" npm_names
load_decision "VS Code Marketplace publisher" marketplace_publisher
load_decision "Open VSX publisher" openvsx_publisher
load_decision "macOS bundle id" macos_bundle_id

if [ "$fail" -ne 0 ]; then
  exit 1
fi

check_json_value() {
  local file=$1
  local query=$2
  local expected=$3
  local label=$4
  if [ ! -f "$root/$file" ]; then
    echo "FAIL: missing $file"
    fail=1
    return
  fi
  local value
  value="$(jq -r "$query // \"\"" "$root/$file")"
  if [ "$value" != "$expected" ]; then
    echo "FAIL: $file $label is \"$value\", expected \"$expected\""
    fail=1
  fi
}

check_cargo_package_name() {
  local file=$1
  local prefix=$2
  local name
  name="$(sed -n '/^\[package\]/,/^\[/p' "$root/$file" | sed -n 's/^name[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1)"
  if [ -z "$name" ]; then
    echo "FAIL: could not read package name from $file"
    fail=1
  elif [[ "$name" != "$prefix"* ]]; then
    echo "FAIL: $file package name \"$name\" does not match Rust crate prefix \"$rust_prefix\""
    fail=1
  fi
}

check_json_value "crates/fleet-host/tauri.conf.json" '.productName' "$product_name" "productName"
check_json_value "crates/fleet-host/tauri.conf.json" '.identifier' "$macos_bundle_id" "identifier"

if [ ! -f "$root/crates/fleet-host/bundle.sh" ]; then
  echo "FAIL: missing crates/fleet-host/bundle.sh"
  fail=1
else
  if ! rg -F -q "<key>CFBundleName</key><string>$product_name</string>" "$root/crates/fleet-host/bundle.sh"; then
    echo "FAIL: crates/fleet-host/bundle.sh CFBundleName does not match \"$product_name\""
    fail=1
  fi
  if ! rg -F -q "<key>CFBundleDisplayName</key><string>$product_name</string>" "$root/crates/fleet-host/bundle.sh"; then
    echo "FAIL: crates/fleet-host/bundle.sh CFBundleDisplayName does not match \"$product_name\""
    fail=1
  fi
  if ! rg -F -q "<key>CFBundleIdentifier</key><string>$macos_bundle_id</string>" "$root/crates/fleet-host/bundle.sh"; then
    echo "FAIL: crates/fleet-host/bundle.sh CFBundleIdentifier does not match \"$macos_bundle_id\""
    fail=1
  fi
fi

IFS=',' read -r -a npm_expected <<<"$npm_names"
if [ "${#npm_expected[@]}" -ne 2 ]; then
  echo "FAIL: npm package names must contain exactly two comma-separated names"
  fail=1
else
  bridge_name="$(jq -r '.name // ""' "$root/packages/fleet-bridge/package.json")"
  extension_name="$(jq -r '.name // ""' "$root/packages/extension/package.json")"
  expected_names=" $(printf '%s\n' "${npm_expected[@]}" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//; s/^`//; s/`$//' | tr '\n' ' ')"
  for actual in "$bridge_name" "$extension_name"; do
    if [[ "$expected_names " != *" $actual "* ]]; then
      echo "FAIL: npm package name \"$actual\" is not listed in owner decision \"$npm_names\""
      fail=1
    fi
  done
  check_json_value "packages/fleet-bridge/package-lock.json" '.packages[""].name' "$bridge_name" "root package name"
  check_json_value "packages/extension/package-lock.json" '.packages[""].name' "$extension_name" "root package name"
fi

for manifest in "$root"/crates/*/Cargo.toml; do
  rel="${manifest#"$root"/}"
  check_cargo_package_name "$rel" "${rust_prefix%\*}"
done

for manifest in \
  packages/fleet-bridge/package.json \
  packages/extension/package.json
do
  check_json_value "$manifest" '.publisher' "$marketplace_publisher" "publisher"
done

if [ -f "$root/crates/fleet-host/src/spawn.rs" ] && [ -n "${bridge_name:-}" ]; then
  bridge_extension_prefix="$marketplace_publisher.$bridge_name-"
  if ! rg -F -q "starts_with(\"$bridge_extension_prefix\")" "$root/crates/fleet-host/src/spawn.rs"; then
    echo "FAIL: crates/fleet-host/src/spawn.rs bridge extension detection prefix does not match \"$bridge_extension_prefix\""
    fail=1
  fi
fi

if [ "$marketplace_publisher" != "$openvsx_publisher" ]; then
  echo "WARN: Open VSX publisher \"$openvsx_publisher\" differs from VS Code Marketplace publisher \"$marketplace_publisher\"; no separate Open VSX manifest exists to verify"
fi

if [ -n "$github_org" ] && [ -n "$github_repo" ]; then
  :
fi

if [ "$fail" -ne 0 ]; then
  exit 1
fi

echo "Namespace decision check passed."
