# VS-Fleet — Requirements

What must be present on the Mac (and on an SSH/remote target) for Fleet to run. Scope:
**local + existing SSH** (see `NORTH_STAR.md`). Declared so nothing goes silently missing
again — `code-server` was, which is why `+` (new server) failed with no feedback.

## Runtime — on the Mac (local servers)

| Requirement | Source | Why |
|---|---|---|
| **code-server** | nix — `modules/home/packages/infra.nix` | The editor. Fleet launches one per local server; the rail embeds it. **Was missing → added.** |
| **claude** (Claude Code) | user-managed — `~/.local/bin/claude` | Agent-state: the per-server `claude` shim wraps it so the rail shows working/waiting/idle. Kept latest outside nix by choice. |
| **git** | nix | Clone-on-spawn (`FLEET_SPAWN_REPO` → repo-as-workspace). |
| **openssh** (`ssh`) | system — `/usr/bin/ssh` | SSH deploy (`FLEET_SPAWN_MODE=ssh`) + its `-L`/`-R` tunnels. |
| **netcat** (`nc`, BSD with `-U`) | system — `/usr/bin/nc` | The claude shim relays lifecycle hooks to the reporter's unix socket via `nc -U`. |

## Built from this repo (not external packages)

| Built | How | Note |
|---|---|---|
| **fleet-host** (the Tauri app) | `crates/fleet-host/bundle.sh` → `Fleet.app` | The multiplexer window. |
| **fleet-reporter** | `cargo build -p fleet-reporter` | Per-server agent-state → Hub. **The .app must bundle it or find it on PATH** (currently the app can't locate it → reporter fails; fix: bundle into `Fleet.app` / next-to-exe discovery). |
| **fleet-hub** | `cargo build -p fleet-hub` | The agent-state Hub the app + reporters connect to. |
| **fleet-bridge** (`.vsix`) | the bridge build | code-server extension: observe/act + rail registration. Installed into the editor's extensions dir. |

## Container mode (available, not the current scope)

| Requirement | Source | Why |
|---|---|---|
| **docker** + **colima** | nix — `infra.nix` | `FLEET_SPAWN_MODE=container` runs the `fleet-env` image. Deferred from the daily-driver scope. |

## Dev / build

| Requirement | Source |
|---|---|
| **rust / cargo** | nix — builds the crates above |
| **nodejs** | nix (`nodejs_22`) — the eval harness + the `.vsix` build |

## Deferred (cloud — not used yet)

| Requirement | Source | Why |
|---|---|---|
| **hcloud** | nix — `infra.nix` | Hetzner Cloud CLI. Already present, for the future cloud provider (north-star roadmap, deferred). |

---

After adding `code-server` to nix, apply with your usual rebuild, e.g.:

```bash
darwin-rebuild switch --flake ~/.config/nix-darwin
```

Then `code-server` is on PATH and Fleet's `+` can launch a local server. (Two code fixes
still needed for a clean daily-driver: bundle/locate `fleet-reporter` from the `.app`, and
surface spawn errors in the rail instead of swallowing them.)
