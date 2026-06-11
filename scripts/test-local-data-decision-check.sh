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

### 21. Local Data And Uninstall Policy

- [$([ "$checked" = "manual" ] && echo x || echo ' ')] Document local data locations and manual cleanup for source alpha. Fleet does not promise an automated uninstaller, but public docs identify \`~/.fleet/run\`, \`~/.fleet/mux\`, cleanup commands, and the process ownership boundary.
- [$([ "$checked" = "automated" ] && echo x || echo ' ')] Add an automated cleanup or uninstall command before public visibility.
- [$([ "$checked" = "other" ] && echo x || echo ' ')] Other: \`Private cleanup policy\`

## Required Before Binary Distribution
EOF
}

write_manual_docs() {
  local root=$1
  mkdir -p "$root/docs/release"
  cat >"$root/docs/LOCAL_DATA_AND_UNINSTALL.md" <<'EOF'
# Local Data And Uninstall

`~/.fleet/run` stores Hub runtime files.
`~/.fleet/mux` stores spawned editor data.
FLEET_RUNTIME_DIR changes runtime storage.
FLEET_MUX_DIR changes mux storage.
Quitting Fleet must not kill external servers.
Closing a Fleet-spawned server from the Fleet UI is the explicit action.
Manual cleanup:
rm -rf ~/.fleet/run ~/.fleet/mux
This does not remove the user's VS Code installation or repositories outside the Fleet runtime.
EOF
  cat >"$root/README.md" <<'EOF'
See docs/LOCAL_DATA_AND_UNINSTALL.md.
Runtime data lives under `~/.fleet/run` and `~/.fleet/mux`.
EOF
  cat >"$root/docs/QUICKSTART.md" <<'EOF'
# Quickstart

## Cleanup

Close any Fleet-spawned servers from the Fleet UI.
rm -rf ~/.fleet/run ~/.fleet/mux
If FLEET_RUNTIME_DIR or FLEET_MUX_DIR was set, delete those configured directories instead.
EOF
  cat >"$root/docs/ARCHITECTURE.md" <<'EOF'
# Architecture

## Local Data And Cleanup

Quitting Fleet does not promise to delete spawned editor userdata or logs.
Fleet must not kill externally registered sessions.
EOF
  cat >"$root/SECURITY.md" <<'EOF'
# Security

Source-alpha runtime files live under `~/.fleet/run` and `~/.fleet/mux`.
Manual cleanup is documented in docs/LOCAL_DATA_AND_UNINSTALL.md.
EOF
  cat >"$root/docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" <<'EOF'
# Notes

## Local Data And Cleanup

rm -rf ~/.fleet/run ~/.fleet/mux
EOF
  cat >"$root/docs/release/PUBLIC_ALPHA_DECISIONS.md" <<'EOF'
| Local data and uninstall policy | Manual cleanup is documented. |
EOF
}

expect_pass() {
  local label=$1
  local owner=$2
  local root=$3
  if ! "$ROOT/scripts/check-local-data-decision.sh" "$owner" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  local owner=$2
  local root=$3
  if "$ROOT/scripts/check-local-data-decision.sh" "$owner" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

owner_manual="$TMPDIR/owner-manual.md"
manual_root="$TMPDIR/manual-root"
write_owner_record "$owner_manual" APPROVED manual
write_manual_docs "$manual_root"
expect_pass "manual local data cleanup docs are accepted" "$owner_manual" "$manual_root"

missing_cleanup="$TMPDIR/missing-cleanup"
write_manual_docs "$missing_cleanup"
perl -0pi -e 's/rm -rf ~\/\.fleet\/run ~\/\.fleet\/mux\n//' "$missing_cleanup/docs/LOCAL_DATA_AND_UNINSTALL.md"
expect_fail "missing cleanup command is rejected" "$owner_manual" "$missing_cleanup"

owner_automated="$TMPDIR/owner-automated.md"
automated_root="$TMPDIR/automated-root"
write_owner_record "$owner_automated" APPROVED automated
write_manual_docs "$automated_root"
cat >"$automated_root/docs/LOCAL_DATA_AND_UNINSTALL.md" <<'EOF'
# Local Data And Uninstall

Automated cleanup command: scripts/fleet-cleanup.sh
rm -rf ~/.fleet/run ~/.fleet/mux
EOF
mkdir -p "$automated_root/scripts"
printf '#!/usr/bin/env bash\n# cleanup implementation\n' >"$automated_root/scripts/fleet-cleanup.sh"
expect_pass "automated cleanup decision requires command docs and implementation marker" "$owner_automated" "$automated_root"

owner_other="$TMPDIR/owner-other.md"
other_root="$TMPDIR/other-root"
write_owner_record "$owner_other" APPROVED other
write_manual_docs "$other_root"
printf 'Owner decision: private cleanup policy\n' >"$other_root/docs/LOCAL_DATA_AND_UNINSTALL.md"
expect_pass "concrete Other local-data policy is accepted" "$owner_other" "$other_root"

owner_pending="$TMPDIR/owner-pending.md"
write_owner_record "$owner_pending" PENDING manual
expect_fail "pending owner record is rejected" "$owner_pending" "$manual_root"

echo "Local data decision check tests passed."
