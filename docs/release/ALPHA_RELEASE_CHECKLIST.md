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
- [ ] Fill `docs/release/OWNER_DECISION_RECORD.md` for required owner choices.
- [x] Decide whether the public root README is a product README or an
  engineering spec, then make the first screen match that decision.
- [x] Add `SECURITY.md` with supported versions and report channel.
- [x] Add `CONTRIBUTING.md` and a contribution licensing policy.
- [x] Remove or redact tracked artifacts that include local paths, process
  command lines, raw logs, or failed eval output.
- [x] State the alpha support boundary: best-effort, breaking changes expected,
  not production-ready.
- [ ] Confirm package namespaces before publishing anything to crates.io,
  Open-VSX, VS Code Marketplace, npm, or GitHub Releases.

## Recommended before public alpha, but not necessarily blocking

- [x] Add a short public quickstart for the currently working local path.
- [x] Add an architecture overview that distinguishes production code from
  research/eval scaffolding.
- [x] Add a privacy note describing local-only operation and log contents.
- [x] Add a release process for source tags and app bundles.
- [x] Add issue templates for bug reports and alpha feedback.
- [x] Add a code of conduct if you want public contributions.
- [ ] Run CI on the exact public branch after artifact cleanup.

## Current evidence from the repository

- Root `Cargo.toml`, `crates/fleet-host/Cargo.toml`,
  `packages/*/package.json`, and `packages/*/package-lock.json` currently
  declare `UNLICENSED`.
- No root `LICENSE` is tracked yet. `SECURITY.md` and `CONTRIBUTING.md` are
  present.
- `SUPPORT.md`, `CODE_OF_CONDUCT.md`, GitHub issue templates, and a pull request
  template are present.
- `docs/QUICKSTART.md`, `docs/ARCHITECTURE.md`, and
  `docs/release/RELEASE_PROCESS.md` are present.
- `docs/release/OWNER_DECISION_RECORD.md` is present but still marked
  `PENDING`; `scripts/release-check.sh` requires `APPROVED` before public
  visibility.
- Root `README.md` is now a public alpha front door; the long engineering spec
  was moved to `docs/ENGINEERING_SPEC.md`.
- `crates/fleet-host/artifacts/**` is now a local ignored artifact area; raw
  probe runs are not tracked for public release.
- `containers/fleet-env/eval/artifacts/**` is now a local ignored artifact area;
  generated eval reports/screenshots are not tracked for public release.
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
