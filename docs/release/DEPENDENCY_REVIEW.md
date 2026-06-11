# Dependency Review

This is the lightweight dependency review expected for a source-only public
alpha. It is not a substitute for a full legal review, but it keeps the first
public release from depending on hidden or unexamined package changes.

## When to Run

Run this before public GitHub visibility and before every source alpha tag.

## Inputs to Review

- Rust workspace manifests and `Cargo.lock`.
- Standalone host manifest: `crates/fleet-host/Cargo.toml`.
- npm manifests and lockfiles under `packages/*`.
- GitHub Actions workflows that fetch third-party actions.
- Bundled artifacts produced by release scripts, especially bridge VSIX files and
  app bundles.

## Commands

From the repository root:

```sh
cargo tree --workspace --all-features
cargo metadata --format-version 1 --locked > /tmp/fleet-cargo-metadata.json
```

For JavaScript packages:

```sh
( cd packages/fleet-bridge && npm ci && npm audit --audit-level=moderate )
( cd packages/extension && npm ci && npm audit --audit-level=moderate )
```

For generated release artifacts:

```sh
./scripts/release-check.sh
git ls-files | rg '(^|/)coverage/|(^|/)node_modules/|(^|/)out/|\.vsix$|Fleet\.app/'
```

The second command should print nothing.

## What to Record

For `DEPENDENCY_REVIEW_EVIDENCE.md` and the alpha release notes, record:

- date reviewed,
- commit SHA,
- dependency commands run,
- any ignored vulnerability or license findings and why they are acceptable for
  alpha,
- whether the release is source-only or includes any bundled binary artifacts.

Use [ALPHA_RELEASE_NOTES_TEMPLATE.md](ALPHA_RELEASE_NOTES_TEMPLATE.md) so this
evidence is captured in the public GitHub pre-release body.

Run `./scripts/check-dependency-review-decision.sh` from the repository root
before approving the owner decision record.

## Current Known Gaps

- No automated cargo/npm license allowlist is enforced yet.
- Dependabot version-update coverage is configured in `.github/dependabot.yml`
  and validated by `scripts/check-dependabot-config.sh`, but that is not a
  substitute for the exact-commit dependency review evidence required here.
- npm audit requires network access and may report advisory data that changes
  over time.
- The release gate currently enforces project license readiness, but not
  third-party dependency license policy.
- Binary distribution needs a stronger dependency and notarization review before
  public app bundles are attached to releases.
