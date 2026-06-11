# Public Alpha Owner Prompt

Use this before marking
[`OWNER_DECISION_RECORD.md`](OWNER_DECISION_RECORD.md) as `APPROVED`.

The release gates intentionally do not infer these choices. Public GitHub
visibility should wait until the owner has made each decision explicitly.

Run this at any point to see the current unresolved owner choices and the
mechanical follow-up commands implied by the selected history/distribution
answers:

```sh
./scripts/public-alpha-decision-packet.sh
```

To create a review draft with the recommended source-alpha defaults checked,
run:

```sh
./scripts/draft-owner-decisions.sh <github-owner> <github-repo> docs/release/OWNER_DECISION_RECORD.draft.md
```

The draft stays `PENDING`; copy or edit it into `OWNER_DECISION_RECORD.md` only
after reviewing every checked choice.

After approving a license decision and preparing the actual legal license text,
apply the metadata with:

```sh
./scripts/apply-license-decision.sh docs/release/OWNER_DECISION_RECORD.md . path/to/LICENSE
```

After approving a namespace decision, apply the manifest metadata with:

```sh
./scripts/apply-namespace-decision.sh docs/release/OWNER_DECISION_RECORD.md .
```

This updates product, bundle, extension publisher, extension package, and
lockfile metadata. It verifies the Rust crate prefix but does not rename crates
automatically.

## Required Answers

Copy the answers into `OWNER_DECISION_RECORD.md`; do not publish while any value
is still `TODO`, ambiguous, or only a recommendation.

1. License:
   Recommended alpha default: `MIT OR Apache-2.0`.
   Keep reusable library/API crates permissive. Reserve AGPL-3.0-only plus a
   commercial exception as a future CLI/hosted-control-plane contingency only
   after a concrete monetization trigger.
   Owner answer:

2. First public history:
   Recommended alpha default: cleaned/squashed first public branch.
   Owner answer:

3. Public namespace:
   GitHub org/user:
   GitHub repo name:
   Product name:
   Rust crate prefix:
   npm package names:
   VS Code Marketplace publisher:
   Open VSX publisher:
   macOS bundle id:

4. Supported alpha scope:
   Recommended alpha default: local macOS Fleet host plus local
   `code serve-web` sessions, Fleet bridge, Fleet reporter, CLI, and embedded
   local Hub. Remote, SSH, Docker/container, visual probe, and eval harness
   paths remain development infrastructure.
   Owner answer:

5. Editor server licensing boundary:
   Recommended alpha default: user-provided VS Code only. Fleet may launch the
   user's local `code serve-web`; Fleet does not download, bundle, host, or
   redistribute Microsoft's VS Code Server, Marketplace extensions, or remote
   extensions.
   Owner answer:

6. Distribution:
   Recommended alpha default: source-only alpha. No public app bundle, crates.io,
   npm, Open VSX, VS Code Marketplace, or container image publishing.
   Owner answer:

7. Security reporting:
   Recommended alpha default: enable GitHub Private Vulnerability Reporting.
   Owner answer:

8. Contribution intake:
   Recommended alpha default: require DCO sign-off for outside code
   contributions; no CLA for source alpha. Revisit CLA before accepting code if
   commercial exceptions or proprietary relicensing become a goal.
   Owner answer:

9. Public CI evidence:
   Recommended alpha default: require GitHub Actions green on the exact public
   branch/commit before public visibility.
   Owner answer:
   Follow-up: after GitHub Actions runs on the exact public commit, generate
   `PUBLIC_CI_EVIDENCE.md` and run
   `./scripts/check-ci-evidence-decision.sh`.

10. Privacy and telemetry posture:
    Recommended alpha default: no telemetry by default. Local logs and artifacts
    may contain workspace paths, local URLs, session labels, process command
    lines, and editor state; users must scrub them before sharing.
    Owner answer:

11. Dependency review evidence:
    Recommended alpha default: run the commands in
    [`DEPENDENCY_REVIEW.md`](DEPENDENCY_REVIEW.md) and record findings before
    public visibility.
    Owner answer:

12. Support commitment:
    Recommended alpha default: best-effort alpha support only. Breaking changes
    are expected; no production support guarantees, response SLAs, paid support
    terms, or stable release lines.
    Owner answer:

