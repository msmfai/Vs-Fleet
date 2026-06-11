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

### 4. Alpha Scope

- [$([ "$checked" = "local" ] && echo x || echo ' ')] Local macOS Fleet host plus local \`code serve-web\` sessions, Fleet bridge,
  Fleet reporter, CLI, and embedded local Hub. Remote, SSH, Docker/container,
  visual probe, and eval harness paths remain development infrastructure, not
  public support commitments.
- [$([ "$checked" = "broad" ] && echo x || echo ' ')] Broaden public alpha scope to include remote, SSH, Docker/container, or
  eval harness paths as supported user workflows.
- [$([ "$checked" = "other" ] && echo x || echo ' ')] Other: \`Private source review only\`

### 5. Distribution Scope
EOF
}

write_local_scope_docs() {
  local root=$1
  mkdir -p "$root/docs/release"
  cat >"$root/README.md" <<'EOF'
# Fleet

A macOS Tauri Fleet host embeds local `code serve-web` sessions.
The Fleet bridge and reporter participate in the supported alpha workflow.
Remote/container deployment is not a supported alpha path.
EOF
  cat >"$root/docs/QUICKSTART.md" <<'EOF'
# Quickstart

This local macOS source alpha uses the user's local `code serve-web`.
Remote, SSH, and container modes are not supported alpha commitments.
EOF
  cat >"$root/docs/ARCHITECTURE.md" <<'EOF'
# Architecture

## Supported Alpha Surface

- macOS Fleet host.
- Local `code serve-web` sessions.
- Fleet bridge extension.
- Fleet reporter process.
- Embedded local Hub.

Remote, SSH, Docker/container, visual probe, and eval harness paths are not
public support commitments.
EOF
  cat >"$root/docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" <<'EOF'
# Notes

- local macOS source builds
- local `code serve-web` sessions
- container/remote deployment as a supported user path is excluded
EOF
}

expect_pass() {
  local label=$1
  local owner=$2
  local root=$3
  if ! "$ROOT/scripts/check-alpha-scope-decision.sh" "$owner" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  local owner=$2
  local root=$3
  if "$ROOT/scripts/check-alpha-scope-decision.sh" "$owner" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

owner_local="$TMPDIR/owner-local.md"
local_root="$TMPDIR/local-root"
write_owner_record "$owner_local" APPROVED local
write_local_scope_docs "$local_root"
expect_pass "local macOS source alpha docs are accepted" "$owner_local" "$local_root"

missing_boundary="$TMPDIR/missing-boundary"
write_local_scope_docs "$missing_boundary"
printf '# Quickstart\n\nlocal macOS and local code serve-web only\n' >"$missing_boundary/docs/QUICKSTART.md"
expect_fail "missing remote/container boundary is rejected" "$owner_local" "$missing_boundary"

owner_broad="$TMPDIR/owner-broad.md"
broad_root="$TMPDIR/broad-root"
write_owner_record "$owner_broad" APPROVED broad
write_local_scope_docs "$broad_root"
printf 'Alpha scope: remote SSH and Docker workflows are supported.\n' >"$broad_root/docs/release/ALPHA_SCOPE.md"
expect_pass "broadened scope requires explicit scope doc" "$owner_broad" "$broad_root"

broad_placeholder="$TMPDIR/broad-placeholder"
write_local_scope_docs "$broad_placeholder"
printf 'Alpha scope: TODO\n' >"$broad_placeholder/docs/release/ALPHA_SCOPE.md"
expect_fail "placeholder broadened scope is rejected" "$owner_broad" "$broad_placeholder"

owner_other="$TMPDIR/owner-other.md"
other_root="$TMPDIR/other-root"
write_owner_record "$owner_other" APPROVED other
write_local_scope_docs "$other_root"
printf 'Alpha scope: private source review only\n' >"$other_root/docs/release/ALPHA_SCOPE.md"
expect_pass "concrete Other scope is accepted" "$owner_other" "$other_root"

owner_pending="$TMPDIR/owner-pending.md"
write_owner_record "$owner_pending" PENDING local
expect_fail "pending owner record is rejected" "$owner_pending" "$local_root"

echo "Alpha scope decision check tests passed."
