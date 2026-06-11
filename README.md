> This is a vibe coded app I put together because I had personal use for it, that will be updated when I personally require new features. I'm providing it as is to see if there is more general interest in vs-code multiplexing. If this project gets any more general interest I'll take it more seriously. PRs and issues welcome. As you have a right to know what you are reading. All AI text on this repo will be color marked.

<p align="center">
  <img src="crates/fleet-host/icons/icon.png" alt="VS Fleet logo" width="512" height="512">
</p>

<h1 align="center">VS Fleet</h1>

<p align="center">
  🟪 A macOS app for managing local VS Code web sessions used by terminal-based AI coding agents.
</p>

<p align="center">
  <a href="docs/release/PUBLIC_ALPHA_READINESS_ASSESSMENT.md"><img alt="Status" src="https://img.shields.io/badge/status-pre--alpha%20prototype-orange"></a>
  <img alt="Platform" src="https://img.shields.io/badge/platform-macOS-lightgrey">
  <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-green"></a>
</p>

🟪 AI-authored text is marked with this purple square.

## 🟪 What is VS Fleet?

🟪 VS Fleet is a compact macOS app for supervising local `code serve-web` sessions
used by terminal-based AI coding agents. It collects editor and agent state,
shows each session in a small Fleet window, and switches between sessions
without owning agent processes or capturing keystrokes.

🟪 The alpha is intentionally local-only: it uses the installed VS Code CLI and
starts local web sessions from the Fleet app.

## 🟪 Features

- 🟪 Launch local VS Code web sessions from the Fleet window.
- 🟪 Open a new session in the home directory or in a selected local folder.
- 🟪 View, rename, mute, dismiss, and switch between sessions.
- 🟪 Track unread and waiting state from terminal-based coding agents.
- 🟪 Keep Fleet as a client: editor sessions and reporters connect back to Fleet.

## 🟪 Requirements

- 🟪 macOS.
- 🟪 A local VS Code installation with the `code` command available.
- 🟪 Rust and Node.js tooling for building from source.

## 🟪 Getting Started

🟪 Alpha macOS build:

```sh
cd crates/fleet-host
./bundle.sh debug
open Fleet.app
```

🟪 From Fleet, use the plus menu to start a local session in your home folder or
open another local folder. Remote and container launch paths are not the
supported alpha path.

## 🟪 Components

| Component | Path | Description |
|---|---|---|
| 🟪 Fleet host | `crates/fleet-host` | 🟪 Tauri macOS app, session list, embedded VS Code webviews, local bridge, and local session launcher. |
| 🟪 Fleet Bridge | `packages/fleet-bridge` | 🟪 VS Code extension packaged into the app and installed into spawned `code serve-web` profiles. |
| 🟪 Fleet reporter | `crates/fleet-reporter` | 🟪 Reports editor/session/agent state back to Fleet. |
| 🟪 Fleet hub | `crates/fleet-hub` | 🟪 Local state projection used by the host, reporter, and CLI. |

🟪 `crates/fleet-host/bundle.sh` builds the Rust host, builds the Fleet Bridge
VSIX, copies both into `Fleet.app`, and includes the reporter binary used by
spawned sessions.

## 🟪 Status

🟪 VS Fleet is an AI-assisted pre-alpha concept prototype. It exists to test
whether there is broader interest in VS Code multiplexing before the project
receives sustained product, support, and release engineering effort.

🟪 Current support boundary:

- 🟪 Local macOS app.
- 🟪 Local `code serve-web` sessions launched from the local VS Code install.
- 🟪 Source builds only; no signed or notarized binary distribution yet.

## 🟪 Documentation

