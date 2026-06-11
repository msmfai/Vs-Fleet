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

Outside code contributions require Developer Certificate of Origin (DCO)
sign-off. Add a `Signed-off-by` line to every commit:

```text
Signed-off-by: Your Name <your.email@example.com>
```

Use `git commit -s` to add it automatically.

- Contributors certify that they have the right to submit their work.
- Contributions are licensed under the same license as the project.
- No Contributor License Agreement (no CLA) is required for source alpha.

A DCO does not assign copyright and does not preserve commercial exception or
future proprietary relicensing rights over contributor-owned code. The project
must revisit the CLA decision before accepting outside code if that becomes a
goal.

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
