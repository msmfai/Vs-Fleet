# Contributing

Fleet is an experimental project licensed under MIT. Small issue reports,
technical feedback, and focused patches are welcome. Large or speculative code
changes are reviewed conservatively while the project is small.

## Before opening a pull request

- Keep changes focused and tested.
- Do not include generated build outputs, local logs, private screenshots,
  credentials, or machine-specific paths.
- Do not add dependencies without explaining why they are needed.

## Contribution Licensing

Outside code contributions require Developer Certificate of Origin (DCO)
sign-off. Add a `Signed-off-by` line to every commit:

```text
Signed-off-by: Your Name <your.email@example.com>
```

Use `git commit -s` to add it automatically.

- Contributors certify that they have the right to submit their work.
- Contributions are licensed under the MIT License.
- No Contributor License Agreement is required.

## Local Checks

Common checks used in this repository:

```sh
cargo fmt --all -- --check
cargo test --workspace --all-targets --all-features
node --check crates/fleet-host/ui/main.js
```
