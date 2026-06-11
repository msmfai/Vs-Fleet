# Architecture

Fleet is a local-first control surface for terminal-based coding sessions. The
current implementation is intentionally small: a local Hub, phone-home
reporters/bridges, and a macOS Tauri host that renders registered sessions.

## Mental Model

Fleet is a stateless client for live sessions. Sessions push state to Fleet; Fleet
does not poll a static server list. The host may create local sessions as a
convenience, but externally registered sessions are not owned by the host and
must survive a Fleet restart.

## Supported Surface

The current supported path is:

- macOS Fleet host.
- Local `code serve-web` sessions spawned from the host.
- Fleet bridge extension installed into each spawned server data dir.
- Fleet reporter process per spawned session.
- Embedded local Hub process started by the host when no external Hub URL is
  provided.

Remote, SSH, Docker/container, visual probe, and eval harness paths exist in
the tree, but they are not currently supported user workflows.

Fleet is a user-provided VS Code workflow: Fleet may launch the user's local
`code serve-web` install, but Fleet does not download, bundle, host, or
redistribute Microsoft's VS Code Server, Microsoft Marketplace extensions, or
Microsoft remote extensions.

## Components

| Component | Path | Role |
|---|---|---|
| Protocol | `crates/fleet-protocol` | JSON event and command types shared by Hub, reporter, CLI, and host-core. |
| Hub | `crates/fleet-hub` | Local broker that accepts reporter events and serves subscribed faces. |
| Reporter | `crates/fleet-reporter` | Session-side process that observes agent state and pushes events to the Hub. |
| Host core | `crates/fleet-host-core` | Pure Rust reducer/view model for inbox state. |
| Host app | `crates/fleet-host` | Tauri macOS app with the rail UI, embedded editor webviews, local spawn convenience, and bridge listener. |
| Bridge extension | `packages/fleet-bridge` | VS Code workspace extension that phones home to the host bridge and supports command/probe frames. |
| Extension prototype | `packages/extension` | VS Code extension face and reporter integration prototype. |
| CLI | `crates/fleet-cli` | Command-line face for Hub state. |
| Eval harness | `containers/fleet-env` | Containerized behavior tests and screenshot review tooling. |

## Local Data Flow

1. Fleet host starts.
2. If no external `FLEET_HUB_URL` or CLI argument is supplied, the host starts an
   embedded local Hub under `~/.fleet/run`.
3. The host starts its bridge listener on loopback.
4. When the user creates a local server, Fleet launches a reporter and
   `code serve-web` with Fleet environment variables.
5. The bridge extension inside that editor connects back to Fleet and registers
   `{server_id, url, label}`.
6. The reporter pushes session and run events to the Hub.
7. The host subscribes to the Hub and renders the latest inbox projection.

The rail is driven by live registered state plus Fleet-spawned pending entries.
Selection changes switch embedded webviews; they should not imply ownership of
the server process.

## Ownership Boundaries

- Fleet owns only processes it explicitly spawned and only closes them when the
  user asks to close that server.
- Fleet does not kill external sessions when the host exits.
- Fleet does not persist a server list across restarts. Reporters and bridges
  re-register after the host/Hub comes back.
- Fleet installs only a static AppKit-aware native shell menu with no Edit
  submenu and no editor/server command proxies. It does not install global
  keyboard hooks or app-wide native text-editing accelerators for the embedded
  editor. Keystrokes belong to the active editor webview.

## Logs and Privacy

Fleet is local-first and has no intended telemetry by default. It still observes
developer environments and can log local metadata:

- workspace paths,
- local URLs and ports,
- session labels,
- process command lines,
- editor and agent state.

Logs and screenshots must be scrubbed before being posted publicly.

## Local Data And Cleanup

Fleet writes local runtime data under `~/.fleet/run` for Hub runtime files and
`~/.fleet/mux` for spawned editor workspaces, server logs, VS Code
`--server-data-dir` userdata, reporter sockets, and agent shim files.

Manual cleanup is `rm -rf ~/.fleet/run ~/.fleet/mux` after closing
Fleet-spawned servers from the UI. Quitting Fleet does not promise to delete spawned editor userdata or logs, and it must not kill externally registered sessions.

## Release Boundary

Treat the repo as source code plus build instructions. The project does not
currently publish signed app bundles, extension marketplace packages, or
container images.
