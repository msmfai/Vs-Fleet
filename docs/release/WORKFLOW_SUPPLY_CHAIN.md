# GitHub Actions Supply-Chain Posture

Status: source-alpha policy; no package or binary publishing credentials in
workflows.

Current policy:

- Tagged third-party GitHub Actions are accepted for source alpha.
- `GITHUB_TOKEN` permissions are read-only: `contents: read`.
- Workflows must not reference repository secrets.
- Workflows must not request `contents: write`, `packages: write`,
  `id-token: write`, `actions: write`, or similar publish-capable permissions.
- Workflows must not publish packages, create releases, upload release assets,
  or push tags.

Third-party Actions currently used by release-critical workflows:

- `actions/checkout@v4`
- `actions/cache@v4`
- `actions/setup-node@v4`
- `dtolnay/rust-toolchain@stable`
- `pnpm/action-setup@v4`
- `taiki-e/install-action@cargo-llvm-cov`

Owner decision source: `docs/release/OWNER_DECISION_RECORD.md`, section
`GitHub Actions Supply-Chain Posture`.

Revisit this before public binaries, package publishing, workflow use of
secrets, or outside-maintainer workflow edits. A stricter future policy can pin
third-party Actions by full commit SHA.
