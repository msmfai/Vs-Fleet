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

### 6. Distribution Scope

- [$([ "$checked" = "source" ] && echo x || echo ' ')] Source-only alpha. No public app bundle, crates.io, npm, Open VSX, VS Code
  Marketplace, or container image publishing.
- [$([ "$checked" = "unsigned" ] && echo x || echo ' ')] Source plus unsigned macOS app bundle.
- [$([ "$checked" = "signed" ] && echo x || echo ' ')] Source plus signed/notarized macOS app bundle.
- [$([ "$checked" = "other" ] && echo x || echo ' ')] Other: \`Private source snapshot only\`

### 7. Security Reporting Channel

### 13. Branding Stability

- [x] \`Fleet\` name and current icon are alpha placeholders.

## Required Before Binary Distribution

### 21. macOS Signing and Notarization

- [$([ "$checked" = "source" ] && echo ' ' || echo x)] Publish unsigned binaries and document Gatekeeper warnings.

### 22. Update Channel

- [$([ "$checked" = "source" ] && echo ' ' || echo x)] No auto-update in alpha.
EOF
}

write_source_tree() {
  local root=$1
  for crate in fleet-cli fleet-e2e fleet-host-core fleet-host fleet-hub fleet-protocol fleet-reporter; do
    mkdir -p "$root/crates/$crate"
  done
  for package in fleet-bridge extension; do
    mkdir -p "$root/packages/$package"
  done
  mkdir -p "$root/docs/release"
  for crate_dir in "$root"/crates/*; do
    printf '[package]\npublish = false\n' >"$crate_dir/Cargo.toml"
  done
  printf '{"private":true}\n' >"$root/packages/fleet-bridge/package.json"
  printf '{"private":true}\n' >"$root/packages/extension/package.json"
  printf 'This process is for a source-only public alpha.\n' >"$root/docs/release/RELEASE_PROCESS.md"
  printf 'This is the current source-only alpha path.\n' >"$root/docs/QUICKSTART.md"
  printf 'Package publication: `[none for source-only alpha | explicit approved scope]`\n' \
    >"$root/docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md"
}

write_binary_process() {
  local root=$1
  mkdir -p "$root/docs/release"
  cat >"$root/docs/release/BINARY_RELEASE_PROCESS.md" <<'EOF'
# Binary Release Process

Document Gatekeeper warnings for unsigned builds, or Developer ID signing and
notarization for signed builds. Generate SHA256 checksums for release assets.
Describe upgrade and rollback expectations.
EOF
}

expect_pass() {
  local label=$1
  local owner=$2
  local root=$3
  if ! "$ROOT/scripts/check-distribution-decision.sh" "$owner" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  local owner=$2
  local root=$3
  if "$ROOT/scripts/check-distribution-decision.sh" "$owner" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

owner_source="$TMPDIR/owner-source.md"
source_root="$TMPDIR/source-root"
write_owner_record "$owner_source" APPROVED source
write_source_tree "$source_root"
expect_pass "source-only fences are accepted" "$owner_source" "$source_root"

bad_publish_root="$TMPDIR/bad-publish-root"
write_source_tree "$bad_publish_root"
printf '[package]\npublish = true\n' >"$bad_publish_root/crates/fleet-hub/Cargo.toml"
expect_fail "source-only Rust publish drift is rejected" "$owner_source" "$bad_publish_root"

bad_private_root="$TMPDIR/bad-private-root"
write_source_tree "$bad_private_root"
printf '{"private":false}\n' >"$bad_private_root/packages/extension/package.json"
expect_fail "source-only npm publication drift is rejected" "$owner_source" "$bad_private_root"

artifact_root="$TMPDIR/artifact-root"
write_source_tree "$artifact_root"
mkdir -p "$artifact_root/crates/fleet-host/Fleet.app/Contents"
printf 'binary\n' >"$artifact_root/crates/fleet-host/Fleet.app/Contents/file"
expect_fail "tracked/generated app bundle is rejected" "$owner_source" "$artifact_root"

owner_unsigned="$TMPDIR/owner-unsigned.md"
binary_root="$TMPDIR/binary-root"
write_owner_record "$owner_unsigned" APPROVED unsigned
write_source_tree "$binary_root"
write_binary_process "$binary_root"
expect_pass "unsigned app bundle scope requires binary process" "$owner_unsigned" "$binary_root"

missing_binary_process="$TMPDIR/missing-binary-process"
write_source_tree "$missing_binary_process"
expect_fail "binary distribution without process doc is rejected" "$owner_unsigned" "$missing_binary_process"

owner_other="$TMPDIR/owner-other.md"
other_root="$TMPDIR/other-root"
write_owner_record "$owner_other" APPROVED other
write_source_tree "$other_root"
printf 'Distribution scope: private source snapshot only\n' >"$other_root/docs/release/DISTRIBUTION_SCOPE.md"
expect_pass "concrete Other distribution scope is accepted" "$owner_other" "$other_root"

owner_pending="$TMPDIR/owner-pending.md"
write_owner_record "$owner_pending" PENDING source
expect_fail "pending owner record is rejected" "$owner_pending" "$source_root"

echo "Distribution decision check tests passed."
