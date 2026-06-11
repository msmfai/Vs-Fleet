# Fleet Alpha Release Notes Template

Use this as the GitHub pre-release body for the first public source alpha. Copy
it into the release draft and replace every bracketed placeholder before
publishing.

## Release

- Version: `[v0.1.0-alpha.1]`
- Commit: `[full commit SHA]`
- Date: `[YYYY-MM-DD]`
- Distribution: `[source-only | source plus approved binary scope]`
- Branding: `[alpha placeholders | Fleet name stable, icon may change | name and icon stable]`
- Owner decision record: `[link to approved OWNER_DECISION_RECORD.md at this commit]`

## Alpha Scope

This alpha is intended for:

- local macOS source builds,
- local `code serve-web` sessions,
- Fleet host, hub, reporter, CLI, and bridge development/testing.

Not supported as a public alpha commitment:

- signed or notarized macOS binaries,
- crates.io, npm, VS Code Marketplace, or Open VSX publication,
- container/remote deployment as a supported user path,
- production support, stable APIs, or backwards-compatible state formats.

## Supported Platform And Toolchain

- macOS source build only.
- Rust 1.78 or newer.
- Node.js 20 and npm.
- Git.
- user-provided VS Code `code` CLI.
- Linux, Windows, and remote/container workflows are not supported alpha
  platforms.

## Roadmap And Non-Goals

- No public roadmap commitments are made during alpha.
- Issues, labels, and milestones are triage hints, not delivery promises.
- Remote/container workflows, binary packages, stable APIs, and production
  support remain non-goals unless a later owner decision approves them.

## Naming And Trademark Posture

- `Fleet` is a provisional source-alpha working name.
- This release makes no trademark claim to the `Fleet` name.
- Stable package or binary publication under Fleet namespaces is deferred until
  the owner completes the public name decision.

## Local Data And Cleanup

- Runtime data lives under `~/.fleet/run` and `~/.fleet/mux` unless
  `FLEET_RUNTIME_DIR` or `FLEET_MUX_DIR` is set.
- Manual cleanup after closing Fleet-spawned servers:
  `rm -rf ~/.fleet/run ~/.fleet/mux`.
- Quitting Fleet does not promise to delete spawned editor userdata or logs, and
  externally registered sessions are not owned by the host.

## Workflow Supply Chain

- Source-alpha GitHub Actions use read-only `GITHUB_TOKEN` permissions:
  `contents: read`.
- Release-critical workflows do not use repository secrets or publishing
  credentials.
- Tagged third-party Actions are accepted for source alpha; full SHA pinning is
  deferred until binaries, package publishing, or stricter maintainer policy.

## What Changed

- `[one-line change]`
- `[one-line change]`
- `[one-line change]`

## Verification

Record exact commands and where they ran.

- GitHub CI on exact commit: `[workflow URL or accepted exception]`
- Release readiness workflow: `[workflow URL or accepted exception]`
- Rust workspace checks: `[commands/results]`
- Fleet host checks: `[commands/results]`
- JavaScript/package checks: `[commands/results]`
- Lockfile policy: `[./scripts/check-lockfile-policy.sh result]`
- Dependency review: `[commands/results or owner-approved skip]`
- Documentation link audit: `[./scripts/check-doc-links.sh result]`
- Public tree size audit: `[./scripts/check-public-tree-size.sh result]`
- History exposure audit: `[passed | cleaned history | owner-approved current history exposure]`
- Secret exposure audit: `[./scripts/secret-release-check.sh result]`
- Release hygiene gate: `[./scripts/release-check.sh result]`

## Dependency And License Review

- Project license: `[chosen license]`
- Third-party dependency review date: `[YYYY-MM-DD]`
- Accepted advisory/license findings: `[none | list finding and rationale]`
- Package publication: `[none for source-only alpha | explicit approved scope]`

## Security And Privacy Notes

- Editor server boundary: user's local `code serve-web` only; Fleet does not
  redistribute Microsoft's VS Code Server.
- Fleet is local-first and has no intended telemetry by default.
- Logs and artifacts can contain workspace paths, local URLs, session labels,
  process command lines, and editor state.
- Vulnerability reporting path: `[GitHub Private Vulnerability Reporting | private contact]`

## Known Rough Edges

- `[known alpha limitation]`
- `[known alpha limitation]`
- `[known alpha limitation]`

## Upgrade And Rollback

- No stable upgrade path is promised during alpha.
- No auto-update channel is enabled unless explicitly approved in the owner
  decision record.
- To roll back, check out the previous tag and rebuild from source.
