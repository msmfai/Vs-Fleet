# Alpha Release Checklist

This checklist is intentionally strict. It answers one question: "Can this be
published on GitHub without creating avoidable legal, security, privacy, or
support debt?"

## Current verdict

**Not ready for public alpha yet.** The code is promising, but the repository is
too rough as a public open-source project until the blocking items below are
closed or explicitly accepted.

## Blocking before public GitHub visibility

- [ ] Choose and apply a project license.
- [ ] Add a root `LICENSE` file.
- [ ] Replace `UNLICENSED` in `Cargo.toml`, `crates/fleet-host/Cargo.toml`, and
  package manifests/lockfiles.
- [ ] After the license decision is approved and real license text is ready, run
  `./scripts/apply-license-decision.sh docs/release/OWNER_DECISION_RECORD.md . path/to/LICENSE`
  to apply the SPDX expression to release metadata.
- [ ] Run `./scripts/check-license-decision.sh` after choosing the license to
  verify the owner record, root `LICENSE`, Rust manifests, npm manifests, and
  package lockfiles agree.
- [x] Fence package publication for source-only alpha with `publish = false` in
  Rust crates and `"private": true` in extension package manifests.
- [ ] Run `./scripts/check-distribution-decision.sh` after choosing
  distribution scope to verify source-only fences or binary-release process
  docs match the owner record.
- [ ] Answer `docs/release/PUBLIC_ALPHA_OWNER_PROMPT.md`, then copy the final
  choices into `docs/release/OWNER_DECISION_RECORD.md`.
- [ ] Optionally run
  `./scripts/draft-owner-decisions.sh <github-owner> <github-repo> docs/release/OWNER_DECISION_RECORD.draft.md`
  to create a PENDING review draft with recommended source-alpha defaults.
- [ ] Run `./scripts/public-alpha-decision-packet.sh` and confirm it reports
  `Release readiness: OWNER DECISIONS COMPLETE` before marking the owner record
  approved.
- [ ] Choose the supported source-alpha scope and run
  `./scripts/check-alpha-scope-decision.sh`.
- [ ] Choose the editor server licensing boundary and run
  `./scripts/check-editor-server-boundary-decision.sh`.
- [x] Decide whether the public root README is a product README or an
  engineering spec, then make the first screen match that decision.
- [x] Add `SECURITY.md` with supported versions and report channel.
- [ ] Run `./scripts/check-security-reporting-decision.sh` after choosing the
  security reporting channel to verify `SECURITY.md` matches the owner record.
- [x] Add `CONTRIBUTING.md` and a contribution licensing policy.
- [ ] Run `./scripts/check-contribution-decision.sh` after choosing the
  contribution intake policy to verify `CONTRIBUTING.md` and the PR template
  match the owner record.
- [x] Remove or redact tracked artifacts that include local paths, process
  command lines, raw logs, or failed eval output.
- [x] Add a redacted secret exposure gate for the tracked tree and git history.
- [ ] Run `./scripts/secret-release-check.sh` before public visibility; any
  credential-looking hit must be removed from the tracked tree and public
  history rather than accepted.
- [ ] Run `./scripts/release-check.sh`; it includes
  `./scripts/history-release-check.sh`, `./scripts/secret-release-check.sh`,
  and requires either cleaned history or explicit owner acceptance of current
  branch history exposure.
- [ ] If current history is not accepted, create the public branch with
  `./scripts/prepare-public-branch.sh <public-branch> <source-ref>` and run
  `./scripts/history-release-check.sh docs/release/OWNER_DECISION_RECORD.md <public-branch>`.
  In the same private clone, run
  `FLEET_RELEASE_HISTORY_REF=<public-branch> ./scripts/release-check.sh` for the
  aggregate gate.
- [ ] Run `./scripts/run-dependency-review.sh` for the exact public branch, or
  explicitly accept skipping dependency review in the owner decision record.
