# Release Process

This process is for a source-only public alpha. It deliberately does not publish
signed macOS binaries, crates, npm packages, VS Code marketplace packages, Open
VSX packages, or container images.

## Preconditions

Do not publish a public alpha until these are true:

- The `MIT OR Apache-2.0` project license is applied.
- Root `LICENSE` exists, with full MIT and Apache-2.0 texts under `docs/legal/`.
- Rust and npm package metadata use `MIT OR Apache-2.0`.
- If using recommended defaults, generate a PENDING review draft with
  `./scripts/draft-owner-decisions.sh <github-owner> <github-repo> docs/release/OWNER_DECISION_RECORD.draft.md`
  and copy only reviewed choices into `docs/release/OWNER_DECISION_RECORD.md`.
- `./scripts/check-license-decision.sh docs/release/OWNER_DECISION_RECORD.md .`
  passes.
- `./scripts/check-public-alpha-readiness-assessment.sh` passes, so the public
  release posture still says source-only alpha after gates, not binaries,
  package publication, production support, stable APIs, or remote/container
  support.
- `./scripts/apply-namespace-decision.sh docs/release/OWNER_DECISION_RECORD.md .`
  has been run after the approved namespace decision, unless the metadata was
  updated manually. Rust crate renames are intentionally not automatic.
- `./scripts/public-alpha-decision-packet.sh` reports
  `Release readiness: OWNER DECISIONS COMPLETE`.
- For the recommended cleaned-history release, `./scripts/check-public-release-branch.sh
  <public-branch> <source-ref-sha>` passes. If the owner explicitly accepts
  current branch history exposure instead, `./scripts/release-check.sh` passes
  on the current branch.
- CI is green on the exact public branch or commit, including the manual
  "Release Readiness" workflow.
- CI and Release Readiness run URLs for the exact public commit are recorded in
  the release notes or publication checklist.
- GitHub repository URL, visibility review, repository settings, security
  settings, branch-protection review, and release custody are checked before the
  first public GitHub alpha.
- Generated artifacts, local logs, screenshots, VSIX files, app bundles, and
  machine-specific paths are not tracked.
- `./scripts/secret-release-check.sh` passes for the tracked tree and git
  history.
- `./scripts/history-release-check.sh` passes, or the approved owner decision
  record explicitly accepts current branch history exposure.
- If current history is not accepted, `./scripts/prepare-public-branch.sh` is
  used to create a single-commit public branch from the approved source tree.
- If current history is not accepted, `./scripts/check-public-release-branch.sh
  <public-branch> <source-ref-sha>` verifies the public branch tree, history,
  secrets, and aggregate release gates.
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
- `./scripts/check-github-workflows.sh .github/workflows/ci.yml .github/workflows/release-readiness.yml`
  passes, so the public CI workflows still contain the required source-alpha
  checks.
- `./scripts/check-privacy-decision.sh docs/release/OWNER_DECISION_RECORD.md .`
  passes.
- `./scripts/run-dependency-review.sh` passes for the exact public commit when
  the owner chooses to run dependency review.
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
- GitHub pre-release notes are generated with
  `./scripts/generate-alpha-release-notes.sh` after owner decisions and release
  checks pass, then checked with `./scripts/check-release-notes.sh`.
  [ALPHA_RELEASE_NOTES_TEMPLATE.md](ALPHA_RELEASE_NOTES_TEMPLATE.md) remains the
  checked content template and disclosure baseline.
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

6. Run the public release hygiene gate during release-prep:

   ```sh
   ./scripts/check-license-decision.sh docs/release/OWNER_DECISION_RECORD.md .
   ./scripts/check-public-alpha-readiness-assessment.sh
   ./scripts/check-namespace-decision.sh docs/release/OWNER_DECISION_RECORD.md .
   ./scripts/check-alpha-scope-decision.sh docs/release/OWNER_DECISION_RECORD.md .
   ./scripts/check-editor-server-boundary-decision.sh docs/release/OWNER_DECISION_RECORD.md .
   ./scripts/check-distribution-decision.sh docs/release/OWNER_DECISION_RECORD.md .
   ./scripts/check-security-reporting-decision.sh docs/release/OWNER_DECISION_RECORD.md SECURITY.md
   ./scripts/check-contribution-decision.sh docs/release/OWNER_DECISION_RECORD.md CONTRIBUTING.md .github/PULL_REQUEST_TEMPLATE.md
   ./scripts/check-github-intake-templates.sh
   ./scripts/check-doc-links.sh
   ./scripts/check-public-tree-size.sh
   ./scripts/check-github-workflows.sh .github/workflows/ci.yml .github/workflows/release-readiness.yml
   ./scripts/check-privacy-decision.sh docs/release/OWNER_DECISION_RECORD.md .
   ./scripts/run-dependency-review.sh
   ./scripts/check-dependabot-config.sh .github/dependabot.yml
   ./scripts/check-lockfile-policy.sh
   ./scripts/check-support-decision.sh docs/release/OWNER_DECISION_RECORD.md SUPPORT.md .
   ./scripts/check-branding-decision.sh docs/release/OWNER_DECISION_RECORD.md .
   ./scripts/check-versioning-decision.sh docs/release/OWNER_DECISION_RECORD.md .
   ./scripts/secret-release-check.sh
   ./scripts/release-check.sh
   ```

   On the current private release-prep branch, `./scripts/release-check.sh` is
   expected to keep failing until owner decisions are approved and either
   current history exposure is accepted or the final check is run through
   `./scripts/check-public-release-branch.sh` against the cleaned branch.

