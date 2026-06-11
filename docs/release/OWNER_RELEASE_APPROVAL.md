# Owner Release Approval Sheet

Approval status: PENDING

This sheet is the short-form owner review before public GitHub visibility. It
does not replace `OWNER_DECISION_RECORD.md`; copy final choices there and mark
that file `APPROVED` only after every item below has been reviewed.

## Honest Readiness Judgment

Fleet is still too rough for a broad open-source launch, package announcement,
binary distribution, or "stable project" presentation.

Fleet is reasonable for a narrow source-only alpha if, and only if, the owner
accepts the constraints below:

- Source-only release: no app bundle, package registry publishing, marketplace
  publishing, container image, auto-update, or binary support promise.
- macOS-only source alpha: local Fleet host, local user-provided
  `code serve-web`, Fleet bridge, Fleet reporter, CLI, and local Hub.
- Clean public history: publish a one-commit public branch made with
  `scripts/prepare-public-branch.sh`, not the current private branch history.
- Provisional name: `Fleet` remains a working name with no trademark claim and
  no stable package/binary namespace promise.
- Best-effort support only: breaking changes are expected; no production SLA,
  stable API, stable protocol, stable state format, or upgrade promise.
- Local privacy boundary: no telemetry by default, but logs and artifacts can
  include workspace paths, local URLs, session labels, command lines, and editor
  state.

## Owner Decisions To Approve Or Change

Use these as the recommended source-alpha choices:

| Decision | Recommended owner answer |
|---|---|
| License | `MIT OR Apache-2.0`; keep reusable library/API crates permissive. |
| Public history | Cleaned/squashed first public branch. |
| Namespace | Fill exact GitHub owner/repo and keep package/binary namespaces provisional. |
| Alpha scope | Local macOS host plus local `code serve-web` workflow only. |
| Editor server boundary | User-provided VS Code only; no Microsoft server or Marketplace redistribution. |
| Distribution | Source-only alpha. |
| Security reporting | Enable GitHub Private Vulnerability Reporting, or add a private security contact. |
| Contributions | Require DCO sign-off; no CLA for source alpha. |
| CI evidence | GitHub Actions green on the exact public branch/commit. |
| Privacy | No telemetry by default; document local log contents and scrub-before-sharing. |
| Dependency review | Run dependency review for the exact public commit and record findings. |
| Support | Best-effort alpha support only. |
| Branding | `Fleet` name and current icon are alpha placeholders. |
| Versioning | Alpha pre-release tags only; no stable compatibility promise. |
| Community intake | Scoped bug reports and alpha feedback only; blank issues and Discussions off. |
| Release custody | Single-maintainer alpha; no package publishing credentials. |
| AI contributions | Allow only human-reviewed, rights-certified AI-assisted contributions. |
| Platform | macOS source alpha only with Rust 1.78+, Node.js 20/npm, Git, and VS Code `code` CLI. |
| Roadmap | No public roadmap commitments; issues/labels/milestones are triage hints. |
| Name collision | Treat `Fleet` as provisional and make no trademark claim. |
| Local data | Document `~/.fleet/run`, `~/.fleet/mux`, manual cleanup, and process ownership. |
| Workflow supply chain | Tagged Actions accepted only with read-only token, no secrets, and no publishing credentials. |

## Evidence Required Before Approval

- `OWNER_DECISION_RECORD.md` is `APPROVED` and has exactly one checked answer for
  every required section.
- `PUBLIC_BRANCH_EVIDENCE.md` is `PASS` if cleaned history is selected.
- `PUBLIC_CI_EVIDENCE.md` is `PASS` for the exact public commit.
- `GITHUB_PUBLICATION_EVIDENCE.md` is `PASS` for the exact repository settings.
- `DEPENDENCY_REVIEW_EVIDENCE.md` records dependency review for the exact public
  commit, unless the owner explicitly accepts skipping it.
- `scripts/check-public-release-branch.sh <public-branch> <source-ref-sha>`
  passes if cleaned history is selected. If the owner explicitly accepts current
  branch history exposure instead, `scripts/release-check.sh` passes on the
  current branch.

## Mechanical Approval Path

1. Generate a full recommended draft:

   ```sh
   ./scripts/draft-owner-decisions.sh <github-owner> <github-repo> docs/release/OWNER_DECISION_RECORD.draft.md
   ```

2. If accepting the recommended source-only alpha posture, fill
   [OWNER_DECISION_REPLY_TEMPLATE.md](OWNER_DECISION_REPLY_TEMPLATE.md) first so
   the remaining namespace, security, emergency-removal, and CI evidence values
   are explicit.
3. Review every checked choice in the draft against this sheet.
4. Copy approved choices into `docs/release/OWNER_DECISION_RECORD.md`.
5. Fill namespace, public branch, CI, GitHub publication, and dependency review
   evidence. Use the evidence generators instead of hand-editing reviewed
   evidence:
   `./scripts/generate-public-branch-evidence.sh`,
   `./scripts/generate-public-ci-evidence.sh`, and
   `./scripts/generate-github-publication-evidence.sh`.
6. Change `OWNER_DECISION_RECORD.md` to `Decision record status: APPROVED`.
7. Run the release gate for the public branch:

   ```sh
   ./scripts/release-evidence-status.sh
   ./scripts/check-public-release-branch.sh <public-branch> <source-ref-sha>
   ./scripts/generate-alpha-release-notes.sh v0.1.0-alpha.1 <source-ref-sha> path/to/release-notes.md
   ```

Do not publish publicly while this sheet or any evidence file remains pending.
