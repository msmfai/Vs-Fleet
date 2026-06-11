# Public Alpha Readiness Assessment

This is the maintainer-facing answer to: "Is Fleet too rough to publish as an
open-source alpha?"

## Verdict

Current verdict: GATED FOR PUBLIC SOURCE ALPHA.

Fleet is credible for a source-only public alpha after the release gates pass.
It is not credible yet as:

- a general end-user product;
- a public macOS binary;
- a package-index release;
- a supported remote/container platform.

The acceptable first public shape is technical review and dogfooding by people
who can build from source, read alpha caveats, and tolerate breakage. The
unacceptable first public shape is a broad launch that implies stable APIs,
binary install support, package publication, production support, or a supported
remote deployment story.

## Why Source Alpha Is Reasonable

- The core local architecture exists: Hub, protocol, reporter, CLI, host-core,
  macOS host, and Fleet bridge.
- The public docs now explain the local-first model, editor-server boundary,
  local data locations, alpha support boundary, and provisional naming posture.
- Release gates cover license metadata, DCO/contribution posture, local data,
  issue templates, documentation links, lockfiles, public tree size, history
  exposure, secrets, dependency review evidence, CI evidence, and GitHub
  publication evidence.
- Package publishing is fenced off with Rust `publish = false` and npm
  `"private": true`.
- The README does not promise remote/container, binary, package-index,
  production-support, or stable-compatibility surfaces.

## Why It Must Stay Gated

- The owner decision record is not approved until every public-alpha choice is
  explicit.
- The public namespace table still needs concrete GitHub, package, publisher,
  product, and bundle-id decisions.
- Current private branch history contains local paths and generated/eval
  artifacts; publish a cleaned public branch unless the owner explicitly accepts
  that exposure.
- Public CI evidence and GitHub publication evidence remain pending
  release-control requirements.
- Public branch evidence and dependency review evidence are recorded for the
  current release-prep tree, but still need the matching owner decisions and
  final public-ref gates.
- GitHub repository settings, vulnerability reporting, branch protection,
  issue/discussion settings, and release/package settings must be recorded
  before the visibility change.

## Too Rough For These Claims

Do not make any of these claims for the first public alpha:

- "Install the app" or "download the binary" as a public release path.
- "Fleet supports remote machines, containers, or SSH workflows."
- "Fleet owns or manages externally started editor/agent processes."
- "Fleet provides production support, response SLAs, or stable release lines."
- "The protocol, CLI, local state, package names, or app identity are stable."
- "Fleet redistributes Microsoft's VS Code Server, Marketplace extensions, or
  remote extensions."

## Required Public Disclosures

The first public README, release notes, and support docs must disclose:

- source-only alpha;
- macOS local source build as the supported path;
- user-provided VS Code `code serve-web`;
- no telemetry by default, but local logs can include paths, command lines,
  local URLs, session labels, and editor state;
- `~/.fleet/run` and `~/.fleet/mux` local data and manual cleanup;
- best-effort support only;
- no stable API, protocol, state-file, or upgrade compatibility promise;
- provisional `Fleet` name and alpha branding unless the owner approves
  stability;
- no package-index, marketplace, binary, remote/container, or production
  support commitment.

## Decision Rule

Approve public visibility only when:

1. `docs/release/OWNER_DECISION_RECORD.md` is `APPROVED`.
2. `./scripts/public-alpha-decision-packet.sh` reports owner decisions complete.
3. `./scripts/release-evidence-status.sh` reports release evidence complete.
4. For the recommended cleaned-history path,
   `./scripts/check-public-release-branch.sh <public-branch> <source-ref-sha>`
   passes on the exact public ref. If the owner explicitly accepts current
   history exposure instead, `./scripts/release-check.sh` passes on that
   publishable ref.
5. The first public release notes state the rough edges above plainly.

Until then, the correct public-release answer is "not yet"; the correct
engineering answer is "continue hardening toward a source-only alpha."
