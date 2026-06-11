#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

write_record() {
  local file=$1
  local status=$2
  local history=$3
  local distribution=$4
  local binary=$5

  local current_history_checked=' '
  local clean_history_checked=' '
  local source_checked=' '
  local unsigned_checked=' '
  local signing_checked=' '
  local update_checked=' '

  case "$history" in
    current) current_history_checked='x' ;;
    clean) clean_history_checked='x' ;;
    none) ;;
    *) echo "unknown history fixture: $history" >&2; exit 2 ;;
  esac

  case "$distribution" in
    source) source_checked='x' ;;
    unsigned) unsigned_checked='x' ;;
    none) ;;
    *) echo "unknown distribution fixture: $distribution" >&2; exit 2 ;;
  esac

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

- [x] MIT OR Apache-2.0 dual license.
- [ ] Other: \`TODO\`

### 2. Public History

- [$current_history_checked] Publish the current branch history and accept that old commits may contain
  removed local artifacts or failed eval evidence.
- [$clean_history_checked] Publish a cleaned/squashed history for the first public branch.

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

- [x] \`Fleet\` name and current icon are alpha placeholders.
- [ ] Other: \`TODO\`

### 14. Versioning And Compatibility

- [x] Alpha pre-release tags only. No stable API, protocol, state-file, or upgrade compatibility is promised during alpha.
- [ ] Other: \`TODO\`

### 15. Community Intake And Moderation

- [x] Open public issues only for scoped bug reports and alpha feedback; keep blank issues disabled and keep discussions off unless explicitly enabled later.
- [ ] Other: \`TODO\`

### 16. Release Custody And Maintainer Authority

- [x] Single-maintainer alpha. Only the repository owner or named maintainer may push release tags, create GitHub releases, change repository settings, or publish packages.
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
  shift
  if ! "$ROOT/scripts/public-alpha-decision-packet.sh" "$@" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected pass: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_fail() {
  local label=$1
  shift
  if "$ROOT/scripts/public-alpha-decision-packet.sh" "$@" >"$TMPDIR/out" 2>&1; then
    echo "FAIL: expected failure: $label" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

expect_output() {
  local label=$1
  local pattern=$2
  if ! rg -q "$pattern" "$TMPDIR/out"; then
    echo "FAIL: expected $label output to contain: $pattern" >&2
    cat "$TMPDIR/out" >&2
    exit 1
  fi
}

pending="$TMPDIR/pending.md"
write_record "$pending" PENDING none none undecided
expect_fail "pending incomplete record" "$pending"
expect_output "pending incomplete record" 'Decision record status: not APPROVED'
expect_output "pending incomplete record" '2\. Public History'
expect_output "pending incomplete record" '6\. Distribution Scope'
expect_output "pending incomplete record" 'Choose Public History before selecting the release-check command'
expect_output "pending incomplete record" 'Release readiness: BLOCKED'

clean_source="$TMPDIR/clean-source.md"
write_record "$clean_source" APPROVED clean source undecided
expect_pass "approved clean source-only record" "$clean_source"
expect_output "approved clean source-only record" 'Release readiness: OWNER DECISIONS COMPLETE'
expect_output "approved clean source-only record" 'FLEET_RELEASE_HISTORY_REF=public-alpha ./scripts/release-check.sh'

current_source="$TMPDIR/current-source.md"
write_record "$current_source" APPROVED current source undecided
expect_pass "approved current-history source-only record" "$current_source"
expect_output "approved current-history source-only record" './scripts/release-check.sh'

binary_missing="$TMPDIR/binary-missing.md"
write_record "$binary_missing" APPROVED clean unsigned undecided
expect_fail "binary distribution without binary decisions" "$binary_missing"
expect_output "binary distribution without binary decisions" '17\. macOS Signing and Notarization'
expect_output "binary distribution without binary decisions" '18\. Update Channel'

todo_namespace="$TMPDIR/todo-namespace.md"
write_record "$todo_namespace" APPROVED clean source undecided
perl -0pi -e 's/\| GitHub repo name \| vs-fleet \|/| GitHub repo name | `TODO` |/' "$todo_namespace"
expect_fail "namespace TODOs are reported" "$todo_namespace"
expect_output "namespace TODOs are reported" '3\. Public Namespace'
expect_output "namespace TODOs are reported" 'GitHub repo name'

echo "Public alpha decision packet tests passed."