7. Review the `./scripts/run-dependency-review.sh` output and record any
   accepted findings in the release notes. If dependency review is deliberately
   skipped for the first public source alpha, record that accepted risk in
   [OWNER_DECISION_RECORD.md](OWNER_DECISION_RECORD.md).

8. Verify the public tree has no tracked generated artifacts:

   ```sh
   git ls-files | rg '(^|/)coverage/|(^|/)node_modules/|(^|/)out/|\.vsix$|Fleet\.app/'
   ```

   The command should print nothing.

9. Run the history exposure audit on the history you intend to publish:

   ```sh
   ./scripts/history-release-check.sh docs/release/OWNER_DECISION_RECORD.md
   ```

   If it fails, either publish a cleaned/squashed first public branch using
   `./scripts/prepare-public-branch.sh <public-branch> <source-ref>` or approve
   the owner decision record choice that accepts current branch history exposure.

10. Run the secret exposure audit on the history you intend to publish:

   ```sh
   ./scripts/secret-release-check.sh
   ```

   If it fails, remove the secret from the tracked tree and publish rewritten or
   squashed history. Do not treat credential-looking history as an accepted
   public-alpha exception.

   For the recommended cleaned-history path, use
   `./scripts/check-public-release-branch.sh <public-branch> <source-ref-sha>`;
   it verifies the public branch tree and reruns both audits against the public
   branch.

11. Run the normal GitHub "CI" workflow and the manual GitHub "Release
   Readiness" workflow on the exact commit you intend to publish. Put the run
   links in the release notes or publication checklist, not in tracked evidence
   files.

12. Review the exact GitHub repository URL, repository settings, security
   settings, branch-protection status, and release-custody owner before changing
   visibility or cutting a tag. Record the checked values in the publication
   checklist or release notes, not in tracked repo-local markdown records.

13. Create a signed git tag after the final public-branch verifier passes:

   ```sh
   git tag -s v0.1.0-alpha.1 -m "Fleet v0.1.0-alpha.1"
   ```

   Use an annotated tag if signing is not configured, but record that choice in
   the release notes.

14. Generate release notes from the approved owner decisions and concrete
   release checks:

   ```sh
   ./scripts/generate-alpha-release-notes.sh v0.1.0-alpha.1 <source-ref> path/to/release-notes.md
   ```

   Add `change=...` and `rough-edge=...` arguments if the generated defaults do
   not fully describe the first alpha. The generator refuses to run until the
   owner decisions and release checks pass.

15. Validate the drafted release notes against the public commit:

   ```sh
   ./scripts/check-release-notes.sh path/to/release-notes.md <public-root-commit>
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
not acceptable, create a single-commit public branch.

Create the clean public branch from the reviewed source commit, then verify it
from the release-prep branch. The verifier requires the clean public branch tree
to match the reviewed source tree and reruns history, secret, and aggregate
release gates against the public branch.

```sh
./scripts/prepare-public-branch.sh public-alpha HEAD
./scripts/check-public-release-branch.sh public-alpha "$(git rev-parse HEAD)"
```

If `public-alpha` already exists locally during release-prep iteration, refresh
it explicitly:

```sh
FLEET_PUBLIC_BRANCH_FORCE=1 ./scripts/prepare-public-branch.sh public-alpha HEAD
```

`./scripts/check-public-release-branch.sh` runs the history, secret, and
aggregate release gates against the cleaned public branch so the audits match
the history that will be pushed to GitHub.

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
