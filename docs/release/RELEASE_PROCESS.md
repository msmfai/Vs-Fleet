# Release Process

This process is for a source-only public alpha. It deliberately does not publish
signed macOS binaries, crates, npm packages, VS Code marketplace packages, Open
VSX packages, or container images.

## Preconditions

Do not publish a public alpha until these are true:

- A project license is chosen.
- A root `LICENSE` file exists.
- Rust and npm package metadata no longer declare `UNLICENSED`.
- `./scripts/apply-license-decision.sh docs/release/OWNER_DECISION_RECORD.md . path/to/LICENSE`
  has been run with the reviewed legal license text, unless the metadata and
  root `LICENSE` were updated manually.
- If using recommended defaults, generate a PENDING review draft with
  `./scripts/draft-owner-decisions.sh <github-owner> <github-repo> docs/release/OWNER_DECISION_RECORD.draft.md`
  and copy only reviewed choices into `docs/release/OWNER_DECISION_RECORD.md`.
- `./scripts/check-license-decision.sh docs/release/OWNER_DECISION_RECORD.md .`
  passes.
- `./scripts/apply-namespace-decision.sh docs/release/OWNER_DECISION_RECORD.md .`
  has been run after the approved namespace decision, unless the metadata was
  updated manually. Rust crate renames are intentionally not automatic.
- `./scripts/public-alpha-decision-packet.sh` reports
  `Release readiness: OWNER DECISIONS COMPLETE`.
- `./scripts/release-check.sh` passes.
- CI is green on the exact public branch or commit, including the manual
  "Release Readiness" workflow.
- `docs/release/PUBLIC_CI_EVIDENCE.md` records the exact commit, branch, CI
  workflow run, and Release Readiness workflow run for the first public GitHub
  alpha.
- `docs/release/GITHUB_PUBLICATION_EVIDENCE.md` records the exact GitHub
  repository URL, visibility review, repository settings, security settings,
  and branch-protection review for the first public GitHub alpha.
- Generated artifacts, local logs, screenshots, VSIX files, app bundles, and
  machine-specific paths are not tracked.
- `./scripts/secret-release-check.sh` passes for the tracked tree and git
  history.
- `./scripts/history-release-check.sh` passes, or the approved owner decision
  record explicitly accepts current branch history exposure.
- If current history is not accepted, `./scripts/prepare-public-branch.sh` is
  used to create a single-commit public branch from the approved source tree.
- Rust crate manifests retain `publish = false` and extension package manifests
  retain `"private": true` unless the owner decision record explicitly changes
  distribution scope away from source-only alpha.
- `./scripts/check-lockfile-policy.sh` passes, so root Cargo, standalone host
  Cargo, pnpm, and package npm lockfiles are tracked for exact-commit review.
- `./scripts/check-distribution-decision.sh docs/release/OWNER_DECISION_RECORD.md .`
  passes.
- GitHub Private Vulnerability Reporting is enabled, or `SECURITY.md` names a
  private contact channel.
- Public namespaces are confirmed, even if packages are not published yet.
- `./scripts/check-namespace-decision.sh docs/release/OWNER_DECISION_RECORD.md .`
  passes.
- `./scripts/check-alpha-scope-decision.sh docs/release/OWNER_DECISION_RECORD.md .`
  passes.
- `./scripts/check-editor-server-boundary-decision.sh docs/release/OWNER_DECISION_RECORD.md .`
  passes.
- `./scripts/check-security-reporting-decision.sh docs/release/OWNER_DECISION_RECORD.md SECURITY.md`
  passes.
- `./scripts/check-contribution-decision.sh docs/release/OWNER_DECISION_RECORD.md CONTRIBUTING.md .github/PULL_REQUEST_TEMPLATE.md`
  passes.
- `./scripts/check-github-intake-templates.sh` passes, so public issue and PR
  templates keep alpha scope, privacy, security-reporting, and contribution
  boundaries visible.
