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
- [ ] Run `./scripts/check-license-decision.sh` after choosing the license to
  verify the owner record, root `LICENSE`, Rust manifests, npm manifests, and
  package lockfiles agree.
- [x] Fence package publication for source-only alpha with `publish = false` in
  Rust crates and `"private": true` in extension package manifests.
- [ ] Fill `docs/release/OWNER_DECISION_RECORD.md` for required owner choices.
- [x] Decide whether the public root README is a product README or an
  engineering spec, then make the first screen match that decision.
- [x] Add `SECURITY.md` with supported versions and report channel.
- [x] Add `CONTRIBUTING.md` and a contribution licensing policy.
- [x] Remove or redact tracked artifacts that include local paths, process
  command lines, raw logs, or failed eval output.
- [ ] Run `./scripts/history-release-check.sh` and either publish cleaned
  history or explicitly accept current branch history exposure.
- [ ] Run dependency review for the exact public branch, or explicitly accept
  skipping it in the owner decision record.
- [ ] Draft GitHub pre-release notes from
  `docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md` and remove all placeholders.
- [ ] Run `./scripts/check-release-notes.sh` on the drafted GitHub pre-release
  notes.
- [x] State the alpha support boundary: best-effort, breaking changes expected,
  not production-ready.
- [ ] Confirm package namespaces before publishing anything to crates.io,
  Open-VSX, VS Code Marketplace, npm, or GitHub Releases.
- [ ] Run `./scripts/check-namespace-decision.sh` after filling the namespace
  table to verify product name, crate names, npm package names, extension
  publisher fields, and macOS bundle id agree with the owner record.

## Recommended before public alpha, but not necessarily blocking

- [x] Add a short public quickstart for the currently working local path.
- [x] Add an architecture overview that distinguishes production code from
  research/eval scaffolding.
- [x] Add a privacy note describing local-only operation and log contents.
- [x] Add a release process for source tags and app bundles.
- [x] Add issue templates for bug reports and alpha feedback.
- [x] Add a code of conduct if you want public contributions.
- [x] Add a manual release-readiness CI workflow for the exact public ref.
- [ ] Run CI on the exact public branch after artifact cleanup.

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
- `docs/QUICKSTART.md`, `docs/ARCHITECTURE.md`, and
  `docs/release/RELEASE_PROCESS.md` are present.
- `.github/workflows/release-readiness.yml` and
  `docs/release/DEPENDENCY_REVIEW.md` are present.
- `docs/release/ALPHA_RELEASE_NOTES_TEMPLATE.md` is present so the first GitHub
  pre-release has a consistent scope, verification, history, dependency,
  security, and known-rough-edges disclosure.
- `scripts/check-release-notes.sh` validates the filled release notes draft for
  required sections and unresolved placeholders before a GitHub pre-release is
  published.
- `docs/release/OWNER_DECISION_RECORD.md` is present but still marked
  `PENDING`; `scripts/release-check.sh` requires `APPROVED` before public
  visibility.
- `scripts/check-license-decision.sh` validates that the approved owner license
  choice matches the root `LICENSE`, Rust package metadata, npm package
  metadata, and package lockfile root metadata.
- `scripts/check-namespace-decision.sh` validates that the approved namespace
  table matches locally-verifiable Rust, npm, Tauri, and extension metadata.
- Root `README.md` is now a public alpha front door; the long engineering spec
  was moved to `docs/ENGINEERING_SPEC.md`.
- `crates/fleet-host/artifacts/**` is now a local ignored artifact area; raw
  probe runs are not tracked for public release.
- `containers/fleet-env/eval/artifacts/**` is now a local ignored artifact area;
  generated eval reports/screenshots are not tracked for public release.
- `scripts/history-release-check.sh` audits full git history for local paths,
  generated outputs, logs, and raw artifacts. On the current branch it still
  fails because prior commits contain reviewed host/eval artifacts; publish a
  cleaned history or explicitly accept that exposure in the owner record.
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
