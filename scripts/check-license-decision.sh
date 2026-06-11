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
  echo "FAIL: jq is required for npm manifest license checks"
  exit 1
fi

if ! rg -q '^Decision record status: APPROVED$' "$owner_record"; then
  echo "FAIL: owner decision record is not approved"
  exit 1
fi

license_block="$(
  sed -n '/^### 1\. License$/,/^### 2\. Public History$/p' "$owner_record"
)"
checked_count="$(printf '%s\n' "$license_block" | rg -c '^- \[x\] ' || true)"
checked_count="${checked_count:-0}"
if [ "$checked_count" -ne 1 ]; then
  echo "FAIL: license decision must have exactly one checked choice; found $checked_count"
  exit 1
fi

checked="$(printf '%s\n' "$license_block" | rg '^- \[x\] ' | head -n1)"
case "$checked" in
  "- [x] MIT OR Apache-2.0 dual license.") expected="MIT OR Apache-2.0" ;;
  "- [x] MIT only.") expected="MIT" ;;
  "- [x] Apache-2.0 only.") expected="Apache-2.0" ;;
  "- [x] AGPL-3.0-only.") expected="AGPL-3.0-only" ;;
  "- [x] Other: "*)
    expected="$(printf '%s\n' "$checked" | sed -n 's/^- \[x\] Other: `\(.*\)`$/\1/p')"
    if [ -z "$expected" ] || [ "$expected" = "TODO" ]; then
      echo "FAIL: checked Other license decision must contain a concrete SPDX expression"
      exit 1
    fi
    ;;
  *)
    echo "FAIL: unsupported license decision: $checked"
    exit 1
    ;;
esac

check_file() {
  local file=$1
  if [ ! -f "$root/$file" ]; then
    echo "FAIL: missing $file"
    fail=1
  fi
}

check_cargo_license() {
  local file=$1
  check_file "$file"
  if [ -f "$root/$file" ] && ! rg -q -F "license = \"$expected\"" "$root/$file"; then
    echo "FAIL: $file license does not match owner decision $expected"
    fail=1
  fi
}

check_json_license() {
  local file=$1
  check_file "$file"
  if [ -f "$root/$file" ]; then
    local value
    value="$(jq -r '.license // ""' "$root/$file")"
    if [ "$value" != "$expected" ]; then
      echo "FAIL: $file license is \"$value\", expected \"$expected\""
      fail=1
    fi
  fi
}

check_lock_root_license() {
  local file=$1
  check_file "$file"
  if [ -f "$root/$file" ]; then
    local value
    value="$(jq -r '.packages[""].license // ""' "$root/$file")"
    if [ "$value" != "$expected" ]; then
      echo "FAIL: $file root package license is \"$value\", expected \"$expected\""
      fail=1
    fi
  fi
}

if [ ! -s "$root/LICENSE" ]; then
  echo "FAIL: root LICENSE is missing or empty"
  fail=1
fi

check_cargo_license "Cargo.toml"
check_cargo_license "crates/fleet-host/Cargo.toml"
check_json_license "packages/fleet-bridge/package.json"
check_json_license "packages/extension/package.json"
check_lock_root_license "packages/fleet-bridge/package-lock.json"
check_lock_root_license "packages/extension/package-lock.json"

if [ "$fail" -ne 0 ]; then
  exit 1
fi

echo "License decision check passed: $expected"
