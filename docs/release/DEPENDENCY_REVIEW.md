# Dependency Review

This is the lightweight dependency review expected for a source-only public
alpha. It is not a substitute for a full legal review, but it keeps the first
public release from depending on hidden or unexamined package changes.

## When to Run

Run this before public GitHub visibility and before every source alpha tag.

## Inputs to Review

- Rust workspace manifests and root `Cargo.lock`.
- Standalone host manifest and lockfile:
  `crates/fleet-host/Cargo.toml` and `crates/fleet-host/Cargo.lock`.
- pnpm workspace lockfile: `pnpm-lock.yaml`.
- npm manifests and package lockfiles under `packages/*`.
- GitHub Actions workflows that fetch third-party actions.
- Bundled artifacts produced by release scripts, especially bridge VSIX files and
  app bundles.

## Command

From the repository root:

```sh
./scripts/run-dependency-review.sh
```

The script runs Rust metadata checks, npm audit checks, lockfile policy, and a
generated-artifact check. Use its terminal output and command logs while
preparing release notes. Run `./scripts/release-check.sh` afterwards as the
final local release verifier.

## What to Record

For the alpha release notes, record:

- date reviewed,
- commit SHA,
- lockfiles checked,
- dependency commands run,
- any ignored vulnerability or license findings and why they are acceptable for
  alpha,
- whether the release is source-only or includes any bundled binary artifacts.

Use `./scripts/generate-alpha-release-notes.sh` after the owner decisions and
release checks pass so the dependency review result is captured in the public
GitHub pre-release body. [ALPHA_RELEASE_NOTES_TEMPLATE.md](ALPHA_RELEASE_NOTES_TEMPLATE.md)
remains the checked content template that the generator and release-notes
checker enforce.

Run `./scripts/run-dependency-review.sh` from the repository root before
publishing the source alpha.

## Current Known Gaps

- No automated cargo/npm license allowlist is enforced yet.
- Dependabot version-update coverage is configured in `.github/dependabot.yml`
  and validated by `scripts/check-dependabot-config.sh`, but that is not a
  substitute for reviewing the exact commit being published.
- npm audit requires network access and may report advisory data that changes
  over time.
- The release gate currently enforces project license readiness, but not
  third-party dependency license policy.
- Binary distribution needs a stronger dependency and notarization review before
  public app bundles are attached to releases.
