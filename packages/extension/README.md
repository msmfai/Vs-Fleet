# Fleet — VS Code extension

Live agent state across editor windows. Fleet observes terminal-based AI coding
agents (Claude Code, Codex CLI) running in this window's **integrated terminals**
and reports their state to the local Fleet Hub, so a sidebar/CLI face can show
which agents are working, waiting, or done — across every window.

## What it does on activation

- **Injects per-window identity** into integrated-terminal shells via the stable
  `EnvironmentVariableCollection` API: `FLEET_SESSION_ID` (this window) and
  `FLEET_REPORTER_SOCKET` (where this window's reporter listens).
- **Installs a transparent PATH shim** for `claude`/`codex`: you still *type*
  `claude`, but the wrapper `exec`s the real binary with `--settings` pointed at a
  Fleet hooks file, so Claude relays its lifecycle hooks to the reporter —
  **without modifying your `~/.claude/settings.json`** (Claude layers `--settings`
  on top). Outside this editor the shim is not on `PATH` (pure pass-through).
- **Spawns this window's `fleet-reporter --serve`**, which binds the reporter
  socket, registers the window with the Hub, and turns the agent hooks into Hub
  state deltas.

It is **observer-not-owner**: it never intercepts keystrokes, never launches an
agent, and never owns a terminal. Everything is reversible — disabling/uninstalling
removes the injected env, the shim, and stops the reporter.

## Requirements

- A running **Fleet Hub** (`fleet-hub`).
- The **`fleet-reporter`** binary on `PATH` (or set `fleet.reporterBinPath`).

## Settings

| Setting | Default | Purpose |
|---|---|---|
| `fleet.hubWsUrl` | `ws://127.0.0.1:51777` | Hub WebSocket URL. |
| `fleet.hubUnixSocket` | `""` | Hub unix socket (fast path; overrides WS). |
| `fleet.reporterBinPath` | `fleet-reporter` | Path to the reporter binary. |
| `fleet.claude.allowDangerouslySkipPermissions` | `false` | **Opt-in, dangerous.** Prepend `--allow-dangerously-skip-permissions` in shimmed terminals. Never silent. |

Engine: `^1.93.0`. No proposed APIs — Open-VSX-publishable.
