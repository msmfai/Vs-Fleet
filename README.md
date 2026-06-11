> This is a vibe coded app I put together because I had personal use for it, that will be updated when I personally require new features. I'm providing it as is to see if there is more general interest in vs-code multiplexing. If this project gets any more general interest I'll take it more seriously. PRs and issues welcome. As you have a right to know what you are reading. All AI text on this repo will be color marked.

<p align="center">
  <img src="crates/fleet-host/icons/icon.png" alt="VS Fleet logo" width="512" height="512">
</p>

<h1 align="center"><font color="#8250df">VS Fleet</font></h1>

<p align="center">
  <font color="#8250df">A macOS app for managing local VS Code web sessions used by terminal-based AI coding agents.</font>
</p>

<p align="center">
  <a href="docs/release/PUBLIC_ALPHA_READINESS_ASSESSMENT.md"><img alt="Status" src="https://img.shields.io/badge/status-alpha-orange"></a>
  <img alt="Platform" src="https://img.shields.io/badge/platform-macOS-lightgrey">
  <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-green"></a>
</p>

## <font color="#8250df">What is VS Fleet?</font>

<font color="#8250df">VS Fleet is a compact macOS app for supervising local `code serve-web` sessions
used by terminal-based AI coding agents. It collects editor and agent state,
shows each session in a small Fleet window, and switches between sessions
without owning agent processes or capturing keystrokes.</font>

<font color="#8250df">The alpha is intentionally local-only: it uses the installed VS Code CLI and
starts local web sessions from the Fleet app.</font>

## <font color="#8250df">Features</font>

- <font color="#8250df">Launch local VS Code web sessions from the Fleet window.</font>
- <font color="#8250df">Open a new session in the home directory or in a selected local folder.</font>
- <font color="#8250df">View, rename, mute, dismiss, and switch between sessions.</font>
- <font color="#8250df">Track unread and waiting state from terminal-based coding agents.</font>
- <font color="#8250df">Keep Fleet as a client: editor sessions and reporters connect back to Fleet.</font>

## <font color="#8250df">Requirements</font>

- <font color="#8250df">macOS.</font>
- <font color="#8250df">A local VS Code installation with the `code` command available.</font>
- <font color="#8250df">Rust and Node.js tooling for building from source.</font>

## <font color="#8250df">Getting Started</font>

<font color="#8250df">Alpha macOS build:</font>

```sh
cd crates/fleet-host
./bundle.sh debug
open Fleet.app
```

<font color="#8250df">From Fleet, use the plus menu to start a local session in your home folder or
open another local folder. Remote and container launch paths are not the
supported alpha path.</font>

## <font color="#8250df">Components</font>

| <font color="#8250df">Component</font> | <font color="#8250df">Path</font> | <font color="#8250df">Description</font> |
|---|---|---|
| <font color="#8250df">Fleet host</font> | `crates/fleet-host` | <font color="#8250df">Tauri macOS app, session list, embedded VS Code webviews, local bridge, and local session launcher.</font> |
| <font color="#8250df">Fleet Bridge</font> | `packages/fleet-bridge` | <font color="#8250df">VS Code extension packaged into the app and installed into spawned `code serve-web` profiles.</font> |
| <font color="#8250df">Fleet reporter</font> | `crates/fleet-reporter` | <font color="#8250df">Reports editor/session/agent state back to Fleet.</font> |
| <font color="#8250df">Fleet hub</font> | `crates/fleet-hub` | <font color="#8250df">Local state projection used by the host, reporter, and CLI.</font> |

<font color="#8250df">`crates/fleet-host/bundle.sh` builds the Rust host, builds the Fleet Bridge
VSIX, copies both into `Fleet.app`, and includes the reporter binary used by
spawned sessions.</font>

## <font color="#8250df">Status</font>

<font color="#8250df">VS Fleet is alpha software. It is suitable for private dogfooding and technical
review, but not for general users or production workflows.</font>

<font color="#8250df">Current support boundary:</font>

- <font color="#8250df">Local macOS app.</font>
- <font color="#8250df">Local `code serve-web` sessions launched from the local VS Code install.</font>
- <font color="#8250df">Source builds only; no signed or notarized binary distribution yet.</font>

## <font color="#8250df">Documentation</font>

- <a href="docs/release/PUBLIC_ALPHA_DECISIONS.md"><font color="#8250df">Public alpha decisions</font></a>
- <a href="docs/release/PUBLIC_ALPHA_READINESS_ASSESSMENT.md"><font color="#8250df">Public alpha readiness assessment</font></a>
- <a href="docs/release/ALPHA_RELEASE_CHECKLIST.md"><font color="#8250df">Alpha release checklist</font></a>
- <a href="docs/QUICKSTART.md"><font color="#8250df">Quickstart</font></a>
- <a href="docs/ARCHITECTURE.md"><font color="#8250df">Architecture overview</font></a>
- <a href="docs/LOCAL_DATA_AND_UNINSTALL.md"><font color="#8250df">Local data and uninstall</font></a>
- <a href="docs/release/RELEASE_PROCESS.md"><font color="#8250df">Release process</font></a>

