#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

write_owner_record() {
  local file=$1
  local status=$2
  local checked=$3
  local product=${4:-Fleet}
  cat >"$file" <<EOF
# Owner Decision Record

Decision record status: $status

## Required Before Public GitHub Visibility

### 3. Public Namespace

| Surface | Decision |
|---|---|
| GitHub org/user | example |
| GitHub repo name | vs-fleet |
| Product name | $product |
| Rust crate prefix | fleet-* |
| npm package names | fleet-extension, fleet-bridge |
| VS Code Marketplace publisher | fleet-team |
| Open VSX publisher | fleet-team |
| macOS bundle id | dev.fleet.host |

### 4. Alpha Scope

### 20. Public Name Collision And Trademark Posture

- [$([ "$checked" = "provisional" ] && echo x || echo ' ')] Use \`Fleet\` only as a provisional source-alpha working name. Make no trademark claim, acknowledge name-collision review is unresolved, and do not publish packages or binaries under stable Fleet namespaces.
- [$([ "$checked" = "rename" ] && echo x || echo ' ')] Rename the product and package namespaces before public visibility.
- [$([ "$checked" = "clearance" ] && echo x || echo ' ')] Owner has reviewed name/trademark clearance and accepts using \`Fleet\` publicly.
- [$([ "$checked" = "other" ] && echo x || echo ' ')] Other: \`Private codename only\`

## Required Before Binary Distribution
EOF
}

write_provisional_docs() {
  local root=$1
  mkdir -p "$root/docs/release"
  cat >"$root/README.md" <<'EOF'
Fleet is a provisional source-alpha working name.
This repository makes no trademark claim to the name.
Stable package or binary publication under Fleet namespaces is deferred.
EOF
  cat >"$root/docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" <<'EOF'
# Notes

## Naming And Trademark Posture

- `Fleet` is a provisional source-alpha working name.
- This release makes no trademark claim to the `Fleet` name.
- Stable package or binary publication under Fleet namespaces is deferred until
  the owner completes the public name decision.
EOF
  cat >"$root/docs/release/PUBLIC_ALPHA_DECISIONS.md" <<'EOF'
| Public name collision and trademark posture | Fleet is provisional. |
EOF
  cat >"$root/docs/release/GITHUB_PUBLICATION_RUNBOOK.md" <<'EOF'
Public name collision/trademark posture has been recorded.
EOF
  cat >"$root/docs/release/NAME_COLLISION_REVIEW.md" <<'EOF'
Status: no trademark clearance claim.

Known collision: JetBrains has used `Fleet` for a developer IDE/product.
Stable package or binary publication under Fleet namespaces is deferred.
EOF
}

expect_pass() {
  local label=$1
  local owner=$2
  local root=$3
  if ! "$ROOT/scripts/check-name-collision-decision.sh" "$owner" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  local owner=$2
  local root=$3
  if "$ROOT/scripts/check-name-collision-decision.sh" "$owner" "$root" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

owner_provisional="$TMPDIR/owner-provisional.md"
provisional_root="$TMPDIR/provisional-root"
write_owner_record "$owner_provisional" APPROVED provisional
write_provisional_docs "$provisional_root"
expect_pass "provisional source-alpha name docs are accepted" "$owner_provisional" "$provisional_root"

missing_no_claim="$TMPDIR/missing-no-claim"
write_provisional_docs "$missing_no_claim"
perl -0pi -e 's/This repository makes no trademark claim to the name\.\n//' "$missing_no_claim/README.md"
expect_fail "missing no-trademark-claim wording is rejected" "$owner_provisional" "$missing_no_claim"

owner_rename="$TMPDIR/owner-rename.md"
rename_root="$TMPDIR/rename-root"
write_owner_record "$owner_rename" APPROVED rename "HarborDeck"
write_provisional_docs "$rename_root"
cat >"$rename_root/docs/release/NAME_COLLISION_REVIEW.md" <<'EOF'
Selected public name: HarborDeck
Rename scope: product name, package names, bundle id, and public docs before visibility.
EOF
expect_pass "rename before public visibility is accepted with non-Fleet namespace" "$owner_rename" "$rename_root"

owner_rename_bad="$TMPDIR/owner-rename-bad.md"
write_owner_record "$owner_rename_bad" APPROVED rename Fleet
expect_fail "rename decision rejects unchanged Fleet product name" "$owner_rename_bad" "$rename_root"

owner_clearance="$TMPDIR/owner-clearance.md"
clearance_root="$TMPDIR/clearance-root"
write_owner_record "$owner_clearance" APPROVED clearance
write_provisional_docs "$clearance_root"
cat >"$clearance_root/docs/release/NAME_COLLISION_REVIEW.md" <<'EOF'
Selected public name: Fleet
Clearance review date: 2026-06-11
Reviewed collision: JetBrains Fleet developer IDE/product.
Owner decision: accept public source-alpha use of Fleet.
EOF
expect_pass "owner-reviewed Fleet clearance is accepted" "$owner_clearance" "$clearance_root"

owner_other="$TMPDIR/owner-other.md"
other_root="$TMPDIR/other-root"
write_owner_record "$owner_other" APPROVED other
write_provisional_docs "$other_root"
printf 'Owner decision: private codename only\n' >"$other_root/docs/release/NAME_COLLISION_REVIEW.md"
expect_pass "concrete Other naming policy is accepted" "$owner_other" "$other_root"

owner_pending="$TMPDIR/owner-pending.md"
write_owner_record "$owner_pending" PENDING provisional
expect_fail "pending owner record is rejected" "$owner_pending" "$provisional_root"

echo "Name collision decision check tests passed."
