# VS-Fleet — Requirements

VS-Fleet is **OS- and processor-agnostic** — it runs wherever its deps do (Linux, macOS,
x86, arm). This lists *what* must be present on a host running Fleet (and on an SSH/remote
target), not *how* a given OS provides it. Scope: **local + existing SSH** (see
`NORTH_STAR.md`). Declared so nothing goes silently missing again — `code-server` was,
which is why `+` (new server) failed with no feedback.

> "How it's provided" is host-specific. On the current dev host (a nix-darwin machine)
> these are declared in that machine's nix config; on a Linux box it'd be apt/nix/etc. The
> project assumes **nothing** about OS, arch, or package manager.

## Runtime — on the host that runs a server (local or remote)

| Requirement | Why | Provisioning |
|---|---|---|
| **VS Code** (`code serve-web`) | The editor — **Microsoft's official** web server. Fleet runs `code serve-web` per server; the rail embeds it. Same `code` CLI, local or over SSH. (Personal/own-hardware use is fine; see the licensing note in `NORTH_STAR`.) | the installed VS Code app provides the `code` CLI (already present on the dev host); on `aarch64-darwin` nixpkgs also has it as `vscode` (unfree). **Not** code-server — it isn't packaged for `aarch64-darwin` and pulls Open-VSX instead of the MS Marketplace. |
| **claude** (Claude Code) | Agent-state: the per-server `claude` shim wraps it so the rail shows working/waiting/idle. | host install (cross-platform) |
| **git** | Clone-on-spawn (`FLEET_SPAWN_REPO` → repo-as-workspace). | host package manager |
| **ssh** (OpenSSH client) | SSH deploy (`FLEET_SPAWN_MODE=ssh`) + its `-L`/`-R` tunnels. | usually system-provided |
| **nc** (netcat with `-U`) | The claude shim relays lifecycle hooks to the reporter's unix socket via `nc -U`. | usually system-provided |

## Built from this repo (not external packages)

| Built | How | Note |
|---|---|---|
| **fleet-host** (the app) | `crates/fleet-host` (Tauri) | The multiplexer window. |
| **fleet-reporter** | `cargo build -p fleet-reporter` | Per-server agent-state → Hub. The app bundle includes it for packaged/debug launches, and Fleet also resolves a next-to-exe or PATH copy. |
| **fleet-hub** | `cargo build -p fleet-hub` | The agent-state Hub. |
| **fleet-bridge** (`.vsix`) | the bridge build | code-server extension: observe/act + rail registration. |

## Container mode — available, deferred from the current scope

| Requirement | Why | Provisioning |
|---|---|---|
| **docker** + a daemon | `FLEET_SPAWN_MODE=container` runs the `fleet-env` image. | host (e.g. colima/Docker Desktop/native dockerd) |

## Dev / build

| Requirement | Why |
|---|---|
| **rust / cargo** | builds the crates above |
| **nodejs** | the eval harness + the `.vsix` build |

## Deferred (cloud — not used yet)

| Requirement | Why |
|---|---|
| **provider CLI/SDK** (e.g. hcloud) | future cloud providers (north-star roadmap). Out of current scope. |

## Known open items (block a clean local daily-driver)

- ✅ **fleet-bridge install for serve-web** — verified on VS Code 1.123.0:
  `code serve-web --server-data-dir <D>` loads extensions from `<D>/extensions`.
  Fleet now installs the bundled `fleet-bridge` VSIX there before starting each local
  server.
- ✅ **Local spawned server state** — Fleet defaults local `code serve-web`
  workspaces, server data, logs, shims, reporter sockets, and `TMPDIR` under
  `~/.fleet/mux` instead of macOS temp folders. Override with `FLEET_MUX_DIR`.
- ✅ **Cold host reboot hygiene** — the embedded Hub is a non-persisted live
  mirror under `~/.fleet/run`, and bridge registrations carry a per-window token
  so stale orphaned servers from an older Fleet process cannot attach to a new
  window.
- ✅ **GUI-launch binary discovery** — the debug app bundle includes
  `fleet-reporter`, resolves bundled helpers next to `fleet-host`, and builds a
  conservative tool PATH for Finder/LaunchServices launches. The path keeps the
  inherited PATH first, then adds common macOS/Homebrew/Nix/Home Manager CLI
  locations so `code`, `docker`, `claude`, and cmux-style wrappers are still
  discoverable without launching Fleet from a shell.
- ✅ **Surface spawn errors** — spawn failures from the rail button, app menu, and
  startup autospawn path emit a `host-status` event and persist the latest error
  long enough for the rail to show the reason in its status pill.
