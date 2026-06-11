#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
usage: scripts/apply-namespace-decision.sh [owner-record] [repo-root]

Apply the approved Public Namespace owner decision to release metadata:
  - crates/fleet-host/tauri.conf.json productName and identifier
  - crates/fleet-host/bundle.sh CFBundle name/display name/identifier
  - VS Code extension package names, publishers, display names, and lockfile root names
  - fleet-host bridge-extension install detection prefix

Rust crate names are not renamed automatically. The selected Rust crate prefix
must already match every crates/*/Cargo.toml package name; otherwise this script
fails and leaves the crate rename as an explicit migration.
EOF
}

owner_record="${1:-docs/release/OWNER_DECISION_RECORD.md}"
root="${2:-.}"

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  exit 2
fi

if [ ! -f "$owner_record" ]; then
  echo "FAIL: missing owner decision record: $owner_record" >&2
  exit 1
fi

if [ ! -d "$root" ]; then
  echo "FAIL: repo root does not exist: $root" >&2
  exit 1
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "FAIL: jq is required to update namespace manifests" >&2
  exit 1
fi

if ! rg -q '^Decision record status: APPROVED$' "$owner_record"; then
  echo "FAIL: owner decision record is not approved" >&2
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
    echo "FAIL: Public Namespace table missing decision for $surface" >&2
    exit 1
  fi
  if [ -z "$value" ] || [[ "$value" == *TODO* ]] || [[ "$value" == *" or "* ]]; then
    echo "FAIL: Public Namespace decision for $surface is not concrete: $value" >&2
    exit 1
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

require_file() {
  local file=$1
  if [ ! -f "$root/$file" ]; then
    echo "FAIL: missing $file" >&2
    exit 1
  fi
}

trim_npm_name() {
  printf '%s' "$1" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//; s/^`//; s/`$//'
}

IFS=',' read -r -a raw_npm_names <<<"$npm_names"
if [ "${#raw_npm_names[@]}" -ne 2 ]; then
  echo "FAIL: npm package names must contain exactly two comma-separated names" >&2
  exit 1
fi

bridge_npm_name=""
extension_npm_name=""
for raw_name in "${raw_npm_names[@]}"; do
  name="$(trim_npm_name "$raw_name")"
  if [ -z "$name" ]; then
    echo "FAIL: npm package names must not contain empty entries" >&2
    exit 1
  fi
  case "$name" in
    *bridge*)
      if [ -n "$bridge_npm_name" ]; then
        echo "FAIL: npm package names contain more than one bridge package: $npm_names" >&2
        exit 1
      fi
      bridge_npm_name="$name"
      ;;
    *extension*)
      if [ -n "$extension_npm_name" ]; then
        echo "FAIL: npm package names contain more than one extension package: $npm_names" >&2
        exit 1
      fi
      extension_npm_name="$name"
      ;;
  esac
done

if [ -z "$bridge_npm_name" ] || [ -z "$extension_npm_name" ]; then
  echo "FAIL: npm package names must include one bridge name and one extension name: $npm_names" >&2
  exit 1
fi

crate_prefix="${rust_prefix%\*}"
for manifest in "$root"/crates/*/Cargo.toml; do
  if [ ! -f "$manifest" ]; then
    continue
  fi
  name="$(sed -n '/^\[package\]/,/^\[/p' "$manifest" | sed -n 's/^name[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1)"
  if [ -z "$name" ]; then
    echo "FAIL: could not read package name from ${manifest#"$root"/}" >&2
    exit 1
  fi
  if [[ "$name" != "$crate_prefix"* ]]; then
    echo "FAIL: ${manifest#"$root"/} package name \"$name\" does not match Rust crate prefix \"$rust_prefix\"; crate renames are not applied automatically" >&2
    exit 1
  fi
done

update_json() {
  local file=$1
  shift
  require_file "$file"
  local tmp="$root/$file.$$"
  jq "$@" "$root/$file" >"$tmp"
  mv "$tmp" "$root/$file"
}

update_json "crates/fleet-host/tauri.conf.json" \
  --arg product "$product_name" --arg bundle "$macos_bundle_id" \
  '.productName = $product | .identifier = $bundle'

require_file "crates/fleet-host/bundle.sh"
PRODUCT_NAME="$product_name" MACOS_BUNDLE_ID="$macos_bundle_id" perl -0pi -e '
  my $product = $ENV{PRODUCT_NAME};
  my $bundle = $ENV{MACOS_BUNDLE_ID};
  s|(<key>CFBundleName</key><string>)[^<]*(</string>)|$1$product$2|g;
  s|(<key>CFBundleDisplayName</key><string>)[^<]*(</string>)|$1$product$2|g;
  s|(<key>CFBundleIdentifier</key><string>)[^<]*(</string>)|$1$bundle$2|g;
' "$root/crates/fleet-host/bundle.sh"

update_package_manifest() {
  local file=$1
  local package_name=$2
  local display_name=$3
  update_json "$file" \
    --arg name "$package_name" --arg publisher "$marketplace_publisher" --arg display "$display_name" \
    '.name = $name | .publisher = $publisher | .displayName = $display'
}

update_package_lock() {
  local file=$1
  local package_name=$2
  update_json "$file" --arg name "$package_name" '.packages[""].name = $name'
}

update_package_manifest "packages/fleet-bridge/package.json" "$bridge_npm_name" "$product_name Bridge"
update_package_manifest "packages/extension/package.json" "$extension_npm_name" "$product_name"
update_package_lock "packages/fleet-bridge/package-lock.json" "$bridge_npm_name"
update_package_lock "packages/extension/package-lock.json" "$extension_npm_name"

bridge_extension_prefix="$marketplace_publisher.$bridge_npm_name-"
require_file "crates/fleet-host/src/spawn.rs"
BRIDGE_EXTENSION_PREFIX="$bridge_extension_prefix" perl -0pi -e '
  my $prefix = $ENV{BRIDGE_EXTENSION_PREFIX};
  s/"[^"]+\.[^"]+-0\.2\.0"/"$prefix" . "0.2.0"/ge;
  s/starts_with\("[^"]+\.[^"]+-"\)/starts_with("$prefix")/g;
' "$root/crates/fleet-host/src/spawn.rs"

echo "Applied namespace metadata:"
echo "  product: $product_name"
echo "  bundle id: $macos_bundle_id"
echo "  bridge package: $bridge_npm_name"
echo "  extension package: $extension_npm_name"
echo "  marketplace publisher: $marketplace_publisher"
if [ "$marketplace_publisher" != "$openvsx_publisher" ]; then
  echo "  note: Open VSX publisher is $openvsx_publisher; no separate Open VSX manifest exists"
fi
echo "  GitHub: $github_org/$github_repo"
