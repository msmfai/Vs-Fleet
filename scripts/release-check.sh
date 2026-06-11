#!/usr/bin/env bash
set -euo pipefail

fail=0

check_absent() {
  local pattern=$1
  local description=$2
  shift 2
  if rg -n "$pattern" "$@" >/tmp/fleet-release-check.$$ 2>/dev/null; then
    echo "FAIL: $description"
    sed -n '1,40p' /tmp/fleet-release-check.$$
    fail=1
  fi
  rm -f /tmp/fleet-release-check.$$
}

if [ ! -f LICENSE ]; then
  echo "FAIL: missing root LICENSE"
  fail=1
fi

check_absent 'license\s*=\s*"UNLICENSED"|"license"\s*:\s*"UNLICENSED"' \
  "package manifests still declare UNLICENSED" \
  Cargo.toml crates packages

check_absent '/Users/|/private/tmp|/var/folders|C:\\\\Users\\\\' \
  "tracked release-facing text artifacts contain local absolute paths" \
  --glob '!target/**' \
  --glob '!**/node_modules/**' \
  --glob '!**/out/**' \
  --glob '!**/*.png' \
  --glob '!**/*.jpg' \
  --glob '!**/*.jpeg' \
  --glob '!**/*.icns' \
  crates/fleet-host/artifacts \
  containers/fleet-env/eval/artifacts \
  packages/extension

if git ls-files | rg '(^|/)coverage/|(^|/)node_modules/|(^|/)out/|\.vsix$|Fleet\.app/' >/tmp/fleet-release-check.$$; then
  echo "FAIL: generated dependency/build outputs are tracked"
  sed -n '1,80p' /tmp/fleet-release-check.$$
  fail=1
fi
rm -f /tmp/fleet-release-check.$$

if [ ! -f SECURITY.md ]; then
  echo "FAIL: missing SECURITY.md"
  fail=1
fi

if [ ! -f CONTRIBUTING.md ]; then
  echo "FAIL: missing CONTRIBUTING.md"
  fail=1
fi

if [ "$fail" -ne 0 ]; then
  echo
  echo "Release check failed. See docs/release/PUBLIC_ALPHA_DECISIONS.md."
  exit 1
fi

echo "Release check passed."
