# Public Alpha Decisions

Fleet is not ready to publish until every **Required before public alpha** item
has an explicit owner decision. These are the choices that are expensive to
change once strangers clone, package, fork, or depend on the project.

Record the actual selected choices in
[OWNER_DECISION_RECORD.md](OWNER_DECISION_RECORD.md).

## Required before public alpha

| Decision | Current state | Why it matters | Recommended alpha default |
|---|---|---|---|
| License | `UNLICENSED` in Rust crates and VS Code packages; no `LICENSE` file. | Without a license, outsiders have no clear right to use, fork, package, or contribute. Changing later can be messy if external contributors appear. | Pick either MIT/Apache-2.0 dual license for permissive Rust ecosystem norms, or AGPL-3.0-only if you want strong network-copyleft. Do not publish while `UNLICENSED`; after choosing, run `scripts/check-license-decision.sh` to verify every manifest and lockfile agrees. |
| Release scope | Mixed source tree: protocol/hub/reporter/CLI, macOS Tauri host, VS Code extensions, container eval harness, large design spec. | Users need to know what is usable today vs research/eval scaffolding. | Alpha scope should be "local macOS Fleet host + local code serve-web sessions + reporter/bridge pieces"; mark remote/deploy/container eval as experimental or internal; after choosing, run `scripts/check-alpha-scope-decision.sh` so public docs match the owner record. |
| Editor server boundary | The local source alpha launches the user's `code serve-web`; remote/deployed design docs discuss code-server/openvscode-server, Open VSX, and Microsoft VS Code Server licensing constraints. | Public users need to know Fleet is not redistributing Microsoft server or Marketplace components, and remote/deployed support should not accidentally depend on a proprietary server license. | For source alpha, use user-provided VS Code only and explicitly state no Microsoft VS Code Server or Marketplace redistribution; before remote/container support, require an OSS server/Open VSX path. After choosing, run `scripts/check-editor-server-boundary-decision.sh`. |
| Distribution | Source quickstart and release process are present; release bundle can be built locally; no signed/notarized macOS release policy. | Unsigned macOS apps trigger trust warnings. Public binary releases imply update/signing/support expectations. | Source-only alpha first. Add signed binaries later; after choosing, run `scripts/check-distribution-decision.sh` so package fences and binary-process docs match the owner record. |
| Artifact and secret policy | Raw host/eval artifacts are ignored going forward, but `scripts/history-release-check.sh` still finds prior local artifacts in branch history until history is cleaned or explicitly accepted. `scripts/secret-release-check.sh` now separately scans tracked refs for private-key blocks and common token shapes. | Public repos should not leak local paths/process details, credentials, or failed release evidence. | Before first GitHub publish, either squash/rewrite the branch so removed artifacts never appear in public history, or accept that old commits expose local paths. Credential-looking findings are not an owner-accepted exception path; clean or rewrite them before public visibility. Keep only curated, redacted screenshots in public history. |
| Security disclosure | `SECURITY.md` is present; GitHub Private Vulnerability Reporting still needs to be enabled or replaced with a private contact before publish. | Users need to know how to report issues and what versions are supported. | Enable GitHub Private Vulnerability Reporting, or document a private contact channel before public visibility; after choosing, run `scripts/check-security-reporting-decision.sh` so the owner record and `SECURITY.md` agree. |
| Privacy/logging posture | Host/reporter logs contain workspace paths, command lines, session labels, and local URLs. | This product observes developer environments; privacy expectations must be explicit. | State that Fleet is local-first, has no telemetry by default, and logs local metadata that users should scrub before sharing; after choosing, run `scripts/check-privacy-decision.sh` so public docs match the owner record. |
| Contribution policy | `CONTRIBUTING.md` and a PR template are present; broad outside code PRs are deferred until license choice is final. | First outside PR forces a licensing/provenance decision. | No CLA for alpha; require contributors to certify they can license work under the chosen project license, via DCO or simple PR statement; after choosing, run `scripts/check-contribution-decision.sh` so the docs and PR template agree. |
| Public name/namespace | Product and package names are `Fleet`, `fleet-*`, publisher `fleet-team`, bundle id `dev.fleet.host`. | Names collide easily and package namespaces can be hard to migrate. | Confirm GitHub org/repo, VS Code/Open-VSX publisher, crates package names, and macOS bundle identifier before publishing packages; run `scripts/check-namespace-decision.sh` after recording the choice. |
| Support boundary | `SUPPORT.md` states best-effort alpha, breaking changes expected, and no production support/SLA. | Alpha users need expectations; otherwise every bug can become implied support. | Keep support best-effort for source-only alpha; after choosing, run `scripts/check-support-decision.sh` so support docs match the owner record. Revisit before binaries or package publishing. |
| Branding stability | Generated icon and possibly temporary `Fleet` name. | Public assets become recognizable quickly, and users may screenshot, fork, or write docs against them. | Decide whether the `Fleet` name and icon are alpha placeholders or stable public assets before the first GitHub pre-release; the release notes checker requires the decision to be stated. |
| Code of conduct | A short project-specific `CODE_OF_CONDUCT.md` is present. | Public issues/PRs need moderation expectations. | Use the short policy for alpha; switch to a standard covenant later if the community grows. |
| Public CI evidence | Normal CI and manual Release Readiness workflows exist; `docs/release/PUBLIC_CI_EVIDENCE.md` is a pending evidence record. | "CI was green" is meaningless unless tied to an exact commit and run; after public release, users will judge the project by that evidence. | Require GitHub Actions green on the exact public commit for both normal CI and Release Readiness, then run `scripts/check-ci-evidence-decision.sh` so the evidence record matches the owner decision. |
| Dependency review evidence | `docs/release/DEPENDENCY_REVIEW.md` defines manual cargo/npm/workflow review commands; `docs/release/DEPENDENCY_REVIEW_EVIDENCE.md` is a pending exact-commit evidence record; `.github/dependabot.yml` covers version-update surfaces, but no automated license/advisory allowlist exists yet. | First public users inherit the dependency graph and any advisories/license surprises. | Run and record the dependency review for the exact public commit, or explicitly accept skipping it in the owner record; after choosing, run `scripts/check-dependency-review-decision.sh`. |

## Should decide before packaging binaries

| Decision | Current state | Why it matters | Recommended alpha default |
|---|---|---|---|
| macOS signing/notarization | Local app bundle only. | Unsigned binaries are difficult for non-developers to run. | Do not ship binaries until Apple Developer ID signing and notarization are automated. |
| Update channel | None. | Auto-update is a security boundary and support commitment. | No auto-update in alpha. Use GitHub releases only. |
| Dependency/license review cadence | Manual dependency review is required for source alpha; no automated cargo/npm license allowlist is enforced yet. | Automation prevents drift after the first public release. | Add an automated allowlist/advisory policy after alpha if the project keeps taking outside users. |

## Explicit "do not forget" calls

- Public alpha is an external promise. If the README says a feature works, it
  should either have a quickstart path or be labeled experimental.
- Do not publish a repo with raw local path artifacts unless you are comfortable
  with that data being indexed forever.
- Do not accept outside contributions before the license/contributor policy is
  settled.
- Do not ship a macOS binary until you decide whether "unsigned alpha" is
  acceptable for your audience.
