# Quickstart

This is the current source-only alpha path. It is intended for local macOS
dogfooding and technical review, not for packaged binary distribution.

Fleet is licensed as `MIT OR Apache-2.0`; public visibility remains gated by
the owner decision record and release evidence in
`docs/release/ALPHA_RELEASE_CHECKLIST.md`.

## Prerequisites

- macOS.
- Rust 1.78 or newer.
- Node.js 20 and npm.
- Visual Studio Code with the `code` CLI available. In VS Code, run
  "Shell Command: Install 'code' command in PATH" if needed.
- Git.

Fleet's supported alpha workflow uses the user's local `code serve-web` install.
Fleet does not download, bundle, host, or redistribute Microsoft's VS Code
Server, Microsoft Marketplace extensions, or Microsoft remote extensions in
this source-only alpha path.

## Build

Install the Fleet bridge dependencies:

```sh
( cd packages/fleet-bridge && npm ci )
```

Run the core Rust checks from the repository root:

```sh
cargo fmt --all -- --check
cargo test --workspace --all-targets --all-features
```

Build and test the macOS host:

```sh
( cd crates/fleet-host && cargo test && ./bundle.sh release )
```

The bundle script builds `fleet-host`, builds `fleet-reporter`, packages the
Fleet bridge VSIX, refreshes the app icon from `crates/fleet-host/icons/icon.png`,
and writes `crates/fleet-host/Fleet.app`.

## Run

Launch the app bundle:

```sh
open crates/fleet-host/Fleet.app
```

The host starts a local Hub and bridge listener. Click `New Server` in Fleet to
spawn a local `code serve-web` session. The spawned editor phones home through
the Fleet bridge and appears in the rail.

Local runtime files are under the user's home directory by default:

- `~/.fleet/run` for the embedded Hub lock, socket, token, and host log.
- `~/.fleet/mux` for spawned workspaces, server logs, user data, and reporter
  sockets.

Fleet is a stateless client for externally registered sessions. Quitting Fleet
must not kill already-running external servers. Closing a Fleet-spawned server
from the UI is the explicit owner action that removes the server process.

## Cleanup

Fleet does not promise an automated source-alpha uninstaller. Close any Fleet-spawned servers from the Fleet UI before deleting runtime data, then run:

```sh
rm -rf ~/.fleet/run ~/.fleet/mux
```

If `FLEET_RUNTIME_DIR` or `FLEET_MUX_DIR` was set, delete those configured
directories instead. See [Local data and uninstall](LOCAL_DATA_AND_UNINSTALL.md)
for the complete local data contract.

## Useful Environment Overrides

Use these only when debugging or dogfooding a non-default setup:

| Variable | Purpose |
|---|---|
| `FLEET_EDITOR_BIN` | Path to the `code` CLI or a compatible `code-server` binary. |
| `FLEET_CODE_SERVER_BIN` | Explicit downloaded `code serve-web` `code-server` binary. |
| `FLEET_REPORTER_BIN` | Reporter binary to launch instead of the bundled one. |
| `FLEET_BRIDGE_VSIX` | Bridge VSIX to install into spawned server data dirs. |
| `FLEET_MUX_DIR` | Alternate directory for spawned server workspaces/logs. |
| `FLEET_RUNTIME_DIR` | Alternate directory for Hub runtime files and host logs. |
| `FLEET_SPAWN_CWD` | Working directory for local child processes Fleet spawns. |
| `FLEET_SPAWN_REPO` | Clone a repo into each spawned workspace. Supports full URLs, `gh:owner/repo`, `gl:owner/repo`, or `owner/repo`. |

Remote, SSH, and container spawn modes exist in the tree but are not supported
alpha commitments yet.

## Release Gate

Run the public-release hygiene check from the repository root:

```sh
./scripts/release-check.sh
```

It is expected to fail until the remaining owner decisions, history review, and
publication evidence are resolved.
