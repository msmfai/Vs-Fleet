# VS-Fleet — North Star

**VS-Fleet is a VS Code multiplexer *and* a control plane over a cloud compute provider.**

Not just a launcher for local editors — the end goal is to drive editor workspaces
onto arbitrary compute (your laptop, a container, an SSH box, or a freshly-provisioned
cloud machine) and manage them all from one rail, with their agent-state (working /
waiting / idle) flowing back live. It lives inside `cluster-infra`, alongside the
bare-metal/OS provisioning primitives (`cluster-bootstrap/` — NixOS images, `disko`,
`configuration.nix`) it will eventually drive.

---

## The end-state ergonomics

From Fleet, in one gesture, you:

```
1. pick a repo            ── git integration, prefer GitHub + GitLab
2. provision compute      ── Hetzner / DigitalOcean / AWS  (a "provider")
3. put the right OS on it  ── a container image OR bare-metal NixOS (cluster-bootstrap/disko)
4. it phones home         ── the call-home invariant, now over the internet
5. clone the repo → ~/    ── you land in a ready VS Code workspace, on fresh compute, with your code
```

> "Take that seriously but not literally — I'm describing the ergonomics." This is the
> shape and feel to build toward, not a frozen spec.

The payoff: a button that turns "I want to work on `org/repo`" into "a live VS Code
workspace running on a machine that didn't exist 60 seconds ago," with Fleet's rail
showing it the moment it's ready and its agent's state pinging you when it needs you.

---

## Architecture — the through-lines (don't lock these off)

**One spawn, many locations.** A server is launched by *one* invocation
(`code_server_args` + `fleet_env`); only *where* it runs differs. Local runs it as a
child process; SSH runs the *exact same* invocation on a remote over `ssh` with tunnels.
Keep this single-invocation discipline — it's what makes every backend behave the same.

**SSH is the cloud last mile.** `FLEET_SPAWN_MODE=ssh` deploys the stack to a remote and
reverse-tunnels its call-home (`-R` for the Hub + bridge, `-L` for the editor surface).
A cloud provider doesn't need a new deploy path — it just **provisions a box, hands off
an SSH target, and the SSH path takes over.** Build providers *in front of* SSH, not
beside it.

**A Provider/Target layer above `spawn()`.** Today `SpawnMode` is `local | container |
ssh`. It is the seed of a `Provider` abstraction:

```
Provider  ::=  local | container | ssh | hetzner | digitalocean | aws | …
                 │ provision()  → a deploy target (often: an SSH endpoint)
                 │ os/image      → container or bare-metal NixOS (cluster-bootstrap)
                 ▼
deploy the stack (one shared code-server spawn)  →  it phones home  →  git-clone the repo
```

Grow `spawn()` into this; don't rebuild it. Cloud providers add a *provisioning
front-end*, then defer to the existing deploy + call-home.

**Call-home is the universal contract.** Every backend PUSHES to Fleet (registers its
bridge, its reporter dials the Hub). Fleet never pulls. This already works across
local / container / ssh; it must keep working over the internet for cloud machines.

**Repo-as-workspace is orthogonal to location.** Cloning a repo into the workspace home
is a per-spawn concern that composes with *any* provider. GitHub/GitLab first; auth via
the user's existing git credentials.

---

## Where we are

- **Multiplexer**: Tauri host with a Discord-style rail + one embedded editor surface
  that navigates between servers (`crates/fleet-host`). License-clean **code-server**
  (Open-VSX), not MS serve-web.
- **Agent-state pipeline**: per-server `fleet-reporter --serve` + a `claude` shim →
  Hub → rail (working / waiting / idle / done). Waiting = the ping (Fleet's whole point).
- **Spawn modes**: `local` (default), `container` (docker `fleet-env` image), `ssh`
  (deploy + reverse-tunnel call-home — the cloud last mile).
- **Headless test suite**: the `fleet-env` container harness — a hyper-specific spec
  (`containers/fleet-env/eval/SPEC/`, 445 entries) with ~170 behaviours implemented and
  green, each carrying a rationale + auto-stamped git provenance.
- **Assumption**: the host / provisioned machine already has `code-server` (and the
  fleet stack). Fleet doesn't ship the editor binary.

## Roadmap toward the north star

1. **Git integration** — spawn takes a repo (GitHub/GitLab), clones it into the
   workspace home. Composes with every provider. *(near-term, testable locally)*
2. **Provider abstraction** — generalize `SpawnMode` → a `Provider` trait; SSH stays
   the deploy primitive underneath.
3. **Cloud providers** — Hetzner / DigitalOcean / AWS: provision a box → SSH target →
   existing deploy + call-home.
4. **OS/image selection** — container vs bare-metal NixOS, driven by `cluster-bootstrap`.
5. **One-gesture flow** — pick repo + provider + OS → a live workspace that phones home.

---

## Principles

- **Phone-home, never pull.** Servers establish the connection to Fleet.
- **One spawn invocation**, parameterized by location — never fork the editor launch.
- **License-clean** (code-server / Open-VSX).
- **Don't lock off the cloud direction** — every design choice should compose with
  "provision a remote machine and let it call home."
- **Agent-state is the point** — the rail must always reflect what each workspace's
  agent is doing, wherever it runs.
