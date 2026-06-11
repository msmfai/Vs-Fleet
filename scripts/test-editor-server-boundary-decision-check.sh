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

### 5. Editor Server Licensing Boundary

- [$([ "$checked" = "user" ] && echo x || echo ' ')] User-provided VS Code only. Fleet may launch the user's local
  \`code serve-web\` install, but Fleet does not download, bundle, host, or
  redistribute Microsoft's VS Code Server, Microsoft Marketplace extensions, or
  Microsoft remote extensions.
- [$([ "$checked" = "oss" ] && echo x || echo ' ')] OSS server only. Supported workflows use \`code-server\` or
  \`openvscode-server\` with Open VSX; no Microsoft VS Code Server or Marketplace
  dependency.
- [$([ "$checked" = "other" ] && echo x || echo ' ')] Other: \`Private editor boundary\`

### 6. Distribution Scope
EOF
}

write_user_vscode_docs() {
  local root=$1
  mkdir -p "$root/docs/release"
  cat >"$root/README.md" <<'EOF'
Fleet uses the user's local `code serve-web` install.
Fleet does not download, bundle, host, or redistribute Microsoft's VS Code Server.
It also does not redistribute Microsoft Marketplace extensions.
EOF
  cat >"$root/docs/QUICKSTART.md" <<'EOF'
Fleet's source alpha uses the user's local `code serve-web`.
Fleet does not download, bundle, host, or redistribute Microsoft's VS Code Server
or Microsoft Marketplace extensions.
EOF
  cat >"$root/docs/ARCHITECTURE.md" <<'EOF'
The source alpha boundary is user-provided VS Code through local code serve-web.
Fleet does not download, bundle, host, or redistribute Microsoft's VS Code Server
or Microsoft Marketplace extensions.
EOF
  cat >"$root/docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" <<'EOF'
Editor server boundary: user's local code serve-web only.
Fleet does not redistribute Microsoft's VS Code Server.
EOF
}

expect_pass() {
  local label=$1
  local owner=$2
  local root=$3
  if ! "$ROOT/scripts/check-editor-server-boundary-decision.sh" "$owner" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  local owner=$2
  local root=$3
  if "$ROOT/scripts/check-editor-server-boundary-decision.sh" "$owner" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

owner_user="$TMPDIR/owner-user.md"
user_root="$TMPDIR/user-root"
write_owner_record "$owner_user" APPROVED user
write_user_vscode_docs "$user_root"
expect_pass "user-provided VS Code boundary is accepted" "$owner_user" "$user_root"

missing_marketplace="$TMPDIR/missing-marketplace"
write_user_vscode_docs "$missing_marketplace"
printf "Fleet uses the user's local code serve-web.\nFleet does not redistribute VS Code Server.\n" \
  >"$missing_marketplace/README.md"
expect_fail "Marketplace boundary is required" "$owner_user" "$missing_marketplace"

owner_oss="$TMPDIR/owner-oss.md"
oss_root="$TMPDIR/oss-root"
write_owner_record "$owner_oss" APPROVED oss
write_user_vscode_docs "$oss_root"
cat >"$oss_root/docs/release/EDITOR_SERVER_BOUNDARY.md" <<'EOF'
Editor server boundary: supported workflows use code-server or openvscode-server.
Open VSX is the extension registry. There is no Microsoft VS Code Server.
EOF
expect_pass "OSS server boundary is accepted" "$owner_oss" "$oss_root"

oss_placeholder="$TMPDIR/oss-placeholder"
write_user_vscode_docs "$oss_placeholder"
printf 'Editor server boundary: TODO\n' >"$oss_placeholder/docs/release/EDITOR_SERVER_BOUNDARY.md"
expect_fail "placeholder OSS boundary is rejected" "$owner_oss" "$oss_placeholder"

owner_other="$TMPDIR/owner-other.md"
other_root="$TMPDIR/other-root"
write_owner_record "$owner_other" APPROVED other
write_user_vscode_docs "$other_root"
printf 'Editor server boundary: private deployment only\n' >"$other_root/docs/release/EDITOR_SERVER_BOUNDARY.md"
expect_pass "concrete Other boundary is accepted" "$owner_other" "$other_root"

owner_pending="$TMPDIR/owner-pending.md"
write_owner_record "$owner_pending" PENDING user
expect_fail "pending owner record is rejected" "$owner_pending" "$user_root"

echo "Editor server boundary decision check tests passed."
