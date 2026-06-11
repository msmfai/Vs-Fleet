#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
usage: scripts/draft-owner-decisions.sh <github-owner> <github-repo> [output-file|-]

Write a PENDING owner decision record with the recommended source-alpha defaults
checked. This does not approve the release. Review the output, edit any choices
you disagree with, then change the status to APPROVED yourself.

If output-file is omitted or "-", the draft is printed to stdout. Existing files
are not overwritten unless FLEET_OWNER_DRAFT_FORCE=1 is set.
EOF
}

github_owner="${1:-}"
github_repo="${2:-}"
output="${3:--}"

if [ -z "$github_owner" ] || [ -z "$github_repo" ] || [ "$github_owner" = "-h" ] || [ "$github_owner" = "--help" ]; then
  usage
  exit 2
fi

if printf '%s\n%s\n' "$github_owner" "$github_repo" | rg -q '`|[[:cntrl:]]'; then
  echo "FAIL: GitHub owner and repo must be plain text without backticks or control characters" >&2
  exit 1
fi

write_draft() {
  cat <<EOF
# Owner Decision Record

Decision record status: PENDING

This draft was generated with recommended source-alpha defaults. It is not an
approval. Review every checked choice, edit anything you disagree with, then
change the status to APPROVED only when the decisions are final.

## Required Before Public GitHub Visibility

### 1. License

Choose one and then apply it to \`LICENSE\`, Rust manifests, npm manifests, and
lockfiles.

- [x] MIT OR Apache-2.0 dual license.
- [ ] MIT only.
- [ ] Apache-2.0 only.
- [ ] AGPL-3.0-only.
- [ ] Other: \`TODO\`

Current default recommendation: MIT OR Apache-2.0 for a permissive Rust-friendly
alpha, unless you deliberately want network copyleft.

### 2. Public History

Choose how the first public GitHub history should look.

- [ ] Publish the current branch history and accept that old commits may contain
  removed local artifacts or failed eval evidence.
- [x] Publish a cleaned/squashed history for the first public branch.

Current default recommendation: cleaned/squashed history before first public
visibility.

### 3. Public Namespace

Fill these before publishing packages or telling users names are stable.

| Surface | Decision |
|---|---|
| GitHub org/user | $github_owner |
| GitHub repo name | $github_repo |
| Product name | Fleet |
| Rust crate prefix | fleet-* |
| npm package names | fleet-extension, fleet-bridge |
| VS Code Marketplace publisher | fleet-team |
| Open VSX publisher | fleet-team |
| macOS bundle id | dev.fleet.host |

Current default recommendation: confirm the GitHub repo and bundle id now; defer
marketplace/crates/npm publication until after source alpha.

### 4. Alpha Scope

Choose what public users can treat as the supported source-alpha surface.

- [x] Local macOS Fleet host plus local \`code serve-web\` sessions, Fleet bridge,
  Fleet reporter, CLI, and embedded local Hub. Remote, SSH, Docker/container,
  visual probe, and eval harness paths remain development infrastructure, not
  public support commitments.
- [ ] Broaden public alpha scope to include remote, SSH, Docker/container, or
  eval harness paths as supported user workflows.
- [ ] Other: \`TODO\`

Current default recommendation: keep the first public alpha scoped to the local
macOS host and local \`code serve-web\` workflow.

### 5. Editor Server Licensing Boundary

Choose how the public alpha handles editor server components.

- [x] User-provided VS Code only. Fleet may launch the user's local
  \`code serve-web\` install, but Fleet does not download, bundle, host, or
  redistribute Microsoft's VS Code Server, Microsoft Marketplace extensions, or
  Microsoft remote extensions.
- [ ] OSS server only. Supported workflows use \`code-server\` or
  \`openvscode-server\` with Open VSX; no Microsoft VS Code Server or Marketplace
  dependency.
- [ ] Other: \`TODO\`

Current default recommendation: user-provided VS Code only for the local source
alpha; require an OSS server/Open VSX path before supporting deployed remote or
container workflows.

### 6. Distribution Scope

Choose what the first public alpha promises.

- [x] Source-only alpha. No public app bundle, crates.io, npm, Open VSX, VS Code
  Marketplace, or container image publishing.
- [ ] Source plus unsigned macOS app bundle.
- [ ] Source plus signed/notarized macOS app bundle.
- [ ] Other: \`TODO\`

Current default recommendation: source-only alpha.

### 7. Security Reporting Channel

Choose the private vulnerability path before public visibility.

- [x] Enable GitHub Private Vulnerability Reporting.
- [ ] Add a private security email/contact to \`SECURITY.md\`.
- [ ] Other: \`TODO\`

Current default recommendation: GitHub Private Vulnerability Reporting.

### 8. Contribution Intake

Choose how to handle first outside PRs after the license is applied.

- [x] Accept small focused PRs under the chosen project license using the PR
  template certification.
- [ ] Require DCO sign-off.
- [ ] Keep code PRs closed; accept issues and docs feedback only.
- [ ] Other: \`TODO\`

Current default recommendation: accept small focused PRs only after the license
is applied; no CLA for alpha.

### 9. Public CI Evidence

Choose the CI gate for the public branch.

- [x] Require GitHub Actions green on the exact branch/commit before public
  visibility.
- [ ] Accept local check output only for the first publish.
- [ ] Other: \`TODO\`

Current default recommendation: require GitHub Actions green on the exact public
branch/commit.

### 10. Privacy And Telemetry Posture

Choose the privacy/logging promise before public visibility.

- [x] No telemetry by default. Local logs and artifacts may contain workspace
  paths, local URLs, session labels, process command lines, and editor state;
  users must scrub them before sharing.
- [ ] Add an explicit telemetry or remote reporting disclosure before public
  visibility.
- [ ] Other: \`TODO\`

Current default recommendation: no telemetry by default, with explicit local log
contents and scrub-before-sharing warnings in public docs.

### 11. Dependency Review Evidence

Choose what dependency evidence is required for the exact public commit.

- [x] Run the dependency review commands in \`docs/release/DEPENDENCY_REVIEW.md\`
  and record findings in the release notes.
- [ ] Publish the first source alpha without dependency review and accept
  advisory/license review risk.
- [ ] Other: \`TODO\`

Current default recommendation: run and record the dependency review before
public visibility; do not defer it unless the release is deliberately
invite-only.

### 12. Support Commitment

Choose what support public alpha users can expect.

- [x] Best-effort alpha support only. Breaking changes are expected; there are
  no production support guarantees, response SLAs, paid support terms, or stable
  release lines.
- [ ] Define a public triage or response target in \`SUPPORT.md\`.
- [ ] Other: \`TODO\`

Current default recommendation: best-effort alpha support only.

### 13. Branding Stability

Choose how stable the public alpha name and visual identity are.

- [x] \`Fleet\` name and current icon are alpha placeholders.
- [ ] \`Fleet\` name is stable, icon may change.
- [ ] Name and icon are stable.
- [ ] Other: \`TODO\`

Current default recommendation: treat the icon as alpha and confirm the name
only after the namespace check passes.

### 14. Versioning And Compatibility

Choose what compatibility public alpha users can expect.

- [x] Alpha pre-release tags only. No stable API, protocol, state-file, or
  upgrade compatibility is promised during alpha.
- [ ] Commit to semver-compatible public CLI, protocol, and state changes during
  alpha.
- [ ] Other: \`TODO\`

Current default recommendation: use alpha pre-release tags and promise no stable
API, protocol, state-file, or upgrade compatibility until the project has real
outside users.

## Required Before Binary Distribution

These do not block a source-only alpha, but they must be decided before any
public app bundle.

### 15. macOS Signing and Notarization

- [ ] No public binaries until Developer ID signing and notarization are
  automated.
- [ ] Publish unsigned binaries and document Gatekeeper warnings.
- [ ] Other: \`TODO\`

Current default recommendation: no public binaries until signing and notarization
are automated.

### 16. Update Channel

- [ ] No auto-update in alpha.
- [ ] GitHub Releases only.
- [ ] In-app updater.
- [ ] Other: \`TODO\`

Current default recommendation: no auto-update in alpha; GitHub Releases only
for source tags.
EOF
}

if [ "$output" = "-" ]; then
  write_draft
  exit 0
fi

if [ -e "$output" ] && [ "${FLEET_OWNER_DRAFT_FORCE:-0}" != "1" ]; then
  echo "FAIL: refusing to overwrite existing file: $output" >&2
  echo "Set FLEET_OWNER_DRAFT_FORCE=1 to replace it." >&2
  exit 1
fi

mkdir -p "$(dirname "$output")"
tmp="${output}.$$"
write_draft >"$tmp"
mv "$tmp" "$output"
echo "Wrote owner decision draft: $output"
echo "Review it, edit any choices you disagree with, then change status to APPROVED yourself."
