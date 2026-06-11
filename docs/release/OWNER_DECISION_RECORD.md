# Owner Decision Record

Decision record status: PENDING

Do not publish the repository publicly until this file has explicit owner
choices for every "Required before public GitHub visibility" item.

This file is intentionally separate from the recommendation docs: it is the
place where the project owner records the actual decision, not what the release
prep work guessed.

## Required Before Public GitHub Visibility

### 1. License

Choose one and then apply it to `LICENSE`, Rust manifests, npm manifests, and
lockfiles.

- [x] MIT OR Apache-2.0 dual license.
- [ ] MIT only.
- [ ] Apache-2.0 only.
- [ ] AGPL-3.0-only.
- [ ] Other: `TODO`

Current default recommendation: MIT OR Apache-2.0 for a permissive Rust-friendly
alpha. Keep reusable library/API crates permissive; reserve AGPL-3.0-only plus
a commercial exception as a future CLI/hosted-control-plane contingency only
after a concrete monetization trigger.

### 2. Public History

Choose how the first public GitHub history should look.

- [ ] Publish the current branch history and accept that old commits may contain
  removed local artifacts or failed eval evidence.
- [ ] Publish a cleaned/squashed history for the first public branch.

Current default recommendation: cleaned/squashed history before first public
visibility.

### 3. Public Namespace

Fill these before publishing packages or telling users names are stable.

| Surface | Decision |
|---|---|
| GitHub org/user | `TODO` |
| GitHub repo name | `TODO` |
| Product name | `Fleet` or `TODO` |
| Rust crate prefix | `fleet-*` or `TODO` |
| npm package names | `fleet-extension`, `fleet-bridge`, or `TODO` |
| VS Code Marketplace publisher | `fleet-team` or `TODO` |
| Open VSX publisher | `fleet-team` or `TODO` |
| macOS bundle id | `dev.fleet.host` or `TODO` |

Current default recommendation: confirm the GitHub repo and bundle id now; defer
marketplace/crates/npm publication until after source alpha.

### 4. Alpha Scope

Choose what public users can treat as the supported source-alpha surface.

- [ ] Local macOS Fleet host plus local `code serve-web` sessions, Fleet bridge,
  Fleet reporter, CLI, and embedded local Hub. Remote, SSH, Docker/container,
  visual probe, and eval harness paths remain development infrastructure, not
  public support commitments.
- [ ] Broaden public alpha scope to include remote, SSH, Docker/container, or
  eval harness paths as supported user workflows.
- [ ] Other: `TODO`

Current default recommendation: keep the first public alpha scoped to the local
macOS host and local `code serve-web` workflow.

### 5. Editor Server Licensing Boundary

Choose how the public alpha handles editor server components.

- [ ] User-provided VS Code only. Fleet may launch the user's local
  `code serve-web` install, but Fleet does not download, bundle, host, or
  redistribute Microsoft's VS Code Server, Microsoft Marketplace extensions, or
  Microsoft remote extensions.
- [ ] OSS server only. Supported workflows use `code-server` or
  `openvscode-server` with Open VSX; no Microsoft VS Code Server or Marketplace
  dependency.
- [ ] Other: `TODO`

Current default recommendation: user-provided VS Code only for the local source
alpha; require an OSS server/Open VSX path before supporting deployed remote or
container workflows.

### 6. Distribution Scope

Choose what the first public alpha promises.

- [ ] Source-only alpha. No public app bundle, crates.io, npm, Open VSX, VS Code
  Marketplace, or container image publishing.
- [ ] Source plus unsigned macOS app bundle.
- [ ] Source plus signed/notarized macOS app bundle.
- [ ] Other: `TODO`

Current default recommendation: source-only alpha.

### 7. Security Reporting Channel

Choose the private vulnerability path before public visibility.

- [ ] Enable GitHub Private Vulnerability Reporting.
- [ ] Add a private security email/contact to `SECURITY.md`.
- [ ] Other: `TODO`

Current default recommendation: GitHub Private Vulnerability Reporting.

### 8. Contribution Intake

Choose how to handle first outside PRs after the license is applied.

