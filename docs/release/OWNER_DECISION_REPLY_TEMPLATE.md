# Owner Decision Reply Template

Use this when the owner accepts the recommended source-only alpha defaults and
needs to provide only the remaining concrete values. This is not an approval record by itself.
Copy the final decisions into `OWNER_DECISION_RECORD.md` and leave that record
`PENDING` until every release evidence file passes.

```text
I accept the recommended source-only alpha defaults in docs/release/OWNER_RELEASE_APPROVAL.md.
This includes cleaned first public history, local macOS-only scope, user-provided VS Code,
source-only distribution, DCO/no CLA for alpha, best-effort support,
no stable compatibility promise, no public roadmap commitment,
provisional Fleet name/no trademark claim, documented local data,
no telemetry by default, and read-only/no-secret workflows.

Namespace values:
  GitHub org/user: <owner>
  GitHub repo name: <repo>
  Product name: Fleet | <new name>
  Rust crate prefix: fleet-* | <new prefix>
  npm package names: fleet-extension, fleet-bridge | <new names>
  VS Code Marketplace publisher: fleet-team | <publisher>
  Open VSX publisher: fleet-team | <publisher>
  macOS bundle id: dev.fleet.host | <new bundle id>

Security reporting: GitHub Private Vulnerability Reporting | <private security contact>
Emergency removal owner for publication evidence: <owner/contact>
CI evidence: provide the CI and Release Readiness run URLs after the public branch exists.
```

Namespace answers must be concrete because manifest metadata needs exact
strings. Source-only alpha still defers crates.io, npm, VS Code Marketplace,
Open VSX, binary, and container publication unless a later owner decision
changes the distribution scope.

After the owner replies, generate a checked draft:

```sh
./scripts/draft-owner-decisions.sh <github-owner> <github-repo> docs/release/OWNER_DECISION_RECORD.draft.md
```

Then edit any non-default answers, copy the result into
`docs/release/OWNER_DECISION_RECORD.md`, and keep the status `PENDING` until the evidence files are concrete.
