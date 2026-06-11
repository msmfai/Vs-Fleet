# Release Process

This process is for a source-only public alpha. It deliberately does not publish
signed macOS binaries, crates, npm packages, VS Code marketplace packages, Open
VSX packages, or container images.

## Preconditions

Do not publish a public alpha until these are true:

- A project license is chosen.
- A root `LICENSE` file exists.
- Rust and npm package metadata no longer declare `UNLICENSED`.
- `./scripts/release-check.sh` passes.
- CI is green on the exact public branch or commit.
- Generated artifacts, local logs, screenshots, VSIX files, app bundles, and
  machine-specific paths are not tracked.
- GitHub Private Vulnerability Reporting is enabled, or `SECURITY.md` names a
  private contact channel.
- Public namespaces are confirmed, even if packages are not published yet.

## Source Alpha Steps

1. Update release notes or the README if the supported alpha scope changed.
2. Run the core Rust checks:

   ```sh
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets --all-features -- -D warnings
   cargo test --workspace --all-targets --all-features
   ```

3. Run the standalone host checks on macOS:

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

4. Run the JavaScript checks:

   ```sh
   ( cd packages/fleet-bridge && npm ci && npm run build )
   ( cd packages/extension && npm ci && npm test )
   ```

   If using the CI path instead, require the `pnpm -r build` and `pnpm -r test`
   jobs to pass on GitHub.

5. Run the public release hygiene gate:

   ```sh
   ./scripts/release-check.sh
   ```

6. Verify the public tree has no tracked generated artifacts:

   ```sh
   git ls-files | rg '(^|/)coverage/|(^|/)node_modules/|(^|/)out/|\.vsix$|Fleet\.app/'
   ```

   The command should print nothing.

7. Create a signed git tag after checks pass:

   ```sh
   git tag -s v0.1.0-alpha.1 -m "Fleet v0.1.0-alpha.1"
   ```

   Use an annotated tag if signing is not configured, but record that choice in
   the release notes.

8. Push the tag and create a GitHub release marked pre-release. The release
   should be source-only unless binary distribution has been explicitly approved.

## First Public GitHub Publish

Before making a previously private branch public, decide whether to squash or
rewrite history. Raw artifacts were removed from the current tree, but prior
commits may still contain local paths or failed visual/eval evidence. If that is
not acceptable, publish a cleaned history rather than the full working branch.

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
