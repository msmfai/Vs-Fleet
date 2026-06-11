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

### 14. Versioning And Compatibility

- [$([ "$checked" = "alpha" ] && echo x || echo ' ')] Alpha pre-release tags only. No stable API, protocol, state-file, or upgrade compatibility is promised during alpha.
- [$([ "$checked" = "semver" ] && echo x || echo ' ')] Commit to semver-compatible public CLI, protocol, and state changes during alpha.
- [$([ "$checked" = "other" ] && echo x || echo ' ')] Other: \`Private preview compatibility only\`

## Required Before Binary Distribution
EOF
}

write_alpha_docs() {
  local root=$1
  mkdir -p "$root/docs/release"
  cat >"$root/docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" <<'EOF'
# Fleet Alpha Release Notes Template

- Version: `[v0.1.0-alpha.1]`

Not supported as a public alpha commitment:

- production support, stable APIs, or backwards-compatible state formats.

## Upgrade And Rollback

- No stable upgrade path is promised during alpha.
- No auto-update channel is enabled unless explicitly approved in the owner decision record.
EOF
  cat >"$root/SUPPORT.md" <<'EOF'
# Support

There are no stable release lines yet.
EOF
  cat >"$root/SECURITY.md" <<'EOF'
# Security

There are no stable release lines yet.
EOF
  cat >"$root/docs/release/RELEASE_PROCESS.md" <<'EOF'
# Release Process

git tag -s v0.1.0-alpha.1 -m "Fleet v0.1.0-alpha.1"

Review upgrade and rollback expectations before release.
EOF
}

expect_pass() {
  local label=$1
  local owner=$2
  local root=$3
  if ! "$ROOT/scripts/check-versioning-decision.sh" "$owner" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  local owner=$2
  local root=$3
  if "$ROOT/scripts/check-versioning-decision.sh" "$owner" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

owner_alpha="$TMPDIR/owner-alpha.md"
alpha_root="$TMPDIR/alpha-root"
write_owner_record "$owner_alpha" APPROVED alpha
write_alpha_docs "$alpha_root"
expect_pass "alpha unstable versioning docs are accepted" "$owner_alpha" "$alpha_root"

missing_upgrade="$TMPDIR/missing-upgrade"
write_alpha_docs "$missing_upgrade"
perl -0pi -e 's/No stable upgrade path is promised during alpha\.//' "$missing_upgrade/docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md"
expect_fail "missing no-upgrade promise is rejected" "$owner_alpha" "$missing_upgrade"

owner_semver="$TMPDIR/owner-semver.md"
semver_root="$TMPDIR/semver-root"
write_owner_record "$owner_semver" APPROVED semver
write_alpha_docs "$semver_root"
cat >"$semver_root/docs/release/VERSIONING.md" <<'EOF'
# Versioning

Versioning commitment: semver-compatible public CLI, protocol, and state changes.
Compatibility scope: documented CLI flags, protocol schema, and state files.
Migration policy: breaking changes include migration notes.
EOF
expect_pass "semver compatibility policy is accepted" "$owner_semver" "$semver_root"

semver_placeholder="$TMPDIR/semver-placeholder"
write_alpha_docs "$semver_placeholder"
cat >"$semver_placeholder/docs/release/VERSIONING.md" <<'EOF'
Versioning commitment: TODO
Compatibility scope: TODO
Migration policy: TODO
EOF
expect_fail "placeholder semver policy is rejected" "$owner_semver" "$semver_placeholder"

owner_other="$TMPDIR/owner-other.md"
other_root="$TMPDIR/other-root"
write_owner_record "$owner_other" APPROVED other
write_alpha_docs "$other_root"
printf 'Versioning commitment: private preview compatibility only\n' >"$other_root/docs/release/VERSIONING.md"
expect_pass "concrete Other versioning policy is accepted" "$owner_other" "$other_root"

owner_pending="$TMPDIR/owner-pending.md"
write_owner_record "$owner_pending" PENDING alpha
expect_fail "pending owner record is rejected" "$owner_pending" "$alpha_root"

echo "Versioning decision check tests passed."
