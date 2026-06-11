# Fleet

Fleet is a local-first control surface for terminal-based AI coding sessions.
It collects the state of local VS Code web sessions, shows them in a compact
Tauri host window, and lets you switch between sessions without Fleet owning the
agent process or the user's keystrokes.

## Alpha status

Fleet is **not ready for a general public alpha yet**. The codebase is suitable
for private dogfooding and technical review, but public release is blocked on
license, artifact, security, and support decisions tracked in:

- [Public alpha decisions](docs/release/PUBLIC_ALPHA_DECISIONS.md)
- [Alpha release checklist](docs/release/ALPHA_RELEASE_CHECKLIST.md)
- [Quickstart](docs/QUICKSTART.md)
- [Architecture overview](docs/ARCHITECTURE.md)
- [Release process](docs/release/RELEASE_PROCESS.md)

The long-form product and architecture spec lives in
[docs/ENGINEERING_SPEC.md](docs/ENGINEERING_SPEC.md).

## What works today

- A Rust Hub, protocol crate, reporter, CLI, and host-core model.
- A macOS Tauri Fleet host that embeds local `code serve-web` sessions.
- A Fleet bridge VS Code extension used by the host to register editor sessions
  and route commands.
- Local session spawning from the host as a convenience function.
- Session rename, mute/solo/dismiss, unread/waiting state, and host logs.
- Automated Rust tests and host-level visual probe infrastructure.

## What is experimental or not release-ready

- Public binary distribution: no signing/notarization policy yet.
- Remote/container deployment: design and eval harness exist, but this is not a
  supported alpha path yet.
- External contributions: deferred until the license and contribution policy are
  settled.
- Tracked visual/eval artifacts: useful for development, but they need pruning
  or redaction before public GitHub visibility.

## Repository layout

| Path | Purpose |
|---|---|
| `crates/fleet-protocol` | JSON-serializable protocol types. |
| `crates/fleet-hub` | Local Hub process and state projection. |
| `crates/fleet-reporter` | Reporter adapters and reporter binary. |
| `crates/fleet-cli` | CLI face, currently `fleet ls` and related commands. |
| `crates/fleet-host-core` | Pure Rust inbox/view-model logic. |
| `crates/fleet-host` | Standalone Tauri macOS host app. |
| `packages/fleet-bridge` | VS Code bridge extension packaged into the host app. |
| `packages/extension` | VS Code extension face/prototype. |
| `containers/fleet-env` | Container/eval harness material. |
| `docs` | Engineering spec and release-readiness docs. |

## Build and test

Core workspace:

```sh
cargo fmt --all -- --check
cargo test --workspace --all-targets --all-features
```

Fleet host:

```sh
cd crates/fleet-host
cargo test
./bundle.sh release
```

Release hygiene gate:

```sh
./scripts/release-check.sh
```

The release check is expected to fail until public-alpha blockers are resolved.
See [docs/release/RELEASE_PROCESS.md](docs/release/RELEASE_PROCESS.md) for the
source-alpha release process.

No public roadmap commitments are made during alpha. Public issues, labels, and
milestones are triage hints only, not delivery promises, unless a later owner
decision publishes a concrete roadmap.

`Fleet` is a provisional source-alpha working name. This repository makes no
trademark claim to the name, and stable package or binary publication under
Fleet namespaces is deferred until the owner completes the public name decision.

## Security and privacy

Fleet is local-first and has no intended telemetry by default. It can still log
local metadata such as workspace paths, local URLs, session labels, process
command lines, and editor state. Scrub logs and review artifacts before sharing
them publicly.

## Editor Server Boundary

The source alpha uses the user's local `code serve-web` install. Fleet does not
download, bundle, host, or redistribute Microsoft's VS Code Server, Microsoft
Marketplace extensions, or Microsoft remote extensions.

See [SECURITY.md](SECURITY.md) for the current alpha security policy.
See [SUPPORT.md](SUPPORT.md) for the current alpha support boundary.

## License

No open-source license has been chosen yet. Do not publish this repository as an
open-source project until a license is selected, a root `LICENSE` file is added,
and package manifests are updated away from `UNLICENSED`.
