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

## Required Answers

Copy the answers into `OWNER_DECISION_RECORD.md`; do not publish while any value
is still `TODO`, ambiguous, or only a recommendation.

1. License:
   Recommended alpha default: `MIT OR Apache-2.0`.
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
   Recommended alpha default: accept small focused PRs under the chosen project
   license using the PR template certification; no CLA for alpha.
   Owner answer:

9. Public CI evidence:
   Recommended alpha default: require GitHub Actions green on the exact public
   branch/commit before public visibility.
   Owner answer:

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

## Binary-Only Decisions

These are not required for a source-only alpha. They become required before any
public app bundle.

14. macOS signing and notarization:
    Recommended default: no public binaries until Developer ID signing and
    notarization are automated.
    Owner answer:

15. Update channel:
    Recommended default: no auto-update in alpha.
    Owner answer:

## Approval Rule

Only after the answers above are copied into `OWNER_DECISION_RECORD.md`, update:

```text
Decision record status: APPROVED
```

Then run:

```sh
./scripts/release-check.sh
```