- `./scripts/check-doc-links.sh` passes, so tracked Markdown does not contain
  broken local links.
- `./scripts/check-public-tree-size.sh` passes, so the public branch does not
  contain accidental large tracked artifacts.
- `./scripts/check-ci-evidence-decision.sh docs/release/OWNER_DECISION_RECORD.md docs/release/PUBLIC_CI_EVIDENCE.md "$(git rev-parse HEAD)"`
  passes.
- `./scripts/check-github-publication-evidence.sh docs/release/OWNER_DECISION_RECORD.md docs/release/GITHUB_PUBLICATION_EVIDENCE.md "$(git rev-parse HEAD)"`
  passes.
- `./scripts/check-github-workflows.sh .github/workflows/ci.yml .github/workflows/release-readiness.yml`
  passes, so the workflows referenced by public CI evidence still contain the
  required source-alpha checks.
- `./scripts/check-privacy-decision.sh docs/release/OWNER_DECISION_RECORD.md .`
  passes.
- `./scripts/run-dependency-review.sh` generates
  `docs/release/DEPENDENCY_REVIEW_EVIDENCE.md` for the exact public commit when
  the owner chooses to run dependency review.
- `./scripts/check-dependency-review-decision.sh docs/release/OWNER_DECISION_RECORD.md docs/release/DEPENDENCY_REVIEW_EVIDENCE.md "$(git rev-parse HEAD)"`
  passes.
- `./scripts/check-dependabot-config.sh .github/dependabot.yml` passes, so the
  first public branch has dependency update coverage for GitHub Actions, Cargo,
  and npm surfaces.
- `./scripts/check-support-decision.sh docs/release/OWNER_DECISION_RECORD.md SUPPORT.md .`
  passes.
- Branding stability for the `Fleet` name and icon is explicitly selected in
  `docs/release/OWNER_DECISION_RECORD.md`.
- `./scripts/check-branding-decision.sh docs/release/OWNER_DECISION_RECORD.md .`
  passes.
- `./scripts/check-versioning-decision.sh docs/release/OWNER_DECISION_RECORD.md .`
  passes.
- Dependency review has been run for the exact public commit, or the approved
  owner decision record explicitly accepts publishing without it.
- `docs/release/DEPENDENCY_REVIEW_EVIDENCE.md` records the exact commit,
  command results, and accepted findings or skipped-review risk.
- GitHub pre-release notes are drafted from
  [ALPHA_RELEASE_NOTES_TEMPLATE.md](ALPHA_RELEASE_NOTES_TEMPLATE.md), with every
  placeholder replaced.