- 🟪 [Public alpha decisions](docs/release/PUBLIC_ALPHA_DECISIONS.md)
- 🟪 [Public alpha readiness assessment](docs/release/PUBLIC_ALPHA_READINESS_ASSESSMENT.md)
- 🟪 [Alpha release checklist](docs/release/ALPHA_RELEASE_CHECKLIST.md)
- 🟪 [Quickstart](docs/QUICKSTART.md)
- 🟪 [Architecture overview](docs/ARCHITECTURE.md)
- 🟪 [Local data and uninstall](docs/LOCAL_DATA_AND_UNINSTALL.md)
- 🟪 [Release process](docs/release/RELEASE_PROCESS.md)

🟪 The long-form product and architecture spec lives in
[docs/ENGINEERING_SPEC.md](docs/ENGINEERING_SPEC.md).

## 🟪 Limitations

- 🟪 Public binary distribution: no signing/notarization policy yet.
- 🟪 Remote/container deployment: design and eval harness exist, but this is not a
  supported alpha path yet.
- 🟪 External contributions: DCO sign-off is required; broad outside code PRs wait
  until the owner opens contribution intake.
- 🟪 Tracked visual/eval artifacts: useful for development, but they need pruning
  or redaction before public GitHub visibility.

## 🟪 Project Structure

| Path | Purpose |
|---|---|
| `crates/fleet-protocol` | 🟪 JSON-serializable protocol types. |
| `crates/fleet-hub` | 🟪 Local Hub process and state projection. |
| `crates/fleet-reporter` | 🟪 Reporter adapters and reporter binary. |
| `crates/fleet-cli` | 🟪 CLI face, currently `fleet ls` and related commands. |
| `crates/fleet-host-core` | 🟪 Pure Rust inbox/view-model logic. |
| `crates/fleet-host` | 🟪 Standalone Tauri macOS host app. |
| `packages/fleet-bridge` | 🟪 VS Code bridge extension packaged into the host app. |
| `packages/extension` | 🟪 VS Code extension face/prototype. |
| `containers/fleet-env` | 🟪 Container/eval harness material. |
| `docs` | 🟪 Engineering spec and release-readiness docs. |

## 🟪 Development

🟪 Core workspace:

```sh
cargo fmt --all -- --check
cargo test --workspace --all-targets --all-features
```

🟪 Fleet host:

```sh
cd crates/fleet-host
cargo test
./bundle.sh release
```

🟪 Release hygiene gate:

```sh
./scripts/release-check.sh
```

🟪 The release check is expected to fail until public alpha blockers are resolved.
See [docs/release/RELEASE_PROCESS.md](docs/release/RELEASE_PROCESS.md) for the
alpha release process.

## 🟪 Roadmap

🟪 No public roadmap commitments are made during alpha. Public issues, labels, and
milestones are triage hints only, not delivery promises, unless a later owner
decision publishes a concrete roadmap.

🟪 `Fleet` is a provisional alpha working name. This repository makes no
trademark claim to the name, and stable package or binary publication under
Fleet namespaces is deferred until the owner completes the public name decision.

## 🟪 Security and Privacy

🟪 Fleet is local-first and has no intended telemetry by default. It can still log
local metadata such as workspace paths, local URLs, session labels, process
command lines, and editor state. Scrub logs and review artifacts before sharing
them publicly.

🟪 Local alpha runtime data lives under `~/.fleet/run` and `~/.fleet/mux`
unless `FLEET_RUNTIME_DIR` or `FLEET_MUX_DIR` is set. Manual cleanup is
documented in [Local data and uninstall](docs/LOCAL_DATA_AND_UNINSTALL.md).

## 🟪 Legal

🟪 The alpha uses the local `code serve-web` install. Fleet does not download,
bundle, host, or redistribute Microsoft's VS Code Server, Microsoft
Marketplace extensions, or Microsoft remote extensions.

🟪 See [SECURITY.md](SECURITY.md) for the current alpha security policy.
🟪 See [SUPPORT.md](SUPPORT.md) for the current alpha support boundary.

## 🟪 License

🟪 Fleet is licensed under the MIT License. See [LICENSE](LICENSE).
