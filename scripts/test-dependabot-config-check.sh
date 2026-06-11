#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

write_config() {
  local file=$1
  local host_interval=${2:-weekly}
  local include_host=${3:-yes}

  cat >"$file" <<EOF
version: 2
updates:
  - package-ecosystem: "github-actions"
    directory: "/"
    schedule:
      interval: "weekly"

  - package-ecosystem: "cargo"
    directory: "/"
    schedule:
      interval: "weekly"
EOF

  if [ "$include_host" = "yes" ]; then
    cat >>"$file" <<EOF

  - package-ecosystem: "cargo"
    directory: "/crates/fleet-host"
    schedule:
      interval: "$host_interval"
EOF
  fi

  cat >>"$file" <<EOF

  - package-ecosystem: "npm"
    directory: "/packages/fleet-bridge"
    schedule:
      interval: "weekly"

  - package-ecosystem: "npm"
    directory: "/packages/extension"
    schedule:
      interval: "weekly"
EOF
}

expect_pass() {
  local label=$1
  local file=$2
  if ! "$ROOT/scripts/check-dependabot-config.sh" "$file" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  local file=$2
  if "$ROOT/scripts/check-dependabot-config.sh" "$file" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

valid="$TMPDIR/dependabot-valid.yml"
write_config "$valid"
expect_pass "required ecosystems are configured weekly" "$valid"

missing_host="$TMPDIR/dependabot-missing-host.yml"
write_config "$missing_host" weekly no
expect_fail "standalone host cargo workspace is required" "$missing_host"

monthly_host="$TMPDIR/dependabot-monthly-host.yml"
write_config "$monthly_host" monthly yes
expect_fail "required ecosystems must be weekly" "$monthly_host"

bad_version="$TMPDIR/dependabot-bad-version.yml"
write_config "$bad_version"
sed -i.bak 's/^version: 2$/version: 1/' "$bad_version"
expect_fail "version 2 is required" "$bad_version"

expect_fail "missing config is rejected" "$TMPDIR/missing.yml"

echo "Dependabot config check tests passed."
