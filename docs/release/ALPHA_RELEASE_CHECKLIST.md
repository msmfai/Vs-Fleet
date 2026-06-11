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
  package manifests.
- [x] Decide whether the public root README is a product README or an
  engineering spec, then make the first screen match that decision.
- [ ] Add `SECURITY.md` with supported versions and report channel.
- [ ] Add `CONTRIBUTING.md` and a contribution licensing policy.
- [ ] Remove or redact tracked artifacts that include local paths, process
  command lines, raw logs, or failed eval output.
- [ ] State the alpha support boundary: best-effort, breaking changes expected,
  not production-ready.
- [ ] Confirm package namespaces before publishing anything to crates.io,
  Open-VSX, VS Code Marketplace, npm, or GitHub Releases.

## Recommended before public alpha, but not necessarily blocking

- [ ] Add a short public quickstart for the currently working local path.
- [ ] Add an architecture overview that distinguishes production code from
  research/eval scaffolding.
- [ ] Add a privacy note describing local-only operation and log contents.
- [ ] Add a release process for source tags and app bundles.
- [ ] Add issue templates for bug reports and alpha feedback.
- [ ] Add a code of conduct if you want public contributions.
- [ ] Run CI on the exact public branch after artifact cleanup.

## Current evidence from the repository

- Root `Cargo.toml` and `packages/*/package.json` currently declare
  `UNLICENSED`.
- No root `LICENSE`, `SECURITY.md`, or `CONTRIBUTING.md` is tracked.
- Root `README.md` is now a public alpha front door; the long engineering spec
  was moved to `docs/ENGINEERING_SPEC.md`.
- `crates/fleet-host/artifacts/**` contains source-controlled probe evidence,
  including raw logs/RSS/JSON with local absolute paths.
- `containers/fleet-env/eval/artifacts/**` contains a large generated eval
  result corpus, including at least one failed result from an older run.
- The GitHub CI workflow exists, but should be re-run after the public tree is
  cleaned.

## "Too rough?" assessment

Yes, for a general public alpha today. It is suitable for a private or
invite-only technical preview where users know they are looking at an active
research/build tree. It is not yet suitable as an open-source project front door:
the legal state is unresolved, the README does not set user expectations, and
tracked generated evidence leaks local implementation details.

Once the blocking items are closed, a source-only alpha is reasonable. Binary
distribution should wait until signing/notarization and packaging expectations
are deliberate.
