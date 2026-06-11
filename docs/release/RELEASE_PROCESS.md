# Release Process

This process is for a source-only public alpha. It deliberately does not publish
signed macOS binaries, crates, npm packages, VS Code marketplace packages, Open
VSX packages, or container images.

## Preconditions

Do not publish a public alpha until these are true:

- A project license is chosen.
- A root `LICENSE` file exists.
- Rust and npm package metadata no longer declare `UNLICENSED`.
- `./scripts/check-license-decision.sh docs/release/OWNER_DECISION_RECORD.md .`
  passes.
- `./scripts/release-check.sh` passes.
- CI is green on the exact public branch or commit, including the manual
  "Release Readiness" workflow.
- `docs/release/PUBLIC_CI_EVIDENCE.md` records the exact commit and CI evidence
  for the first public GitHub alpha.
- Generated artifacts, local logs, screenshots, VSIX files, app bundles, and
  machine-specific paths are not tracked.
- `./scripts/history-release-check.sh` passes, or the approved owner decision
  record explicitly accepts current branch history exposure.
- Rust crate manifests retain `publish = false` and extension package manifests
  retain `"private": true` unless the owner decision record explicitly changes
  distribution scope away from source-only alpha.
- GitHub Private Vulnerability Reporting is enabled, or `SECURITY.md` names a
  private contact channel.
- Public namespaces are confirmed, even if packages are not published yet.
- `./scripts/check-namespace-decision.sh docs/release/OWNER_DECISION_RECORD.md .`
  passes.
- `./scripts/check-security-reporting-decision.sh docs/release/OWNER_DECISION_RECORD.md SECURITY.md`
  passes.
- `./scripts/check-contribution-decision.sh docs/release/OWNER_DECISION_RECORD.md CONTRIBUTING.md .github/PULL_REQUEST_TEMPLATE.md`
  passes.
- `./scripts/check-ci-evidence-decision.sh docs/release/OWNER_DECISION_RECORD.md docs/release/PUBLIC_CI_EVIDENCE.md "$(git rev-parse HEAD)"`
  passes.
- `./scripts/check-dependency-review-decision.sh docs/release/OWNER_DECISION_RECORD.md docs/release/DEPENDENCY_REVIEW_EVIDENCE.md "$(git rev-parse HEAD)"`
  passes.
- Dependency review has been run for the exact public commit, or the approved
  owner decision record explicitly accepts publishing without it.
- `docs/release/DEPENDENCY_REVIEW_EVIDENCE.md` records the exact commit,
  command results, and accepted findings or skipped-review risk.
- GitHub pre-release notes are drafted from
  [ALPHA_RELEASE_NOTES_TEMPLATE.md](ALPHA_RELEASE_NOTES_TEMPLATE.md), with every
  placeholder replaced.

## Source Alpha Steps

1. Update release notes or the README if the supported alpha scope changed.
2. Run the core Rust checks:

   ```sh
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets --all-features -- -D warnings
   cargo test --workspace --all-targets --all-features
   ```

3. Build the Fleet bridge package. The host bundle step packages this VSIX, so
   this must happen before `./bundle.sh release` on a fresh checkout:

   ```sh
   ( cd packages/fleet-bridge && npm ci && npm run build )
   ```

4. Run the standalone host checks on macOS:

   ```sh
   (
     cd crates/fleet-host
     cargo fmt -- --check
     cargo test
     ./bundle.sh release
   )
   ```

   The bundle step is a local verification step for alpha. Do not attach the
   unsigned `Fleet.app` bundle to a public release unless the binary distribution
   decision has explicitly changed.

5. Run the remaining JavaScript checks:

   ```sh
   ( cd packages/extension && npm ci && npm run build && npm test )
   ```

   If using the CI path instead, require the `pnpm -r build` and `pnpm -r test`
   jobs to pass on GitHub.

6. Run the public release hygiene gate:

   ```sh
   ./scripts/check-license-decision.sh docs/release/OWNER_DECISION_RECORD.md .
   ./scripts/check-namespace-decision.sh docs/release/OWNER_DECISION_RECORD.md .
   ./scripts/check-security-reporting-decision.sh docs/release/OWNER_DECISION_RECORD.md SECURITY.md
   ./scripts/check-contribution-decision.sh docs/release/OWNER_DECISION_RECORD.md CONTRIBUTING.md .github/PULL_REQUEST_TEMPLATE.md
   ./scripts/check-ci-evidence-decision.sh docs/release/OWNER_DECISION_RECORD.md docs/release/PUBLIC_CI_EVIDENCE.md "$(git rev-parse HEAD)"
   ./scripts/check-dependency-review-decision.sh docs/release/OWNER_DECISION_RECORD.md docs/release/DEPENDENCY_REVIEW_EVIDENCE.md "$(git rev-parse HEAD)"
   ./scripts/release-check.sh
   ```

7. Run the dependency review in
   [DEPENDENCY_REVIEW.md](DEPENDENCY_REVIEW.md), and record any accepted
   findings in [DEPENDENCY_REVIEW_EVIDENCE.md](DEPENDENCY_REVIEW_EVIDENCE.md)
   and the release notes. If this is deliberately skipped for the first public
   source alpha, record that accepted risk in
   [OWNER_DECISION_RECORD.md](OWNER_DECISION_RECORD.md).

8. Verify the public tree has no tracked generated artifacts:

   ```sh
   git ls-files | rg '(^|/)coverage/|(^|/)node_modules/|(^|/)out/|\.vsix$|Fleet\.app/'
   ```

   The command should print nothing.

9. Run the history exposure audit:

   ```sh
   ./scripts/history-release-check.sh docs/release/OWNER_DECISION_RECORD.md
   ```

   If it fails, either publish a cleaned/squashed first public branch or approve
   the owner decision record choice that accepts current branch history exposure.

10. Run the manual GitHub "Release Readiness" workflow on the exact commit you
   intend to publish, then update
   [PUBLIC_CI_EVIDENCE.md](PUBLIC_CI_EVIDENCE.md) with the commit SHA, branch,
   and workflow run URL. It is expected to fail until the owner decision record
   is approved and the license metadata is applied.

11. Create a signed git tag after checks pass:

   ```sh
   git tag -s v0.1.0-alpha.1 -m "Fleet v0.1.0-alpha.1"
   ```

   Use an annotated tag if signing is not configured, but record that choice in
   the release notes.

12. Draft release notes from
   [ALPHA_RELEASE_NOTES_TEMPLATE.md](ALPHA_RELEASE_NOTES_TEMPLATE.md). Replace
   every placeholder with exact commit, scope, verification, dependency review,
   history exposure, security, support, and known-rough-edge evidence.

13. Validate the drafted release notes:

   ```sh
   ./scripts/check-release-notes.sh path/to/release-notes.md
   ```

14. Push the tag and create a GitHub release marked pre-release. The release
   should be source-only unless binary distribution has been explicitly approved.

## First Public GitHub Publish

Before making a previously private branch public, decide whether to squash or
rewrite history. Raw artifacts were removed from the current tree, but prior
commits may still contain local paths or failed visual/eval evidence. If that is
not acceptable, publish a cleaned history rather than the full working branch.
Use `./scripts/history-release-check.sh` as the mechanical audit before first
public visibility.

## Binary Release Gate

Do not publish a macOS app bundle until there is a separate binary release
process covering:

- Apple Developer ID signing,
- notarization,
- checksum generation,
- release asset naming,
- upgrade and rollback expectations,
- support policy for users who did not build from source.

Until that process exists, `./bundle.sh release` is a local build verification
tool, not a public distribution mechanism.
