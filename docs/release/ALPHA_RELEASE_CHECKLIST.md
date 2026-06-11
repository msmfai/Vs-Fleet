# Alpha Release Checklist

This checklist is intentionally strict. It answers one question: "Can this be
published on GitHub without creating avoidable legal, security, privacy, or
support debt?"

## Current verdict

**Gated for public alpha.** The code is promising, but the repository still
needs the blocking items below closed or explicitly accepted before public
visibility.

## Blocking before public GitHub visibility

- [x] Choose and apply the `MIT OR Apache-2.0` project license.
- [x] Add root `LICENSE`, `LICENSE-MIT`, and `LICENSE-APACHE` files.
- [x] Apply `MIT OR Apache-2.0` to `Cargo.toml`,
  `crates/fleet-host/Cargo.toml`, and package manifests/lockfiles.
- [ ] Run `./scripts/check-license-decision.sh` after the owner record is
  approved to verify the owner record, root `LICENSE`, Rust manifests, npm
  manifests, and package lockfiles agree.
- [x] Fence package publication for source-only alpha with `publish = false` in
  Rust crates and `"private": true` in extension package manifests.
- [ ] Run `./scripts/check-distribution-decision.sh` after choosing
  distribution scope to verify source-only fences or binary-release process
  docs match the owner record.
- [ ] Answer `docs/release/PUBLIC_ALPHA_OWNER_PROMPT.md`, then copy the final
  choices into `docs/release/OWNER_DECISION_RECORD.md`.
- [ ] Review `docs/release/OWNER_RELEASE_APPROVAL.md`; do not publish if the
  owner does not accept the source-only alpha constraints listed there.
- [ ] Review `docs/release/PUBLIC_ALPHA_READINESS_ASSESSMENT.md`; do not
  publish if the first public release claims binaries, package publication,
  production support, stable APIs, or remote/container support.
- [ ] Run `./scripts/check-public-alpha-readiness-assessment.sh`.
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
- [ ] Run `./scripts/secret-release-check.sh` before public visibility, or
  `./scripts/secret-release-check.sh <public-branch>` for a cleaned first
  public branch; any credential-looking hit must be removed from the tracked
  tree and public history rather than accepted.
- [ ] Run `./scripts/release-check.sh`; it includes
  `./scripts/history-release-check.sh`, `./scripts/secret-release-check.sh`,
  and requires either cleaned history or explicit owner acceptance of current
  branch history exposure.
- [ ] If current history is not accepted, create the public branch with
  `./scripts/prepare-public-branch.sh <public-branch> <source-ref>` and run
  generate `docs/release/PUBLIC_BRANCH_EVIDENCE.md` with
  `./scripts/generate-public-branch-evidence.sh <public-branch> <source-ref> docs/release/PUBLIC_BRANCH_EVIDENCE.md`,
  then run `./scripts/check-public-release-branch.sh <public-branch> <source-ref-sha>`.
  The verifier runs the history, evidence, secret, and aggregate release gates
  against the same public ref.
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
- [ ] Confirm `docs/release/ASSET_PROVENANCE.md` has a concrete redistribution
  decision for `crates/fleet-host/icons/icon.png`, or replace the icon before
  public release.
- [ ] Choose the versioning and compatibility promise in
  `docs/release/OWNER_DECISION_RECORD.md` and run
  `./scripts/check-versioning-decision.sh`.
- [ ] Choose the community intake and moderation posture in
  `docs/release/OWNER_DECISION_RECORD.md` and run
  `./scripts/check-community-intake-decision.sh`.
- [ ] Choose release custody and maintainer authority in
  `docs/release/OWNER_DECISION_RECORD.md`, fill the Release Custody section of
  `docs/release/GITHUB_PUBLICATION_EVIDENCE.md`, and run
  `./scripts/check-release-custody-decision.sh`.
- [ ] Choose AI-assisted contribution provenance in
  `docs/release/OWNER_DECISION_RECORD.md` and run
  `./scripts/check-ai-contribution-decision.sh`.
- [ ] Choose supported platform/toolchain scope in
  `docs/release/OWNER_DECISION_RECORD.md` and run
  `./scripts/check-platform-support-decision.sh`.
- [ ] Choose public roadmap/non-goals posture in
  `docs/release/OWNER_DECISION_RECORD.md` and run
  `./scripts/check-roadmap-decision.sh`.
- [ ] Choose public name collision/trademark posture in
  `docs/release/OWNER_DECISION_RECORD.md` and run
  `./scripts/check-name-collision-decision.sh`.
- [ ] Choose local data/uninstall policy in
  `docs/release/OWNER_DECISION_RECORD.md` and run
  `./scripts/check-local-data-decision.sh`.
