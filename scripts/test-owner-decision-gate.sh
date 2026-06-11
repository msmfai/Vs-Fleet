#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

write_record() {
  local file=$1
  local status=$2
  local distribution=$3
  local binary=$4

  local source_checked=' '
  local unsigned_checked=' '
  local signing_checked=' '
  local update_checked=' '
  local branding_checked=' '

  case "$distribution" in
    source) source_checked='x' ;;
    unsigned) unsigned_checked='x' ;;
    *) echo "unknown distribution fixture: $distribution" >&2; exit 2 ;;
  esac

  if [ "$binary" = "decided" ]; then
    signing_checked='x'
    update_checked='x'
    branding_checked='x'
  elif [ "$binary" != "undecided" ]; then
    echo "unknown binary fixture: $binary" >&2
    exit 2
  fi

  cat >"$file" <<EOF
# Owner Decision Record

Decision record status: $status

## Required Before Public GitHub Visibility

### 1. License

- [x] MIT only.
- [ ] Other: \`TODO\`

### 2. Public History

- [x] Publish a cleaned/squashed history for the first public branch.
- [ ] Other: \`TODO\`

### 3. Public Namespace

| Surface | Decision |
|---|---|
| GitHub org/user | example |
| GitHub repo name | vs-fleet |
| Product name | Fleet |
| Rust crate prefix | fleet-* |
| npm package names | fleet-extension, fleet-bridge |
| VS Code Marketplace publisher | fleet-team |
| Open VSX publisher | fleet-team |
| macOS bundle id | dev.fleet.host |

### 4. Distribution Scope

- [$source_checked] Source-only alpha. No public app bundle, crates.io, npm, Open VSX, VS Code Marketplace, or container image publishing.
- [$unsigned_checked] Source plus unsigned macOS app bundle.
- [ ] Other: \`TODO\`

### 5. Security Reporting Channel

- [x] Enable GitHub Private Vulnerability Reporting.
- [ ] Other: \`TODO\`

### 6. Contribution Intake

- [x] Accept small focused PRs under the chosen project license using the PR template certification.
- [ ] Other: \`TODO\`

### 7. Public CI Evidence

- [x] Require GitHub Actions green on the exact branch/commit before public visibility.
- [ ] Other: \`TODO\`

### 8. Dependency Review Evidence

- [x] Run the dependency review commands in \`docs/release/DEPENDENCY_REVIEW.md\` and record findings in the release notes.
- [ ] Other: \`TODO\`

## Required Before Binary Distribution

### 9. macOS Signing and Notarization

- [$signing_checked] No public binaries until Developer ID signing and notarization are automated.
- [ ] Other: \`TODO\`

### 10. Update Channel

- [$update_checked] No auto-update in alpha.
- [ ] Other: \`TODO\`

### 11. Branding Stability

- [$branding_checked] \`Fleet\` name and current icon are alpha placeholders.
- [ ] Other: \`TODO\`
EOF
}

expect_pass() {
  local label=$1
  local file=$2
  if ! "$ROOT/scripts/check-owner-decisions.sh" "$file" >/tmp/fleet-owner-gate.out 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat /tmp/fleet-owner-gate.out >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  local file=$2
  if "$ROOT/scripts/check-owner-decisions.sh" "$file" >/tmp/fleet-owner-gate.out 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat /tmp/fleet-owner-gate.out >&2
    exit 1
  fi
}

source_only="$TMPDIR/source-only.md"
write_record "$source_only" APPROVED source undecided
expect_pass "source-only alpha does not require binary-only decisions" "$source_only"

binary_missing="$TMPDIR/binary-missing.md"
write_record "$binary_missing" APPROVED unsigned undecided
expect_fail "public app bundle requires signing/update/branding decisions" "$binary_missing"

binary_decided="$TMPDIR/binary-decided.md"
write_record "$binary_decided" APPROVED unsigned decided
expect_pass "public app bundle passes after binary-only decisions are recorded" "$binary_decided"

pending="$TMPDIR/pending.md"
write_record "$pending" PENDING source undecided
expect_fail "pending records are rejected" "$pending"

echo "Owner decision gate tests passed."
