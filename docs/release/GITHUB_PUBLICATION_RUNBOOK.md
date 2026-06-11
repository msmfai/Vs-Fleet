# GitHub Publication Runbook

This runbook is for the first public GitHub alpha. It is deliberately about
repository visibility and GitHub-side settings, not about building binaries or
publishing packages.

Do not make the repository public until `./scripts/release-check.sh` passes for
the exact commit that will be published.

## Inputs

Record these in `docs/release/OWNER_DECISION_RECORD.md` before using this
runbook:

- License.
- Public history strategy.
- GitHub org/user and repo name.
- Supported alpha scope.
- Editor server licensing boundary.
- Source-only distribution scope, or an explicitly approved alternative.
- Security reporting channel.
- Contribution intake policy.
- Public CI evidence policy.
- Privacy/logging posture.
- Dependency review evidence.
- Support commitment.
- Branding stability for the public alpha name and icon.

## Public Visibility

Publish from a prepared public branch, not from an arbitrary dirty working
branch.

1. Decide whether the first public branch uses current history or a cleaned /
   squashed history.
2. Run `./scripts/history-release-check.sh docs/release/OWNER_DECISION_RECORD.md`
   on the exact branch to publish.
3. Run `./scripts/secret-release-check.sh` on the exact branch to publish.
4. If the history audit fails and the owner did not explicitly accept current
   history exposure, publish a cleaned branch instead.
5. If the secret exposure audit fails, publish a cleaned branch instead. Do not
   publish credential-looking material as an accepted alpha exception.
6. Review GitHub's repository visibility consequences before changing a private
   repository to public. GitHub documents that public visibility makes code
   visible to anyone, allows anyone to fork, publishes activity, and makes
   Actions history and logs visible.

Reference: <https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/managing-repository-settings/setting-repository-visibility>

## Repository Settings

Set these before public visibility:

- Repository name matches the approved namespace table.
- Default branch is the exact public branch.
- Issues are enabled only if the support commitment allows public issue intake.
- Discussions and wiki are disabled unless deliberately supported.
- `./scripts/check-github-intake-templates.sh` passes before enabling public
  issue intake.
- Packages are not used for the source-only alpha.
- Releases are allowed for source tags and release notes only; no app bundle,
  VSIX, npm package, crate, Open VSX package, or container image is attached
  unless distribution scope explicitly changes.
- GitHub Actions is enabled for the release-readiness and source-check workflows.
- `.github/dependabot.yml` is present and `./scripts/check-dependabot-config.sh
  .github/dependabot.yml` passes before public visibility.
- `./scripts/check-github-workflows.sh .github/workflows/ci.yml
  .github/workflows/release-readiness.yml` passes before recording public CI
  evidence.
- `./scripts/secret-release-check.sh` passes on the exact public branch.

## Security Settings

Before public visibility:

- Enable GitHub Private Vulnerability Reporting if that is the approved security
  reporting channel, or make `SECURITY.md` name the approved private contact.
- Confirm `SECURITY.md` does not tell reporters to use a channel that is not
  actually enabled.
- Confirm secret scanning / push protection / Dependabot alerts are enabled if
  available for the chosen GitHub account or organization.

GitHub documents private vulnerability reporting as a repository feature that
public repository owners and administrators can enable, and notes that it is
separate from `SECURITY.md`.

Reference: <https://docs.github.com/en/code-security/how-tos/report-and-fix-vulnerabilities/report-privately>

## Branch Protection

For a public alpha, protect the default branch before inviting outside changes:

- Require pull requests for non-owner changes.
- Require the source-check workflow to pass before merging.
- Require the release-readiness workflow before release tags or public release
  notes are cut.
- Prefer linear history unless the owner deliberately wants merge commits.
- Do not require signed commits unless the maintainer workflow is already ready
  for it; otherwise it becomes a release blocker rather than a release control.

GitHub documents required status checks as branch-protection checks that must be
successful, skipped, or neutral before merging.

Reference: <https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-protected-branches/about-protected-branches>

## Final Publish Sequence

1. Update `docs/release/OWNER_DECISION_RECORD.md` to `APPROVED`.
2. Apply the approved license and namespace metadata.
3. Run the dependency review and record exact evidence.
4. Run the normal GitHub "CI" workflow and the manual GitHub "Release
   Readiness" workflow on the exact commit.
5. Record exact CI and Release Readiness evidence in
   `docs/release/PUBLIC_CI_EVIDENCE.md`.
6. Run:

   ```sh
   ./scripts/secret-release-check.sh
   ./scripts/release-check.sh
   ./scripts/check-release-notes.sh path/to/release-notes.md "$(git rev-parse HEAD)"
   ```

7. Create the public branch or change repository visibility.
8. Push the alpha source tag.
9. Create a GitHub pre-release using checked release notes.
10. Re-run `./scripts/release-check.sh` on the public checkout.

## Abort Conditions

Abort publication if any of these are true:

- `./scripts/release-check.sh` fails.
- `./scripts/secret-release-check.sh` fails.
- The exact public commit differs from the commit recorded in CI or dependency
  review evidence.
- The selected security reporting channel is not actually available.
- The release notes imply binary, package, remote/container, or production
  support that the owner decision record did not approve.
- The history audit finds local artifacts and the owner did not explicitly
  accept that exposure.