- [ ] Accept small focused PRs under the chosen project license using the PR
  template certification.
- [x] Require DCO sign-off.
- [ ] Keep code PRs closed; accept issues and docs feedback only.
- [ ] Other: `TODO`

Current default recommendation: require DCO sign-off for outside code
contributions; no CLA for source alpha. Revisit CLA before accepting code if
commercial exceptions or proprietary relicensing become a goal.

### 9. Public CI Evidence

Choose the CI gate for the public branch.

- [ ] Require GitHub Actions green on the exact branch/commit before public
  visibility.
- [ ] Accept local check output only for the first publish.
- [ ] Other: `TODO`

Current default recommendation: require GitHub Actions green on the exact public
branch/commit.

### 10. Privacy And Telemetry Posture

Choose the privacy/logging promise before public visibility.

- [ ] No telemetry by default. Local logs and artifacts may contain workspace
  paths, local URLs, session labels, process command lines, and editor state;
  users must scrub them before sharing.
- [ ] Add an explicit telemetry or remote reporting disclosure before public
  visibility.
- [ ] Other: `TODO`

Current default recommendation: no telemetry by default, with explicit local log
contents and scrub-before-sharing warnings in public docs.

### 11. Dependency Review Evidence

Choose what dependency evidence is required for the exact public commit.

- [ ] Run the dependency review commands in `docs/release/DEPENDENCY_REVIEW.md`
  and record findings in the release notes.
- [ ] Publish the first source alpha without dependency review and accept
  advisory/license review risk.
- [ ] Other: `TODO`

Current default recommendation: run and record the dependency review before
public visibility; do not defer it unless the release is deliberately
invite-only.

### 12. Support Commitment

Choose what support public alpha users can expect.

- [ ] Best-effort alpha support only. Breaking changes are expected; there are
  no production support guarantees, response SLAs, paid support terms, or stable
  release lines.
- [ ] Define a public triage or response target in `SUPPORT.md`.
- [ ] Other: `TODO`

Current default recommendation: best-effort alpha support only.

### 13. Branding Stability

Choose how stable the public alpha name and visual identity are.

- [ ] `Fleet` name and current icon are alpha placeholders.
- [ ] `Fleet` name is stable, icon may change.
- [ ] Name and icon are stable.
- [ ] Other: `TODO`

Current default recommendation: treat the icon as alpha and confirm the name
only after the namespace check passes.

### 14. Versioning And Compatibility

Choose what compatibility public alpha users can expect.

- [ ] Alpha pre-release tags only. No stable API, protocol, state-file, or
  upgrade compatibility is promised during alpha.
- [ ] Commit to semver-compatible public CLI, protocol, and state changes during
  alpha.
- [ ] Other: `TODO`

Current default recommendation: use alpha pre-release tags and promise no stable
API, protocol, state-file, or upgrade compatibility until the project has real
outside users.

### 15. Community Intake And Moderation

Choose what public discussion surfaces are open during alpha and how they are
moderated.

- [ ] Open public issues only for scoped bug reports and alpha feedback; keep
  blank issues disabled and keep discussions off unless explicitly enabled
  later.
- [ ] Keep public issues and discussions closed during alpha; collect feedback
  privately or by invite only.
- [ ] Other: `TODO`

Current default recommendation: open only the scoped bug and alpha-feedback
templates for source alpha; keep blank issues disabled and keep Discussions off
until there is maintainer capacity for broad community support.

### 16. Release Custody And Maintainer Authority

Choose who can create public release artifacts or change public repository
controls during alpha.

- [ ] Single-maintainer alpha. Only the repository owner or named maintainer may
  push release tags, create GitHub releases, change repository settings, or
  publish packages.
- [ ] Multi-maintainer governance before public alpha.
- [ ] Other: `TODO`

Current default recommendation: single-maintainer alpha with source tags and
release notes only, no package publishing credentials, and explicit evidence for
tag protection or an accepted unavailable/deferred reason.

### 17. AI-Assisted Contribution Provenance

Choose how public alpha handles AI-assisted or model-generated outside
contributions.

- [ ] Allow AI-assisted contributions if the contributor certifies human review,
  right to submit, and no private prompts, logs, or generated artifacts.