13. Branding stability:
    Recommended alpha default: decide whether `Fleet` is stable now; treat the
    icon as an alpha asset unless you want to freeze it publicly.
    Owner answer:
    Follow-up: run `./scripts/check-branding-decision.sh` after copying this
    choice into `OWNER_DECISION_RECORD.md`.

14. Versioning and compatibility:
    Recommended alpha default: alpha pre-release tags only; no stable API,
    protocol, state-file, or upgrade compatibility promise during alpha.
    Owner answer:
    Follow-up: run `./scripts/check-versioning-decision.sh` after copying this
    choice into `OWNER_DECISION_RECORD.md`.

15. Community intake and moderation:
    Recommended alpha default: open only scoped public bug reports and alpha
    feedback, keep blank issues disabled, and keep Discussions off unless
    explicitly enabled later.
    Owner answer:
    Follow-up: run `./scripts/check-community-intake-decision.sh` after copying
    this choice into `OWNER_DECISION_RECORD.md`.

16. Release custody and maintainer authority:
    Recommended alpha default: single-maintainer alpha. Only the repository
    owner or named maintainer may push release tags, create GitHub releases,
    change repository settings, or publish packages. No package publishing
    credentials for source-only alpha.
    Owner answer:
   Follow-up: generate `GITHUB_PUBLICATION_EVIDENCE.md` with the exact
   repository settings and emergency removal owner, then run
   `./scripts/check-release-custody-decision.sh`.

17. AI-assisted contribution provenance:
    Recommended alpha default: allow AI-assisted contributions only when the
    contributor certifies human review, right to submit, and no private prompts,
    private model transcripts, local logs, workspace paths, or generated
    artifacts.
    Owner answer:
    Follow-up: run `./scripts/check-ai-contribution-decision.sh` after copying
    this choice into `OWNER_DECISION_RECORD.md`.

18. Supported platform and toolchain:
    Recommended alpha default: macOS source alpha only, with Rust 1.78 or newer,
    Node.js 20/npm, Git, and user-provided VS Code `code` CLI/serve-web.
    Owner answer:
    Follow-up: run `./scripts/check-platform-support-decision.sh` after copying
    this choice into `OWNER_DECISION_RECORD.md`.

19. Public roadmap and non-goals:
    Recommended alpha default: no public roadmap commitments during alpha;
    issues, labels, milestones, and feedback are triage signals only, not
    delivery promises.
    Owner answer:
    Follow-up: run `./scripts/check-roadmap-decision.sh` after copying this
    choice into `OWNER_DECISION_RECORD.md`.

20. Public name collision and trademark posture:
    Recommended alpha default: use `Fleet` only as a provisional source-alpha
    working name, make no trademark claim, acknowledge collision review is not
    clearance, and do not publish packages or binaries under stable Fleet
    namespaces.
    Owner answer:
    Follow-up: run `./scripts/check-name-collision-decision.sh` after copying
    this choice into `OWNER_DECISION_RECORD.md`.

21. Local data and uninstall policy:
    Recommended alpha default: document the source-alpha local data locations
    and manual cleanup commands; do not imply that quitting Fleet removes
    spawned editor data, logs, or external sessions.
    Owner answer:
    Follow-up: run `./scripts/check-local-data-decision.sh` after copying this
    choice into `OWNER_DECISION_RECORD.md`.

22. GitHub Actions supply-chain posture:
    Recommended alpha default: accept tagged third-party GitHub Actions for the
    source alpha only with read-only workflow token permissions, no repository
    secrets, and no publishing credentials.
    Owner answer:
    Follow-up: run `./scripts/check-workflow-supply-chain-decision.sh` after
    copying this choice into `OWNER_DECISION_RECORD.md`.

## Binary-Only Decisions

These are not required for a source-only alpha. They become required before any
public app bundle.

23. macOS signing and notarization:
    Recommended default: no public binaries until Developer ID signing and
    notarization are automated.
    Owner answer:

24. Update channel:
    Recommended default: no auto-update in alpha.
    Owner answer:

## Approval Rule

Only after the answers above are copied into `OWNER_DECISION_RECORD.md`, update:

```text
Decision record status: APPROVED
```

Then run:

```sh
./scripts/release-evidence-status.sh
./scripts/check-public-release-branch.sh public-alpha "$(git rev-parse HEAD)"
```
