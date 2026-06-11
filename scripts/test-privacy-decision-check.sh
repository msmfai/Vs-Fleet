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

### 8. Privacy And Telemetry Posture

- [$([ "$checked" = "local" ] && echo x || echo ' ')] No telemetry by default. Local logs and artifacts may contain workspace
  paths, local URLs, session labels, process command lines, and editor state;
  users must scrub them before sharing.
- [$([ "$checked" = "telemetry" ] && echo x || echo ' ')] Add an explicit telemetry or remote reporting disclosure before public
  visibility.
- [$([ "$checked" = "other" ] && echo x || echo ' ')] Other: \`Private deployment only\`

### 9. Dependency Review Evidence
EOF
}

write_default_docs() {
  local root=$1
  mkdir -p "$root/docs/release" "$root/.github/ISSUE_TEMPLATE"
  cat >"$root/README.md" <<'EOF'
Fleet is local-first and has no intended telemetry by default.
It can log workspace paths, local URLs, session labels, process command lines,
and editor state. Scrub logs and review artifacts before sharing.
EOF
  cat >"$root/SECURITY.md" <<'EOF'
Logs can contain workspace paths, local URLs, session labels, and command-line metadata.
Logs and review artifacts should be scrubbed before sharing publicly.
EOF
  printf 'Fleet is local-first and has no intended telemetry by default.\n' >"$root/docs/ARCHITECTURE.md"
  printf 'Please scrub workspace paths, local URLs, logs, screenshots, and command lines before posting.\n' \
    >"$root/.github/ISSUE_TEMPLATE/bug_report.yml"
  printf 'Fleet is local-first and has no intended telemetry by default.\n' \
    >"$root/docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md"
}

expect_pass() {
  local label=$1
  local owner=$2
  local root=$3
  if ! "$ROOT/scripts/check-privacy-decision.sh" "$owner" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  local owner=$2
  local root=$3
  if "$ROOT/scripts/check-privacy-decision.sh" "$owner" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

owner_local="$TMPDIR/owner-local.md"
local_root="$TMPDIR/local-root"
write_owner_record "$owner_local" APPROVED local
write_default_docs "$local_root"
expect_pass "local-first no-telemetry docs are accepted" "$owner_local" "$local_root"

missing_log_disclosure="$TMPDIR/missing-log-disclosure"
write_default_docs "$missing_log_disclosure"
printf 'Fleet has no intended telemetry by default.\n' >"$missing_log_disclosure/README.md"
expect_fail "missing logging disclosures are rejected" "$owner_local" "$missing_log_disclosure"

owner_telemetry="$TMPDIR/owner-telemetry.md"
telemetry_root="$TMPDIR/telemetry-root"
write_owner_record "$owner_telemetry" APPROVED telemetry
write_default_docs "$telemetry_root"
cat >"$telemetry_root/PRIVACY.md" <<'EOF'
Telemetry: disabled by default.
Remote reporting: none unless explicitly configured by the user.
EOF
expect_pass "explicit telemetry disclosure is accepted" "$owner_telemetry" "$telemetry_root"

telemetry_placeholder="$TMPDIR/telemetry-placeholder"
write_default_docs "$telemetry_placeholder"
cat >"$telemetry_placeholder/PRIVACY.md" <<'EOF'
Telemetry: TODO
Remote reporting: TODO
EOF
expect_fail "placeholder telemetry disclosure is rejected" "$owner_telemetry" "$telemetry_placeholder"

owner_other="$TMPDIR/owner-other.md"
other_root="$TMPDIR/other-root"
write_owner_record "$owner_other" APPROVED other
write_default_docs "$other_root"
mkdir -p "$other_root/docs/release"
printf 'Privacy posture: private deployment only\n' >"$other_root/docs/release/PRIVACY_POSTURE.md"
expect_pass "concrete Other privacy posture is accepted" "$owner_other" "$other_root"

owner_pending="$TMPDIR/owner-pending.md"
write_owner_record "$owner_pending" PENDING local
expect_fail "pending owner record is rejected" "$owner_pending" "$local_root"

echo "Privacy decision check tests passed."
