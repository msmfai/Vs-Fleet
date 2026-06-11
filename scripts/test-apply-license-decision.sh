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

### 1. License

- [$([ "$checked" = "dual" ] && echo x || echo ' ')] MIT OR Apache-2.0 dual license.
- [$([ "$checked" = "mit" ] && echo x || echo ' ')] MIT only.
- [$([ "$checked" = "apache" ] && echo x || echo ' ')] Apache-2.0 only.
- [$([ "$checked" = "agpl" ] && echo x || echo ' ')] AGPL-3.0-only.
- [ ] Other: \`TODO\`

### 2. Public History
EOF
}

write_unlicensed_tree() {
  local root=$1
  mkdir -p "$root/crates/fleet-host" "$root/packages/fleet-bridge" "$root/packages/extension"
  cat >"$root/Cargo.toml" <<'EOF'
[workspace.package]
license = "UNLICENSED"
EOF
  cat >"$root/crates/fleet-host/Cargo.toml" <<'EOF'
[package]
license = "UNLICENSED"
EOF
  cat >"$root/packages/fleet-bridge/package.json" <<'EOF'
{"name":"fleet-bridge","license":"UNLICENSED"}
EOF
  cat >"$root/packages/extension/package.json" <<'EOF'
{"name":"fleet-extension","license":"UNLICENSED"}
EOF
  cat >"$root/packages/fleet-bridge/package-lock.json" <<'EOF'
{"packages":{"":{"name":"fleet-bridge","license":"UNLICENSED"}}}
EOF
  cat >"$root/packages/extension/package-lock.json" <<'EOF'
{"packages":{"":{"name":"fleet-extension","license":"UNLICENSED"}}}
EOF
}

expect_fail() {
  local label=$1
  shift
  if "$@" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

license_text="$TMPDIR/LICENSE"
printf 'Fixture license text\n' >"$license_text"

owner_dual="$TMPDIR/owner-dual.md"
write_owner_record "$owner_dual" APPROVED dual

repo="$TMPDIR/repo"
write_unlicensed_tree "$repo"

if ! "$ROOT/scripts/apply-license-decision.sh" "$owner_dual" "$repo" "$license_text" >"$TMPDIR/apply.out" 2>&1; then
  echo "FAIL: expected license application to pass" >&2
  cat "$TMPDIR/apply.out" >&2
  exit 1
fi

if ! rg -q 'Applied license metadata: MIT OR Apache-2.0' "$TMPDIR/apply.out"; then
  echo "FAIL: apply output did not name the applied SPDX expression" >&2
  cat "$TMPDIR/apply.out" >&2
  exit 1
fi

if ! cmp -s "$license_text" "$repo/LICENSE"; then
  echo "FAIL: supplied license file was not copied to root LICENSE" >&2
  exit 1
fi

if ! "$ROOT/scripts/check-license-decision.sh" "$owner_dual" "$repo" >"$TMPDIR/check.out" 2>&1; then
  echo "FAIL: applied license metadata should satisfy license gate" >&2
  cat "$TMPDIR/check.out" >&2
  exit 1
fi

for file in \
  "$repo/Cargo.toml" \
  "$repo/crates/fleet-host/Cargo.toml" \
  "$repo/packages/fleet-bridge/package.json" \
  "$repo/packages/extension/package.json" \
  "$repo/packages/fleet-bridge/package-lock.json" \
  "$repo/packages/extension/package-lock.json"
do
  if rg -q 'UNLICENSED' "$file"; then
    echo "FAIL: $file still contains UNLICENSED" >&2
    cat "$file" >&2
    exit 1
  fi
done

repo_existing="$TMPDIR/repo-existing"
write_unlicensed_tree "$repo_existing"
printf 'Existing license\n' >"$repo_existing/LICENSE"
if ! "$ROOT/scripts/apply-license-decision.sh" "$owner_dual" "$repo_existing" >"$TMPDIR/existing.out" 2>&1; then
  echo "FAIL: expected existing root LICENSE to be accepted" >&2
  cat "$TMPDIR/existing.out" >&2
  exit 1
fi

owner_pending="$TMPDIR/owner-pending.md"
write_owner_record "$owner_pending" PENDING dual
repo_pending="$TMPDIR/repo-pending"
write_unlicensed_tree "$repo_pending"
expect_fail "pending owner record is rejected" \
  "$ROOT/scripts/apply-license-decision.sh" "$owner_pending" "$repo_pending" "$license_text"

repo_no_license="$TMPDIR/repo-no-license"
write_unlicensed_tree "$repo_no_license"
expect_fail "missing root license text is rejected" \
  "$ROOT/scripts/apply-license-decision.sh" "$owner_dual" "$repo_no_license"

repo_bad="$TMPDIR/repo-bad"
write_unlicensed_tree "$repo_bad"
rm "$repo_bad/packages/extension/package-lock.json"
expect_fail "missing package lock is rejected" \
  "$ROOT/scripts/apply-license-decision.sh" "$owner_dual" "$repo_bad" "$license_text"

echo "License application tests passed."
