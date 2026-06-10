# Launching Fleet v1

Fleet v1 actually runs. This is how to bring up the whole stack and see live
agent state — both the headless path (proven in CI-style smokes) and the real
VS Code path.

## Components

| Binary / artifact | What it is | Build |
|---|---|---|
| `fleet-hub` | Standalone canonical state daemon for headless/external-hub runs. The macOS app embeds this by default. | `cargo build -p fleet-hub` |
| `fleet-reporter` | Per-window reporter; `--serve` receives agent hooks → Hub. | `cargo build -p fleet-reporter` |
| `fleet` | CLI face (`fleet ls`). | `cargo build -p fleet-cli` |
| `fleet-host` | The Tauri sidebar **window** (starts the local persistent Hub, subscribes, renders the inbox). | `cd crates/fleet-host && cargo build` |
| `fleet-extension-0.1.0.vsix` | The VS Code extension. | `cd packages/extension && npm install && npx @vscode/vsce package --allow-missing-repository` |

The two sockets, kept distinct (see `fleet_protocol::paths` / `packages/extension/src/paths.ts`):
- **Hub socket** (`hub.sock`) — reporters + faces connect to the Hub.
- **Reporter socket** (`reporter-<sid>.sock`, `FLEET_REPORTER_SOCKET`) — agent hooks → the per-window reporter.

## A) Headless bring-up (no VS Code)

```sh
# 1. Hub endpoint
# Normal GUI path: Fleet.app starts the local persistent Hub itself.
# Headless/no-GUI path: start the standalone Hub explicitly.
target/debug/fleet-hub &

# 2. A window's reporter (binds the reporter socket, registers with the Hub)
export FLEET_REPORTER_SOCKET=/tmp/fleet/reporter-demo.sock
target/debug/fleet-reporter --serve --session-id demo &

# 3. The GUI window (starts/subscribes to the local Hub). Two ways:
#    (a) quick/dev — runs while the launching shell lives:
crates/fleet-host/target/debug/fleet-host &
#    (b) PERSISTENT — a real macOS .app (survives terminal close, LaunchServices):
( cd crates/fleet-host && ./bundle.sh debug && open ./Fleet.app )
#        external hub:  open crates/fleet-host/Fleet.app --args ws://host:port

# 4. Drive an agent. Either run real `claude` with the Fleet hooks…
claude --settings <fleet-hooks.json>             # see packages/extension shim output
#    …or feed a frame by hand (exactly what the hook command does):
printf 'claude {"hook_event_name":"UserPromptSubmit","session_id":"s1","cwd":"'"$PWD"'"}\n' \
  | nc -U "$FLEET_REPORTER_SOCKET"

# 5. Watch it
target/debug/fleet ls            # or just look at the fleet-host window
```

## B) The real VS Code path

1. **Open Fleet**: `Fleet.app` starts the local Hub itself. Use the standalone
   `fleet-hub` binary only for headless or external-hub runs.
2. **Put `fleet-reporter` where the extension can find it.** Either add
   `target/debug` to `PATH`, or set the VS Code setting
   `fleet.reporterBinPath` to the absolute path of `target/debug/fleet-reporter`.
3. **Install the extension:** in VS Code, run *Extensions: Install from VSIX…*
   and pick `packages/extension/fleet-extension-0.1.0.vsix` (or, for iteration,
   open `packages/extension` in VS Code and press **F5** to launch an Extension
   Development Host). The extension auto-activates on startup.
4. **Open an integrated terminal and type `claude`.** The PATH shim transparently
   launches the real `claude` with `--settings` pointed at this window's Fleet
   hooks (your `~/.claude/settings.json` is never touched). As you prompt and
   Claude works/finishes, the `fleet-host` window and `fleet ls` update live.

What happens under the hood on activation: the extension injects
`FLEET_SESSION_ID` + `FLEET_REPORTER_SOCKET` into the window's terminals, writes a
per-window `fleet-hooks.json`, installs the `claude`/`codex` PATH shim, and spawns
this window's `fleet-reporter --serve`. All of it is reversible (disable/uninstall
removes the env, the shim, and stops the reporter).

## Status: Claude live, Codex untested

- **Claude** — live-validated end-to-end (real `claude` hooks → reporter → Hub →
  window/CLI), including the assistant-message inbox preview.
- **Codex** — the hook config (`fleet init`) and adapter are built to OpenAI's
  official Codex-hooks docs and unit-tested, but **not yet run against a real
  `codex` binary**. To validate when you have Codex: `fleet init`, trust the Fleet
  hook via Codex's `/hooks` (never `--dangerously-bypass-hook-trust`), then
  `codex exec` and confirm the payload matches the fixtures.

## Proven end-to-end

- **Synthetic:** a framed hook line → `fleet-reporter --serve` → Hub → both the
  `fleet ls` CLI and the `fleet-host` window, with live `working → idle → dead`.
- **Real Claude (2.1.168):** `claude --settings <shim hooks>` fires
  SessionStart/UserPromptSubmit/Stop/SessionEnd → reporter → Hub → face, with the
  correct session identity, cwd, and `high` confidence on the authoritative exit.

## Notes

- `fleet-host` is a **standalone crate** (its own workspace, excluded from the
  root workspace) so CI's `cargo build --workspace` on Linux never builds the
  native webview stack. Build/run it from `crates/fleet-host`.
- Local Fleet-spawned VS Code servers default to `~/.fleet/mux`, including
  `ws-server-*` workspaces, `cs-userdata-server-*` data dirs, shims, reporter
  sockets, logs, and the `TMPDIR` used by `code serve-web`. Override with
  `FLEET_MUX_DIR` when a test needs isolation.
- Fleet-spawned local processes use a GUI-safe tool PATH. It preserves the
  inherited PATH, then adds common user CLI locations such as `~/.local/bin`,
  Homebrew, per-user Nix profiles, and Home Manager app resource bins. This lets
  a Finder-launched `Fleet.app` still discover `code`, `fleet-reporter`,
  `claude`, and cmux-style wrappers; explicit `FLEET_*_BIN` overrides still win.
- The embedded local Hub is a live mirror by default: it stores its lock/socket
  under `~/.fleet/run` and does not restore old sessions on cold boot. Override
  the runtime directory with `FLEET_RUNTIME_DIR`.
- Fleet generates a per-window bridge token and passes it to spawned servers via
  `FLEET_BRIDGE_TOKEN`; stale orphaned servers from an older Fleet launch cannot
  re-register into a new window.
- The reporter socket defaults to `$XDG_RUNTIME_DIR/fleet/` (unix) or the temp
  dir. Override per-process with `FLEET_REPORTER_SOCKET`.
