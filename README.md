<p align="center">
  <img src="crates/fleet-host/icons/icon.png" alt="VS Fleet logo" width="512" height="512">
</p>

<h1 align="center">VS Fleet</h1>

<p align="center">
  A local-first control surface for terminal-based AI coding sessions in VS Code web.
</p>

<p align="center">
  <a href="docs/release/PUBLIC_ALPHA_READINESS_ASSESSMENT.md"><img alt="Status: source alpha" src="https://img.shields.io/badge/status-source--alpha-orange"></a>
  <img alt="Platform: macOS" src="https://img.shields.io/badge/platform-macOS-lightgrey">
  <img alt="Mode: local first" src="https://img.shields.io/badge/mode-local--first-blue">
  <img alt="Architecture: app plus extension" src="https://img.shields.io/badge/architecture-Fleet.app%20%2B%20VS%20Code%20bridge-6f42c1">
  <a href="LICENSE"><img alt="License: MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-green"></a>
</p>

## Overview

VS Fleet is a compact macOS app for supervising local `code serve-web` sessions
used by terminal-based AI coding agents. It collects editor and agent state,
shows each session in a small Fleet window, and lets you switch between sessions
without Fleet owning the agent process or capturing the user's keystrokes.

This repository contains two runtime pieces that work together:

| Piece | Path | Role |
|---|---|---|
| Fleet app / host | `crates/fleet-host` | The macOS Tauri app, sidebar UI, embedded session webviews, local bridge, and convenience session launcher. |
| Fleet Bridge extension | `packages/fleet-bridge` | A VS Code extension packaged as a VSIX and installed into each spawned `code serve-web` profile so the editor can register with Fleet and report state. |

The app bundle build assembles both pieces. `crates/fleet-host/bundle.sh` builds
the Rust host, builds the Fleet Bridge VSIX, copies both into `Fleet.app`, and
the host installs the bridge extension into spawned local VS Code web sessions.

## Quickstart

Source-alpha macOS build:

```sh
cd crates/fleet-host
./bundle.sh debug
open Fleet.app
```

From Fleet, use the plus menu to start a local session in your home folder or
open another local folder. Remote and container launch paths are not the
supported alpha path.

## Project Status

VS Fleet is in source-alpha release preparation. The codebase is suitable for
private dogfooding and technical review; public visibility is gated by the
owner decision record, history/artifact cleanup, security evidence, and support
evidence tracked in:

- [Public alpha decisions](docs/release/PUBLIC_ALPHA_DECISIONS.md)
- [Public alpha readiness assessment](docs/release/PUBLIC_ALPHA_READINESS_ASSESSMENT.md)
- [Alpha release checklist](docs/release/ALPHA_RELEASE_CHECKLIST.md)
- [Quickstart](docs/QUICKSTART.md)
- [Architecture overview](docs/ARCHITECTURE.md)
- [Local data and uninstall](docs/LOCAL_DATA_AND_UNINSTALL.md)
- [Release process](docs/release/RELEASE_PROCESS.md)

The long-form product and architecture spec lives in
[docs/ENGINEERING_SPEC.md](docs/ENGINEERING_SPEC.md).

## What Works Today

- A Rust Hub, protocol crate, reporter, CLI, and host-core model.
- A macOS Tauri Fleet host that embeds local `code serve-web` sessions.
- A Fleet Bridge VS Code extension used by the host to register editor sessions
  and route commands.
- Local session spawning from the host as a convenience function.
- Session rename, mute/solo/dismiss, unread/waiting state, and host logs.
- Automated Rust tests and host-level visual probe infrastructure.

## Not Release-Ready

- Public binary distribution: no signing/notarization policy yet.
- Remote/container deployment: design and eval harness exist, but this is not a
  supported alpha path yet.
- External contributions: DCO sign-off is required; broad outside code PRs wait
  until the owner opens contribution intake.
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

`Fleet` is a provisional source-alpha working name. This repository makes no trademark claim to the name, and stable package or binary publication under Fleet namespaces is deferred until the owner completes the public name decision.

## Security and privacy

Fleet is local-first and has no intended telemetry by default. It can still log
local metadata such as workspace paths, local URLs, session labels, process command lines, and editor state. Scrub logs and review artifacts before sharing them publicly.

Local source-alpha runtime data lives under `~/.fleet/run` and `~/.fleet/mux`
unless `FLEET_RUNTIME_DIR` or `FLEET_MUX_DIR` is set. Manual cleanup is
documented in [Local data and uninstall](docs/LOCAL_DATA_AND_UNINSTALL.md).

## Editor Server Boundary

The source alpha uses the user's local `code serve-web` install. Fleet does not
download, bundle, host, or redistribute Microsoft's VS Code Server, Microsoft
Marketplace extensions, or Microsoft remote extensions.

See [SECURITY.md](SECURITY.md) for the current alpha security policy.
See [SUPPORT.md](SUPPORT.md) for the current alpha support boundary.

## License

Fleet is licensed under `MIT OR Apache-2.0`. See [LICENSE](LICENSE) for the
project license notice and links to the full MIT and Apache-2.0 texts.