- [GITHUB_PUBLICATION_RUNBOOK.md](GITHUB_PUBLICATION_RUNBOOK.md) has been walked
  for the exact GitHub repository before public visibility or the first public
  pre-release.

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
   ./scripts/check-alpha-scope-decision.sh docs/release/OWNER_DECISION_RECORD.md .
   ./scripts/check-editor-server-boundary-decision.sh docs/release/OWNER_DECISION_RECORD.md .
   ./scripts/check-distribution-decision.sh docs/release/OWNER_DECISION_RECORD.md .
   ./scripts/check-security-reporting-decision.sh docs/release/OWNER_DECISION_RECORD.md SECURITY.md
   ./scripts/check-contribution-decision.sh docs/release/OWNER_DECISION_RECORD.md CONTRIBUTING.md .github/PULL_REQUEST_TEMPLATE.md
   ./scripts/check-github-intake-templates.sh
   ./scripts/check-doc-links.sh
   ./scripts/check-public-tree-size.sh
   ./scripts/check-ci-evidence-decision.sh docs/release/OWNER_DECISION_RECORD.md docs/release/PUBLIC_CI_EVIDENCE.md "$(git rev-parse HEAD)"
   ./scripts/check-github-publication-evidence.sh docs/release/OWNER_DECISION_RECORD.md docs/release/GITHUB_PUBLICATION_EVIDENCE.md "$(git rev-parse HEAD)"
   ./scripts/check-github-workflows.sh .github/workflows/ci.yml .github/workflows/release-readiness.yml
   ./scripts/check-privacy-decision.sh docs/release/OWNER_DECISION_RECORD.md .
   ./scripts/run-dependency-review.sh
   ./scripts/check-dependency-review-decision.sh docs/release/OWNER_DECISION_RECORD.md docs/release/DEPENDENCY_REVIEW_EVIDENCE.md "$(git rev-parse HEAD)"
   ./scripts/check-dependabot-config.sh .github/dependabot.yml
   ./scripts/check-lockfile-policy.sh
   ./scripts/check-support-decision.sh docs/release/OWNER_DECISION_RECORD.md SUPPORT.md .
   ./scripts/check-branding-decision.sh docs/release/OWNER_DECISION_RECORD.md .
   ./scripts/check-versioning-decision.sh docs/release/OWNER_DECISION_RECORD.md .
   ./scripts/secret-release-check.sh
   ./scripts/release-check.sh
   ```

7. Review the generated
   [DEPENDENCY_REVIEW_EVIDENCE.md](DEPENDENCY_REVIEW_EVIDENCE.md), and record
   any accepted findings in the release notes. If dependency review is
   deliberately skipped for the first public source alpha, record that accepted
   risk in
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

   If it fails, either publish a cleaned/squashed first public branch using
   `./scripts/prepare-public-branch.sh <public-branch> <source-ref>` or approve
   the owner decision record choice that accepts current branch history exposure.

10. Run the secret exposure audit:

   ```sh
   ./scripts/secret-release-check.sh
   ```

   If it fails, remove the secret from the tracked tree and publish rewritten or
   squashed history. Do not treat credential-looking history as an accepted
   public-alpha exception.

11. Run the normal GitHub "CI" workflow and the manual GitHub "Release
   Readiness" workflow on the exact commit you intend to publish, then update
   [PUBLIC_CI_EVIDENCE.md](PUBLIC_CI_EVIDENCE.md) with the commit SHA, branch,
   CI workflow run URL, and Release Readiness workflow run URL. Release
   Readiness is expected to fail until the owner decision record is approved and
   the license metadata is applied.

12. Fill `docs/release/GITHUB_PUBLICATION_EVIDENCE.md` with the exact GitHub
   repository URL, repository settings, security settings, and branch-protection
   evidence for the public repo.

13. Create a signed git tag after checks pass:

   ```sh
   git tag -s v0.1.0-alpha.1 -m "Fleet v0.1.0-alpha.1"
   ```

   Use an annotated tag if signing is not configured, but record that choice in
   the release notes.

14. Draft release notes from
   [ALPHA_RELEASE_NOTES_TEMPLATE.md](ALPHA_RELEASE_NOTES_TEMPLATE.md). Replace
   every placeholder with exact commit, scope, branding status, verification,
   dependency review, history exposure, security, support, and known-rough-edge
   evidence.

15. Validate the drafted release notes:

   ```sh
   ./scripts/check-release-notes.sh path/to/release-notes.md "$(git rev-parse HEAD)"
   ```

16. Push the tag and create a GitHub release marked pre-release. The release
   should be source-only unless binary distribution has been explicitly approved.
   Before changing repository visibility or publishing the pre-release, walk
   [GITHUB_PUBLICATION_RUNBOOK.md](GITHUB_PUBLICATION_RUNBOOK.md) against the
   exact GitHub repository settings.

## First Public GitHub Publish

Before making a previously private branch public, decide whether to squash or
rewrite history. Raw artifacts were removed from the current tree, but prior
commits may still contain local paths or failed visual/eval evidence. If that is
not acceptable, create a single-commit public branch:

```sh
./scripts/prepare-public-branch.sh public-alpha HEAD
./scripts/history-release-check.sh docs/release/OWNER_DECISION_RECORD.md public-alpha
FLEET_RELEASE_HISTORY_REF=public-alpha ./scripts/release-check.sh
```

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
