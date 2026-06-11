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

### 13. Branding Stability

- [$([ "$checked" = "placeholders" ] && echo x || echo ' ')] \`Fleet\` name and current icon are alpha placeholders.
- [$([ "$checked" = "name-stable" ] && echo x || echo ' ')] \`Fleet\` name is stable, icon may change.
- [$([ "$checked" = "stable" ] && echo x || echo ' ')] Name and icon are stable.
- [$([ "$checked" = "other" ] && echo x || echo ' ')] Other: \`Private preview brand\`

## Required Before Binary Distribution
EOF
}

write_tree() {
  local root=$1
  mkdir -p "$root/crates/fleet-host/icons" "$root/docs/release"
  printf 'png-bytes\n' >"$root/crates/fleet-host/icons/icon.png"
  cat >"$root/crates/fleet-host/build.rs" <<'EOF'
fn main() {
    println!("cargo:rerun-if-changed=icons/icon.png");
}
EOF
  cat >"$root/crates/fleet-host/bundle.sh" <<'EOF'
"$HERE/scripts/refresh-icons.sh" --strict
# icons/icon.png so replacing that one file is enough.
EOF
  cat >"$root/crates/fleet-host/tauri.conf.json" <<'EOF'
{
  "bundle": {
    "icon": [
      "icons/128x128.png",
      "icons/Fleet.icns"
    ]
  }
}
EOF
  cat >"$root/docs/release/PUBLIC_ALPHA_DECISIONS.md" <<'EOF'
| Branding stability | Generated icon and possibly temporary `Fleet` name. | Public assets become recognizable quickly. | The release notes checker requires the decision to be stated. |
EOF
  cat >"$root/docs/release/ASSET_PROVENANCE.md" <<'EOF'
# Asset Provenance

Asset: crates/fleet-host/icons/icon.png
Type: app icon source PNG
Status: owner affirmed for alpha distribution
Provenance: generated original icon candidate from project prompt
Redistribution decision: distribute under the chosen project license for alpha, or replace in a later release.
EOF
  cat >"$root/docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md" <<'EOF'
# Fleet Alpha Release Notes Template

- Branding: `[alpha placeholders | Fleet name stable, icon may change | name and icon stable]`
EOF
}

expect_pass() {
  local label=$1
  shift
  if ! "$ROOT/scripts/check-branding-decision.sh" "$@" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  shift
  if "$ROOT/scripts/check-branding-decision.sh" "$@" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

owner_placeholders="$TMPDIR/owner-placeholders.md"
placeholders_root="$TMPDIR/placeholders-root"
write_owner_record "$owner_placeholders" APPROVED placeholders
write_tree "$placeholders_root"
expect_pass "template mode accepts canonical options" "$owner_placeholders" "$placeholders_root"

cat >"$placeholders_root/docs/release/notes.md" <<'EOF'
# Notes

- Branding: alpha placeholders
EOF
expect_pass "actual notes match alpha placeholders choice" "$owner_placeholders" "$placeholders_root" "docs/release/notes.md"

cat >"$placeholders_root/docs/release/notes-owner-wording.md" <<'EOF'
# Notes

- Branding: Fleet name and current icon are alpha placeholders
EOF
expect_pass "actual notes accept owner-record placeholder wording" "$owner_placeholders" "$placeholders_root" "docs/release/notes-owner-wording.md"

owner_name_stable="$TMPDIR/owner-name-stable.md"
name_stable_root="$TMPDIR/name-stable-root"
write_owner_record "$owner_name_stable" APPROVED name-stable
write_tree "$name_stable_root"
cat >"$name_stable_root/docs/release/notes.md" <<'EOF'
# Notes

- Branding: Fleet name stable, icon may change
EOF
expect_pass "actual notes match name-stable choice" "$owner_name_stable" "$name_stable_root" "docs/release/notes.md"

wrong_notes="$TMPDIR/wrong-notes-root"
write_tree "$wrong_notes"
cat >"$wrong_notes/docs/release/notes.md" <<'EOF'
# Notes

- Branding: name and icon stable
EOF
expect_fail "actual notes must match owner choice" "$owner_name_stable" "$wrong_notes" "docs/release/notes.md"

bad_template="$TMPDIR/bad-template-root"
write_tree "$bad_template"
printf -- '- Branding: `[alpha placeholders | name and icon stable]`\n' \
  >"$bad_template/docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md"
expect_fail "template must include icon-may-change option" "$owner_placeholders" "$bad_template"

missing_icon="$TMPDIR/missing-icon-root"
write_tree "$missing_icon"
rm "$missing_icon/crates/fleet-host/icons/icon.png"
expect_fail "missing replaceable icon source is rejected" "$owner_placeholders" "$missing_icon"

pending_provenance="$TMPDIR/pending-provenance-root"
write_tree "$pending_provenance"
perl -0pi -e 's/Status: owner affirmed for alpha distribution/Status: pending owner affirmation/; s/Redistribution decision: distribute under the chosen project license for alpha, or replace in a later release\./Redistribution decision: pending; replace or affirm before release./' \
  "$pending_provenance/docs/release/ASSET_PROVENANCE.md"
expect_fail "pending asset provenance is rejected" "$owner_placeholders" "$pending_provenance"

owner_pending="$TMPDIR/owner-pending.md"
write_owner_record "$owner_pending" PENDING placeholders
expect_fail "pending owner record is rejected" "$owner_pending" "$placeholders_root"

owner_other="$TMPDIR/owner-other.md"
other_root="$TMPDIR/other-root"
write_owner_record "$owner_other" APPROVED other
write_tree "$other_root"
cat >"$other_root/docs/release/notes.md" <<'EOF'
# Notes

- Branding: Private preview brand
EOF
expect_pass "concrete Other branding decision is accepted" "$owner_other" "$other_root" "docs/release/notes.md"

echo "Branding decision check tests passed."