- [ ] Record dependency review evidence in
  `docs/release/DEPENDENCY_REVIEW_EVIDENCE.md` and run
  `./scripts/check-dependency-review-decision.sh`.
- [x] Add Dependabot version-update coverage for GitHub Actions, Cargo, and npm
  dependency surfaces.
- [ ] Run `./scripts/check-dependabot-config.sh .github/dependabot.yml`.
- [x] Track release-critical Rust, pnpm, and npm lockfiles.
- [ ] Run `./scripts/check-lockfile-policy.sh` to verify exact-commit
  dependency inputs are tracked and not ignored.
- [ ] Choose the support commitment in
  `docs/release/OWNER_DECISION_RECORD.md` and run
  `./scripts/check-support-decision.sh`.
- [ ] Choose whether the `Fleet` name and icon are alpha placeholders or stable
  public assets in `docs/release/OWNER_DECISION_RECORD.md`.
- [ ] Run `./scripts/check-branding-decision.sh` to verify the branding choice,
  release-notes Branding field, and replaceable source-icon contract agree.
- [ ] Choose the versioning and compatibility promise in
  `docs/release/OWNER_DECISION_RECORD.md` and run
  `./scripts/check-versioning-decision.sh`.
- [ ] Draft GitHub pre-release notes from
  `docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md` and remove all placeholders.
- [ ] Run the release-notes checker with the expected commit:
  `./scripts/check-release-notes.sh path/to/release-notes.md "$(git rev-parse HEAD)"`.
- [ ] Walk through `docs/release/GITHUB_PUBLICATION_RUNBOOK.md` before changing
  repository visibility or creating the public pre-release.
- [ ] Record GitHub repository settings evidence in
  `docs/release/GITHUB_PUBLICATION_EVIDENCE.md` and run
  `./scripts/check-github-publication-evidence.sh` before changing repository
  visibility.
- [x] State the alpha support boundary: best-effort, breaking changes expected,
  not production-ready.
- [ ] Choose the privacy/telemetry posture in
  `docs/release/OWNER_DECISION_RECORD.md` and run
  `./scripts/check-privacy-decision.sh`.
- [ ] Confirm package namespaces before publishing anything to crates.io,
  Open-VSX, VS Code Marketplace, npm, or GitHub Releases.
- [ ] After the namespace decision is approved, run
  `./scripts/apply-namespace-decision.sh docs/release/OWNER_DECISION_RECORD.md .`
  to update product, bundle, extension publisher, extension package, and
  lockfile metadata. Rust crate renames remain a manual migration.
- [ ] Run `./scripts/check-namespace-decision.sh` after filling the namespace
  table to verify product name, crate names, npm package names, extension
  publisher fields, package lockfiles, host bridge-extension lookup, and macOS
  bundle id agree with the owner record.

## Recommended before public alpha, but not necessarily blocking

- [x] Add a short public quickstart for the currently working local path.
- [x] Add an architecture overview that distinguishes production code from
  research/eval scaffolding.
- [x] Add a privacy note describing local-only operation and log contents.
- [x] Add a release process for source tags and app bundles.
- [x] Add issue templates for bug reports and alpha feedback.
- [x] Add an intake-template gate for issue/PR privacy, security, and alpha
  scope warnings.
- [ ] Run `./scripts/check-github-intake-templates.sh`.
- [x] Add a documentation link gate for tracked Markdown files.
- [ ] Run `./scripts/check-doc-links.sh` before publishing release-facing docs.
- [x] Add a public tree size gate to catch accidental large tracked artifacts.
- [ ] Run `./scripts/check-public-tree-size.sh`; only the replaceable source
  icon has a narrow larger-file allowance.
- [x] Add a code of conduct if you want public contributions.
- [x] Add a manual release-readiness CI workflow for the exact public ref.
- [x] Add a workflow integrity gate for normal CI and Release Readiness.
- [ ] Run `./scripts/check-github-workflows.sh .github/workflows/ci.yml
  .github/workflows/release-readiness.yml`.