<font color="#8250df">The long-form product and architecture spec lives in
<a href="docs/ENGINEERING_SPEC.md"><font color="#8250df">docs/ENGINEERING_SPEC.md</font></a>.</font>

## <font color="#8250df">Limitations</font>

- <font color="#8250df">Public binary distribution: no signing/notarization policy yet.</font>
- <font color="#8250df">Remote/container deployment: design and eval harness exist, but this is not a
  supported alpha path yet.</font>
- <font color="#8250df">External contributions: DCO sign-off is required; broad outside code PRs wait
  until the owner opens contribution intake.</font>
- <font color="#8250df">Tracked visual/eval artifacts: useful for development, but they need pruning
  or redaction before public GitHub visibility.</font>

## <font color="#8250df">Project Structure</font>

| <font color="#8250df">Path</font> | <font color="#8250df">Purpose</font> |
|---|---|
| `crates/fleet-protocol` | <font color="#8250df">JSON-serializable protocol types.</font> |
| `crates/fleet-hub` | <font color="#8250df">Local Hub process and state projection.</font> |
| `crates/fleet-reporter` | <font color="#8250df">Reporter adapters and reporter binary.</font> |
| `crates/fleet-cli` | <font color="#8250df">CLI face, currently `fleet ls` and related commands.</font> |
| `crates/fleet-host-core` | <font color="#8250df">Pure Rust inbox/view-model logic.</font> |
| `crates/fleet-host` | <font color="#8250df">Standalone Tauri macOS host app.</font> |
| `packages/fleet-bridge` | <font color="#8250df">VS Code bridge extension packaged into the host app.</font> |
| `packages/extension` | <font color="#8250df">VS Code extension face/prototype.</font> |
| `containers/fleet-env` | <font color="#8250df">Container/eval harness material.</font> |
| `docs` | <font color="#8250df">Engineering spec and release-readiness docs.</font> |

## <font color="#8250df">Development</font>

<font color="#8250df">Core workspace:</font>

```sh
cargo fmt --all -- --check
cargo test --workspace --all-targets --all-features
```

<font color="#8250df">Fleet host:</font>

```sh
cd crates/fleet-host
cargo test
./bundle.sh release
```

<font color="#8250df">Release hygiene gate:</font>

```sh
./scripts/release-check.sh
```

<font color="#8250df">The release check is expected to fail until public alpha blockers are resolved.
See <a href="docs/release/RELEASE_PROCESS.md"><font color="#8250df">docs/release/RELEASE_PROCESS.md</font></a> for the
alpha release process.</font>

## <font color="#8250df">Roadmap</font>

<font color="#8250df">No public roadmap commitments are made during alpha. Public issues, labels, and
milestones are triage hints only, not delivery promises, unless a later owner
decision publishes a concrete roadmap.</font>

<font color="#8250df">`Fleet` is a provisional alpha working name. This repository makes no
trademark claim to the name, and stable package or binary publication under
Fleet namespaces is deferred until the owner completes the public name decision.</font>

## <font color="#8250df">Security and Privacy</font>

<font color="#8250df">Fleet is local-first and has no intended telemetry by default. It can still log
local metadata such as workspace paths, local URLs, session labels, process
command lines, and editor state. Scrub logs and review artifacts before sharing
them publicly.</font>

<font color="#8250df">Local alpha runtime data lives under `~/.fleet/run` and `~/.fleet/mux`
unless `FLEET_RUNTIME_DIR` or `FLEET_MUX_DIR` is set. Manual cleanup is
documented in <a href="docs/LOCAL_DATA_AND_UNINSTALL.md"><font color="#8250df">Local data and uninstall</font></a>.</font>

## <font color="#8250df">Legal</font>

<font color="#8250df">The alpha uses the local `code serve-web` install. Fleet does not download,
bundle, host, or redistribute Microsoft's VS Code Server, Microsoft
Marketplace extensions, or Microsoft remote extensions.</font>

<font color="#8250df">See <a href="SECURITY.md"><font color="#8250df">SECURITY.md</font></a> for the current alpha security policy.</font>
<font color="#8250df">See <a href="SUPPORT.md"><font color="#8250df">SUPPORT.md</font></a> for the current alpha support boundary.</font>

## <font color="#8250df">License</font>

<font color="#8250df">Fleet is licensed under the MIT License. See <a href="LICENSE"><font color="#8250df">LICENSE</font></a>.</font>
