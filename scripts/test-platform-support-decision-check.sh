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

### 18. Supported Platform And Toolchain

- [$([ "$checked" = "macos" ] && echo x || echo ' ')] macOS source alpha only. Supported toolchain: Rust 1.78 or newer, Node.js 20/npm, Git, and user-provided VS Code code CLI/serve-web.
- [$([ "$checked" = "matrix" ] && echo x || echo ' ')] Publish a broader OS/toolchain support matrix before public alpha.
- [$([ "$checked" = "other" ] && echo x || echo ' ')] Other: \`Private macOS dogfood only\`

## Required Before Binary Distribution
EOF
}

write_macos_docs() {
  local root=$1
  mkdir -p "$root/docs/release"
  cat >"$root/docs/QUICKSTART.md" <<'EOF'
# Quickstart

## Prerequisites

- macOS.
- Rust 1.78 or newer.
- Node.js 20 and npm.
- Visual Studio Code with the `code` CLI available.
- Git.
EOF
  printf 'The alpha includes a macOS Tauri Fleet host and local `code serve-web` sessions.\n' \
    >"$root/README.md"
  printf 'Source builds and local macOS dogfooding are the intended alpha path.\n' \
    >"$root/SUPPORT.md"
  cat >"$root/docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" <<'EOF'
# Notes

## Supported Platform And Toolchain

- macOS source build only.
- Rust 1.78 or newer.
- Node.js 20 and npm.
- Git.
- user-provided VS Code `code` CLI.
- Linux, Windows, and remote/container workflows are not supported alpha platforms.
EOF
}

expect_pass() {
  local label=$1
  local owner=$2
  local root=$3
  if ! "$ROOT/scripts/check-platform-support-decision.sh" "$owner" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  local owner=$2
  local root=$3
  if "$ROOT/scripts/check-platform-support-decision.sh" "$owner" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

owner_macos="$TMPDIR/owner-macos.md"
macos_root="$TMPDIR/macos-root"
write_owner_record "$owner_macos" APPROVED macos
write_macos_docs "$macos_root"
expect_pass "macOS source-alpha toolchain docs are accepted" "$owner_macos" "$macos_root"

missing_node="$TMPDIR/missing-node"
write_macos_docs "$missing_node"
perl -0pi -e 's/- Node\.js 20 and npm\.\n//' "$missing_node/docs/QUICKSTART.md"
expect_fail "missing Node.js prerequisite is rejected" "$owner_macos" "$missing_node"

missing_unsupported="$TMPDIR/missing-unsupported"
write_macos_docs "$missing_unsupported"
perl -0pi -e 's/- Linux, Windows, and remote\/container workflows are not supported alpha platforms\.\n//' \
  "$missing_unsupported/docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md"
expect_fail "missing unsupported platform boundary is rejected" "$owner_macos" "$missing_unsupported"

owner_matrix="$TMPDIR/owner-matrix.md"
matrix_root="$TMPDIR/matrix-root"
write_owner_record "$owner_matrix" APPROVED matrix
write_macos_docs "$matrix_root"
cat >"$matrix_root/docs/release/PLATFORM_SUPPORT.md" <<'EOF'
# Platform Support

Supported platforms: macOS and Linux source builds.
Supported toolchains: Rust stable, Node.js 20/npm, Git, VS Code code CLI.
Unsupported platforms: Windows binaries and remote/container workflows.
EOF
expect_pass "concrete broader platform matrix is accepted" "$owner_matrix" "$matrix_root"

matrix_placeholder="$TMPDIR/matrix-placeholder"
write_macos_docs "$matrix_placeholder"
cat >"$matrix_placeholder/docs/release/PLATFORM_SUPPORT.md" <<'EOF'
Supported platforms: TODO
Supported toolchains: TODO
Unsupported platforms: TODO
EOF
expect_fail "placeholder platform matrix is rejected" "$owner_matrix" "$matrix_placeholder"

owner_other="$TMPDIR/owner-other.md"
other_root="$TMPDIR/other-root"
write_owner_record "$owner_other" APPROVED other
write_macos_docs "$other_root"
printf 'Supported platforms: private macOS dogfood only\n' >"$other_root/docs/release/PLATFORM_SUPPORT.md"
expect_pass "concrete Other platform policy is accepted" "$owner_other" "$other_root"

owner_pending="$TMPDIR/owner-pending.md"
write_owner_record "$owner_pending" PENDING macos
expect_fail "pending owner record is rejected" "$owner_pending" "$macos_root"

echo "Platform support decision check tests passed."