- [ ] Run CI on the exact public branch after artifact cleanup.
- [ ] Record exact CI and Release Readiness workflow evidence in
  `docs/release/PUBLIC_CI_EVIDENCE.md` and run
  `./scripts/check-ci-evidence-decision.sh`.

## Current evidence from the repository

- Root `Cargo.toml`, `crates/fleet-host/Cargo.toml`,
  `packages/*/package.json`, and `packages/*/package-lock.json` currently
  declare `UNLICENSED`.
- Rust crates set `publish = false`, and extension package manifests set
  `"private": true`; the release gate enforces those source-only alpha fences.
- No root `LICENSE` is tracked yet. `SECURITY.md` and `CONTRIBUTING.md` are
  present.
- `SUPPORT.md`, `CODE_OF_CONDUCT.md`, GitHub issue templates, and a pull request
  template are present.
- `scripts/check-github-intake-templates.sh` validates that GitHub issue/PR
  templates keep blank issues disabled, preserve alpha scope, redirect
  vulnerability details away from public issues, and require contribution
  hygiene/test evidence.
- `scripts/check-doc-links.sh` validates tracked Markdown links to local files
  so the public GitHub front door does not ship with broken relative docs.
- `scripts/check-public-tree-size.sh` rejects tracked files over 1 MiB, except
  `crates/fleet-host/icons/icon.png`, which is allowed up to 5 MiB as the
  replaceable source icon.
- `docs/QUICKSTART.md`, `docs/ARCHITECTURE.md`, and
  `docs/release/RELEASE_PROCESS.md` are present.
- `.github/workflows/release-readiness.yml`,
  `docs/release/DEPENDENCY_REVIEW.md`, and
  `docs/release/DEPENDENCY_REVIEW_EVIDENCE.md` are present.
- `.github/dependabot.yml` is present for GitHub Actions, root Cargo workspace,
  standalone Fleet host Cargo crate, and both npm packages; the release gate
  validates those entries with `scripts/check-dependabot-config.sh`.
- `scripts/check-lockfile-policy.sh` requires root `Cargo.lock`, standalone
  `crates/fleet-host/Cargo.lock`, root `pnpm-lock.yaml`, and npm package locks
  to be tracked and not ignored.
- `.github/workflows/ci.yml` and `.github/workflows/release-readiness.yml` are
  present; `scripts/check-github-workflows.sh` validates that the public-alpha
  evidence workflows still contain the expected Rust, package, coverage, host
  bundle, release gate, and artifact checks.
- `docs/release/PUBLIC_CI_EVIDENCE.md` is present as the exact commit, branch,
  CI workflow run, and Release Readiness workflow run evidence record for the
  first public GitHub alpha.
- `docs/release/GITHUB_PUBLICATION_RUNBOOK.md` is present for the final
  repository visibility, security settings, branch protection, and pre-release
  sequence.
- `docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md` is present so the first GitHub
  pre-release has a consistent scope, verification, history, dependency,
  security, and known-rough-edges disclosure.
- `scripts/check-release-notes.sh` validates the filled release notes draft for
  required sections and unresolved placeholders before a GitHub pre-release is
  published.
- `scripts/secret-release-check.sh` scans the tracked tree and all reachable
  git history for private-key blocks and common token shapes, reports only
  redacted commit/path/line findings, and passes on the current scanned refs.
- `docs/release/OWNER_DECISION_RECORD.md` is present but still marked
  `PENDING`; `scripts/release-check.sh` requires `APPROVED` before public
  visibility and now requires a branding stability choice before public alpha.
- `scripts/check-license-decision.sh` validates that the approved owner license
  choice matches the root `LICENSE`, Rust package metadata, npm package
  metadata, and package lockfile root metadata.