- [ ] Require maintainer approval before accepting AI-generated code or
  model-generated patches.
- [ ] Other: `TODO`

Current default recommendation: allow AI-assisted contributions only with human
review, right-to-submit certification, and explicit exclusion of private prompts,
private model transcripts, local logs, workspace paths, and generated artifacts.

### 18. Supported Platform And Toolchain

Choose the OS and toolchain matrix that public alpha users can expect.

- [ ] macOS source alpha only. Supported toolchain: Rust 1.78 or newer,
  Node.js 20/npm, Git, and user-provided VS Code code CLI/serve-web.
- [ ] Publish a broader OS/toolchain support matrix before public alpha.
- [ ] Other: `TODO`

Current default recommendation: macOS source alpha only. Do not imply Linux,
Windows, remote/container, or binary-package support until each path has a
documented support matrix and public verification evidence.

### 19. Public Roadmap And Non-Goals

Choose what public users can infer from issues, labels, milestones, and alpha
feedback during the first source alpha.

- [ ] No public roadmap commitments during alpha. Issues, labels, and
  milestones are triage hints only, not delivery promises.
- [ ] Publish a public roadmap before alpha.
- [ ] Other: `TODO`

Current default recommendation: no public roadmap commitments during alpha.
Release notes should list known rough edges and non-goals; issues, labels, and
milestones are triage signals only.

### 20. Public Name Collision And Trademark Posture

Choose how the public alpha handles the `Fleet` name before users, package
indexes, forks, or screenshots treat it as stable.

- [ ] Use `Fleet` only as a provisional source-alpha working name. Make no
  trademark claim, acknowledge name-collision review is unresolved, and do not
  publish packages or binaries under stable Fleet namespaces.
- [ ] Rename the product and package namespaces before public visibility.
- [ ] Owner has reviewed name/trademark clearance and accepts using `Fleet`
  publicly.
- [ ] Other: `TODO`

Current default recommendation: treat `Fleet` as a provisional working name for
source alpha, make no trademark claim, and defer stable package/binary namespace
publication until the owner either clears the name or renames it.

### 21. Local Data And Uninstall Policy

Choose what public source-alpha users are promised about local files, logs,
runtime sockets, spawned editor userdata, and cleanup.

- [ ] Document local data locations and manual cleanup for source alpha. Fleet
  does not promise an automated uninstaller, but public docs identify
  `~/.fleet/run`, `~/.fleet/mux`, cleanup commands, and the process ownership
  boundary.
- [ ] Add an automated cleanup or uninstall command before public visibility.
- [ ] Other: `TODO`

Current default recommendation: document the manual cleanup contract for source
alpha. Do not imply that quitting Fleet removes spawned editor data, logs, or
external sessions.

### 22. GitHub Actions Supply-Chain Posture

Choose how strict the first public alpha is about third-party GitHub Actions,
workflow token permissions, secrets, and publishing credentials.

- [ ] Tagged third-party GitHub Actions are accepted for source alpha, but
  workflows must use read-only `GITHUB_TOKEN` permissions, no repository
  secrets, and no package/release publishing credentials.
- [ ] Require every third-party GitHub Action to be pinned by full commit SHA
  before public visibility.
- [ ] Other: `TODO`

Current default recommendation: accept tagged third-party Actions for source
alpha only with read-only workflow permissions, no secrets, and no publishing
credentials. Revisit full SHA pinning before binaries, package publishing, or
outside-maintainer workflow edits.

## Required Before Binary Distribution

These do not block a source-only alpha, but they must be decided before any
public app bundle.

### 23. macOS Signing and Notarization

- [ ] No public binaries until Developer ID signing and notarization are
  automated.
- [ ] Publish unsigned binaries and document Gatekeeper warnings.
- [ ] Other: `TODO`

Current default recommendation: no public binaries until signing and notarization
are automated.

### 24. Update Channel

- [ ] No auto-update in alpha.
- [ ] GitHub Releases only.
- [ ] In-app updater.
- [ ] Other: `TODO`

Current default recommendation: no auto-update in alpha; GitHub Releases only
for source tags.
