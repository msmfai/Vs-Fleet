# Contributing

Fleet is not ready for broad external contributions until the project license
has been chosen and applied. Small issue reports and private technical feedback
are welcome during alpha, but code contributions should wait unless the
maintainer explicitly asks for them.

## Before opening a pull request

- Check `docs/release/PUBLIC_ALPHA_DECISIONS.md` for unresolved release
  decisions.
- Keep changes focused and tested.
- Do not include generated build outputs, local logs, screenshots with private
  data, credentials, or machine-specific paths.
- Do not add dependencies without explaining why they are needed.

## AI-assisted changes

AI-assisted contributions are allowed only when the contributor has reviewed and
understands the change, has the right to submit it under the project license,
and does not include private prompts, private model transcripts, private logs,
workspace paths, generated build outputs, raw logs, or machine-specific paths.
The contributor is responsible for the submitted code, tests, and provenance.

## Contribution licensing

This section must be finalized before accepting outside code contributions.
Until then, contributors should assume that unsolicited code PRs may be deferred
or closed.

Recommended policy once the license is chosen:

- Contributors certify that they have the right to submit their work.
- Contributions are licensed under the same license as the project.
- A Developer Certificate of Origin sign-off is sufficient; no CLA by default.

## Local checks

Common checks used in this repository:

```sh
cargo fmt --all -- --check
cargo test --workspace --all-targets --all-features
node --check crates/fleet-host/ui/main.js
./scripts/release-check.sh
```

The release check is expected to fail until public-alpha blockers such as the
license and artifact policy are resolved.