- `scripts/apply-namespace-decision.sh` applies approved namespace choices to
  stable manifest targets, and `scripts/check-namespace-decision.sh` validates
  that the approved namespace table matches locally-verifiable Rust, npm,
  Tauri, bundle, lockfile, host bridge lookup, and extension metadata.
- `scripts/check-alpha-scope-decision.sh` validates that the approved source
  alpha scope matches the README, quickstart, architecture overview, and release
  notes template.
- `scripts/check-editor-server-boundary-decision.sh` validates that the
  approved editor server boundary matches public docs, so the source alpha does
  not imply Fleet redistributes Microsoft's VS Code Server or Marketplace
  components.
- `scripts/check-distribution-decision.sh` validates that the approved
  distribution scope matches source-only package fences, release docs, and any
  binary release process required for app-bundle distribution.
- `scripts/check-security-reporting-decision.sh` validates that the approved
  security reporting choice matches `SECURITY.md`, so the public repo does not
  publish ambiguous "once enabled" vulnerability reporting instructions.
- `scripts/check-contribution-decision.sh` validates that the approved
  contribution intake choice matches `CONTRIBUTING.md` and the pull request
  template before outside code PRs arrive.
- `scripts/check-ci-evidence-decision.sh` validates that the approved public CI
  evidence choice has exact commit evidence and, for the recommended path,
  GitHub Actions run URLs for both normal CI and Release Readiness.
- `scripts/check-github-publication-evidence.sh` validates the exact GitHub
  repository URL against the approved namespace and requires concrete evidence
  for visibility review, issues/discussions/wiki/releases/packages settings,
  Actions, security settings, and branch protection.
- `scripts/check-versioning-decision.sh` validates the approved compatibility
  promise against the release notes, support/security docs, and release process,
  so alpha users do not infer stable APIs, state formats, or upgrade paths by
  accident.
- `scripts/check-privacy-decision.sh` validates that the approved
  privacy/telemetry posture matches the README, security policy, architecture
  notes, issue template, and release notes template.
- `scripts/check-dependency-review-decision.sh` validates that the approved
  dependency review choice has exact commit evidence and explicit command
  results or an accepted skipped-review risk.
- `scripts/check-support-decision.sh` validates that the approved support
  commitment matches `SUPPORT.md`, the README, and the release notes template.
- `scripts/check-branding-decision.sh` validates the approved branding
  stability choice against the release-notes Branding field and the replaceable
  source-icon contract. The release-notes checker then requires the final
  GitHub pre-release to use a concrete branding value.
- Root `README.md` is now a public alpha front door; the long engineering spec
  was moved to `docs/ENGINEERING_SPEC.md`.
- `crates/fleet-host/artifacts/**` is now a local ignored artifact area; raw
  probe runs are not tracked for public release.
- `containers/fleet-env/eval/artifacts/**` is now a local ignored artifact area;
  generated eval reports/screenshots are not tracked for public release.
- `scripts/release-check.sh` runs `scripts/history-release-check.sh`, which
  audits full git history for local paths, generated outputs, logs, and raw
  artifacts. On the current branch it still fails because prior commits contain
  reviewed host/eval artifacts; publish a cleaned history or explicitly accept
  that exposure in the owner record.
- `scripts/release-check.sh` also runs `scripts/secret-release-check.sh`; secret
  exposure is not an owner-accepted exception path.
- The GitHub CI workflow exists, but should be re-run after the public tree is
  cleaned.

## "Too rough?" assessment

Yes, for a general public alpha today. It is suitable for a private or
invite-only technical preview where users know they are looking at an active
research/build tree. It is not yet suitable as an open-source project front door:
the legal state is unresolved, package namespaces still need confirmation, and
prior local artifacts may need history cleanup before first public GitHub
visibility.

Once the blocking items are closed, a source-only alpha is reasonable. Binary
distribution should wait until signing/notarization and packaging expectations
are deliberate.
