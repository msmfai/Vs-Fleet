#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

write_record() {
  local file=$1
  local status=$2
  local distribution=$3
  local branding=$4
  local binary=$5

  local source_checked=' '
  local unsigned_checked=' '
  local signing_checked=' '
  local update_checked=' '
  local branding_checked=' '
  local versioning_checked='x'
  local community_checked='x'
  local custody_checked='x'

  case "$distribution" in
    source) source_checked='x' ;;
    unsigned) unsigned_checked='x' ;;
    *) echo "unknown distribution fixture: $distribution" >&2; exit 2 ;;
  esac

  if [ "$branding" = "decided" ]; then
    branding_checked='x'
  elif [ "$branding" != "undecided" ]; then
    echo "unknown branding fixture: $branding" >&2
    exit 2
  fi

  if [ "$binary" = "decided" ]; then
    signing_checked='x'
    update_checked='x'
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

### 4. Alpha Scope

- [x] Local macOS Fleet host plus local \`code serve-web\` sessions, Fleet bridge,
  Fleet reporter, CLI, and embedded local Hub. Remote, SSH, Docker/container,
  visual probe, and eval harness paths remain development infrastructure, not
  public support commitments.
- [ ] Other: \`TODO\`

### 5. Editor Server Licensing Boundary

- [x] User-provided VS Code only. Fleet may launch the user's local
  \`code serve-web\` install, but Fleet does not download, bundle, host, or
  redistribute Microsoft's VS Code Server, Microsoft Marketplace extensions, or
  Microsoft remote extensions.
- [ ] Other: \`TODO\`

### 6. Distribution Scope

- [$source_checked] Source-only alpha. No public app bundle, crates.io, npm, Open VSX, VS Code Marketplace, or container image publishing.
- [$unsigned_checked] Source plus unsigned macOS app bundle.
- [ ] Other: \`TODO\`

### 7. Security Reporting Channel

- [x] Enable GitHub Private Vulnerability Reporting.
- [ ] Other: \`TODO\`

### 8. Contribution Intake

- [x] Accept small focused PRs under the chosen project license using the PR template certification.
- [ ] Other: \`TODO\`

### 9. Public CI Evidence

- [x] Require GitHub Actions green on the exact branch/commit before public visibility.
- [ ] Other: \`TODO\`

### 10. Privacy And Telemetry Posture

- [x] No telemetry by default. Local logs and artifacts may contain workspace paths, local URLs, session labels, process command lines, and editor state; users must scrub them before sharing.
- [ ] Other: \`TODO\`

### 11. Dependency Review Evidence

- [x] Run the dependency review commands in \`docs/release/DEPENDENCY_REVIEW.md\` and record findings in the release notes.
- [ ] Other: \`TODO\`

### 12. Support Commitment

- [x] Best-effort alpha support only. Breaking changes are expected; there are
  no production support guarantees, response SLAs, paid support terms, or stable
  release lines.
- [ ] Other: \`TODO\`

### 13. Branding Stability

- [$branding_checked] \`Fleet\` name and current icon are alpha placeholders.
- [ ] Other: \`TODO\`

### 14. Versioning And Compatibility

- [$versioning_checked] Alpha pre-release tags only. No stable API, protocol, state-file, or upgrade compatibility is promised during alpha.
- [ ] Other: \`TODO\`

### 15. Community Intake And Moderation

- [$community_checked] Open public issues only for scoped bug reports and alpha feedback; keep blank issues disabled and keep discussions off unless explicitly enabled later.
- [ ] Other: \`TODO\`

### 16. Release Custody And Maintainer Authority

- [$custody_checked] Single-maintainer alpha. Only the repository owner or named maintainer may push release tags, create GitHub releases, change repository settings, or publish packages.
- [ ] Other: \`TODO\`

## Required Before Binary Distribution

### 17. macOS Signing and Notarization

- [$signing_checked] No public binaries until Developer ID signing and notarization are automated.
- [ ] Other: \`TODO\`

### 18. Update Channel

- [$update_checked] No auto-update in alpha.
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

expect_fail_contains() {
  local label=$1
  local file=$2
  local pattern=$3
  expect_fail "$label" "$file"
  if ! rg -q "$pattern" /tmp/fleet-owner-gate.out; then
    echo "FAIL: expected failure output for $label to contain: $pattern" >&2
    cat /tmp/fleet-owner-gate.out >&2
    exit 1
  fi
}

source_only="$TMPDIR/source-only.md"
write_record "$source_only" APPROVED source decided undecided
expect_pass "source-only alpha does not require binary-only decisions" "$source_only"

branding_missing="$TMPDIR/branding-missing.md"
write_record "$branding_missing" APPROVED source undecided undecided
expect_fail_contains "source-only alpha requires a branding decision" "$branding_missing" \
  "### 13\\. Branding Stability must have exactly one checked choice; found 0"

versioning_missing="$TMPDIR/versioning-missing.md"
write_record "$versioning_missing" APPROVED source decided undecided
perl -0pi -e 's/- \[x\] Alpha pre-release tags only\./- [ ] Alpha pre-release tags only./' "$versioning_missing"
expect_fail_contains "source-only alpha requires a versioning decision" "$versioning_missing" \
  "### 14\\. Versioning And Compatibility must have exactly one checked choice; found 0"

community_missing="$TMPDIR/community-missing.md"
write_record "$community_missing" APPROVED source decided undecided
perl -0pi -e 's/- \[x\] Open public issues only/- [ ] Open public issues only/' "$community_missing"
expect_fail_contains "source-only alpha requires a community intake decision" "$community_missing" \
  "### 15\\. Community Intake And Moderation must have exactly one checked choice; found 0"

custody_missing="$TMPDIR/custody-missing.md"
write_record "$custody_missing" APPROVED source decided undecided
perl -0pi -e 's/- \[x\] Single-maintainer alpha\./- [ ] Single-maintainer alpha./' "$custody_missing"
expect_fail_contains "source-only alpha requires a release custody decision" "$custody_missing" \
  "### 16\\. Release Custody And Maintainer Authority must have exactly one checked choice; found 0"

binary_missing="$TMPDIR/binary-missing.md"
write_record "$binary_missing" APPROVED unsigned decided undecided
expect_fail "public app bundle requires signing/update decisions" "$binary_missing"

binary_decided="$TMPDIR/binary-decided.md"
write_record "$binary_decided" APPROVED unsigned decided decided
expect_pass "public app bundle passes after binary-only decisions are recorded" "$binary_decided"

pending="$TMPDIR/pending.md"
write_record "$pending" PENDING source decided undecided
expect_fail_contains "pending records are rejected" "$pending" \
  "owner decision record is not approved"

pending_missing="$TMPDIR/pending-missing.md"
write_record "$pending_missing" PENDING source decided undecided
perl -0pi -e 's/- \[x\] MIT only\./- [ ] MIT only./' "$pending_missing"
expect_fail_contains "pending records still report missing choices" "$pending_missing" \
  "### 1\\. License must have exactly one checked choice; found 0"

echo "Owner decision gate tests passed."