- [ ] Choose GitHub Actions supply-chain posture in
  `docs/release/OWNER_DECISION_RECORD.md` and run
  `./scripts/check-workflow-supply-chain-decision.sh`.
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
- [ ] Generate exact CI and Release Readiness workflow evidence with
  `./scripts/generate-public-ci-evidence.sh <branch> <ci-run-url> <release-readiness-run-url> <source-ref>`
  and run
  `./scripts/check-ci-evidence-decision.sh`.
- [ ] Run `./scripts/release-evidence-status.sh`; it must report
  `Release evidence status: COMPLETE` before approval.

## Current evidence from the repository

- Root `Cargo.toml`, `crates/fleet-host/Cargo.toml`,
  `packages/*/package.json`, and `packages/*/package-lock.json` declare
  `MIT OR Apache-2.0`.
- Rust crates set `publish = false`, and extension package manifests set
  `"private": true`; the release gate enforces those source-only alpha fences.
- Root `LICENSE`, `LICENSE-MIT`, and `LICENSE-APACHE` are tracked.
  `SECURITY.md` and `CONTRIBUTING.md` are present.
- `SUPPORT.md`, `CODE_OF_CONDUCT.md`, GitHub issue templates, and a pull request
  template are present.
- `docs/release/PUBLIC_ALPHA_READINESS_ASSESSMENT.md` records the current
  honest readiness verdict: source-only alpha after gates pass, not binaries,
  package-index publication, production support, or remote/container support.
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
- `.github/workflows/release-readiness.yml` and
  `docs/release/DEPENDENCY_REVIEW.md` are present.
- `docs/release/DEPENDENCY_REVIEW_EVIDENCE.md` records a passing cargo/npm
  dependency review for the current release-prep tree; it remains subject to
  the owner dependency-review decision and the final public-ref gate.
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
- `docs/release/ASSET_PROVENANCE.md` is present as the tracked icon
  redistribution record; it is intentionally still pending owner affirmation
  until the icon is either approved for the chosen project license or replaced.
- `docs/release/GITHUB_PUBLICATION_RUNBOOK.md` is present for the final
  repository visibility, security settings, branch protection, and pre-release
  sequence.
- `docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md` is present so the first GitHub
  pre-release has a consistent scope, verification, history, dependency,
  security, and known-rough-edges disclosure.
- `scripts/check-release-notes.sh` validates the filled release notes draft for
  required sections and unresolved placeholders before a GitHub pre-release is
  published.
- `scripts/secret-release-check.sh` scans the tracked tree and, by default, all
  reachable git history for private-key blocks and common token shapes. Passing
  a public branch name scopes the history scan to that release ref. Findings
  report only redacted commit/path/line scope.
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
- `scripts/check-community-intake-decision.sh` validates the approved public
  issue/discussion posture against the issue templates, code of conduct, and
  GitHub publication runbook, so alpha users do not infer a broad support forum
  or public vulnerability-reporting surface.
- `scripts/check-release-custody-decision.sh` validates who can push public
  release tags, create GitHub releases, change repository settings, or publish
  packages, so the first public alpha has a clear supply-chain custody boundary.
- `scripts/check-ai-contribution-decision.sh` validates that AI-assisted
  outside contributions have explicit human-review, right-to-submit, and
  private-prompt/log/artifact provenance boundaries before public PR intake.
- `scripts/check-platform-support-decision.sh` validates that the public alpha
  does not imply unsupported Linux, Windows, remote/container, or binary-package
  support beyond the approved OS/toolchain matrix.
- `scripts/check-roadmap-decision.sh` validates that public issues, labels, and
  milestones are not presented as delivery promises unless the owner publishes
  a concrete roadmap policy.
- `scripts/check-name-collision-decision.sh` validates that `Fleet` is either
  presented as a provisional working name with no trademark claim, replaced
  before public visibility, or backed by a concrete owner-reviewed naming
  clearance record.
- `scripts/check-local-data-decision.sh` validates that public docs identify
  source-alpha local runtime paths, manual cleanup commands, environment
  overrides, and process ownership boundaries before users run Fleet locally.
- `scripts/check-workflow-supply-chain-decision.sh` validates that release-
  critical GitHub Actions use the approved third-party Action pinning posture,
  read-only workflow token permissions, no secrets, and no publishing
  credentials.
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
  source-icon contract, and rejects unresolved tracked-icon provenance. The
  release-notes checker then requires the final GitHub pre-release to use a
  concrete branding value.
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
- `scripts/release-check.sh` also runs `scripts/secret-release-check.sh` against
  the same ref as `FLEET_RELEASE_HISTORY_REF`; secret exposure is not an
  owner-accepted exception path.
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
