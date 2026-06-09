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
| **fleet-reporter** | `cargo build -p fleet-reporter` | Per-server agent-state → Hub. **The app must bundle it or find it on PATH** — currently it can't locate it, so the reporter fails (fix pending: bundle / next-to-exe discovery). |
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

- **fleet-bridge install for serve-web** — `code serve-web` has no `--extensions-dir`, so
  the bridge (observe/act + rail registration) must be installed into the served VS Code
  Server another way (e.g. `code --install-extension <vsix>`). Until then a server starts
  but won't register in the rail.
- **fleet-reporter discovery** — the bundled `Fleet.app` can't find `fleet-reporter` on a
  GUI-launch PATH; bundle it into the `.app` / look next-to-exe.
- **Binary discovery on GUI launch** — apps opened from Finder don't inherit your shell
  PATH, so `code` / `fleet-reporter` may be missing; resolve via absolute paths or PATH
  augmentation (or launch Fleet from a shell).
- **Surface spawn errors** — the rail's `spawnServer` swallows errors; a failed `+` shows
  nothing. Show the reason instead.
