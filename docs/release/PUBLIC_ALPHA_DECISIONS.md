# Public Alpha Decisions

Fleet is not ready to publish until every **Required before public alpha** item
has an explicit owner decision. These are the choices that are expensive to
change once strangers clone, package, fork, or depend on the project.

## Required before public alpha

| Decision | Current state | Why it matters | Recommended alpha default |
|---|---|---|---|
| License | `UNLICENSED` in Rust crates and VS Code packages; no `LICENSE` file. | Without a license, outsiders have no clear right to use, fork, package, or contribute. Changing later can be messy if external contributors appear. | Pick either MIT/Apache-2.0 dual license for permissive Rust ecosystem norms, or AGPL-3.0-only if you want strong network-copyleft. Do not publish while `UNLICENSED`. |
| Release scope | Mixed source tree: protocol/hub/reporter/CLI, macOS Tauri host, VS Code extensions, container eval harness, large design spec. | Users need to know what is usable today vs research/eval scaffolding. | Alpha scope should be "local macOS Fleet host + local code serve-web sessions + reporter/bridge pieces"; mark remote/deploy/container eval as experimental or internal. |
| Distribution | Release bundle can be built locally; no signed/notarized macOS release policy. | Unsigned macOS apps trigger trust warnings. Public binary releases imply update/signing/support expectations. | Source-only alpha first, with build-from-source instructions. Add signed binaries later. |
| Artifact policy | Raw host/eval artifacts are ignored going forward, but prior commits may still contain them until history is cleaned. | Public repos should not leak local paths/process details or look like failing release evidence. | Before first GitHub publish, either squash/rewrite the branch so removed artifacts never appear in public history, or accept that old commits expose local paths. Keep only curated, redacted screenshots in public history. |
| Security disclosure | No `SECURITY.md`; local WebSocket/Unix socket surfaces and app bundle are security-relevant. | Users need to know how to report issues and what versions are supported. | Add `SECURITY.md` with supported alpha branch and GitHub Security Advisories/contact. |
| Privacy/logging posture | Host/reporter logs contain workspace paths, command lines, session labels, and local URLs. | This product observes developer environments; privacy expectations must be explicit. | State that Fleet is local-first, has no telemetry by default, and logs local metadata that users should scrub before sharing. |
| Contribution policy | No contribution guide or DCO/CLA decision. | First outside PR forces a licensing/provenance decision. | No CLA for alpha; require contributors to certify they can license work under the chosen project license, via DCO or simple PR statement. |
| Public name/namespace | Product and package names are `Fleet`, `fleet-*`, publisher `fleet-team`, bundle id `dev.fleet.host`. | Names collide easily and package namespaces can be hard to migrate. | Confirm GitHub org/repo, VS Code/Open-VSX publisher, crates package names, and macOS bundle identifier before publishing packages. |
| Support boundary | No public support/SLA statement. | Alpha users need expectations; otherwise every bug can become implied support. | State "best-effort alpha; breaking changes expected; no production support." |
| Code of conduct | None. | Public issues/PRs need moderation expectations. | Add Contributor Covenant or a short project-specific conduct policy before inviting contributions. |

## Should decide before packaging binaries

| Decision | Current state | Why it matters | Recommended alpha default |
|---|---|---|---|
| macOS signing/notarization | Local app bundle only. | Unsigned binaries are difficult for non-developers to run. | Do not ship binaries until Apple Developer ID signing and notarization are automated. |
| Update channel | None. | Auto-update is a security boundary and support commitment. | No auto-update in alpha. Use GitHub releases only. |
| Dependency/license review cadence | No automated cargo/npm license audit. | You will eventually need dependency license and vulnerability evidence. | Add a lightweight manual release checklist now; automate later. |
| Trademarks/branding | Generated icon and placeholder name. | Public assets become recognizable quickly. | Treat branding as alpha/temporary unless you explicitly want to keep it. |

## Explicit "do not forget" calls

- Public alpha is an external promise. If the README says a feature works, it
  should either have a quickstart path or be labeled experimental.
- Do not publish a repo with raw local path artifacts unless you are comfortable
  with that data being indexed forever.
- Do not accept outside contributions before the license/contributor policy is
  settled.
- Do not ship a macOS binary until you decide whether "unsigned alpha" is
  acceptable for your audience.
