# L2.NET — Networking: host↔container binds, host.docker.internal, published ports, isolation

The two-way network contract between Fleet (host) and each environment (container),
under colima/Docker. Three independent paths must all hold:

1. **container → host (phone-home + bridge dial).** The container resolves the host as
   `FLEET_HOST_ADDR` (default `host.docker.internal`, else the default-route gateway) and
   dials `ws://$HOST:51777` (reporter → Hub) and `ws://$HOST:51778` (bridge → Fleet/harness).
2. **host listeners bound 0.0.0.0.** For (1) to reach them, the host-side Hub binds
   `FLEET_WS_ADDR=0.0.0.0` (server.rs `TcpListener::bind`), Fleet's command-bridge binds
   `FLEET_BRIDGE_ADDR=0.0.0.0` (bridge.rs `serve`), and the harness `BridgeHub` binds
   `host:"0.0.0.0"`. Default is loopback (`127.0.0.1`) — unreachable from a container.
3. **host → container (editor embed).** code-server binds `0.0.0.0:8080`; Docker publishes it
   to a free host port via `-p <port>:8080`; the host reaches it at `http://127.0.0.1:<port>/`.

Isolation edge: `--network none` removes path (1)+(3) entirely; the env runs locally but is
un-drivable (reporter/bridge can't dial out; no port can be published).

Exact facts asserted here:
- entrypoint.sh: `HOST=${FLEET_HOST_ADDR:-$(ip route | awk '/default/{print $3}')}`,
  fallback `192.168.64.1`; `FLEET_HUB_URL=ws://$HOST:${FLEET_HUB_PORT:-51777}`,
  `FLEET_BRIDGE_URL=ws://$HOST:${FLEET_BRIDGE_PORT:-51778}`; code-server `--bind-addr 0.0.0.0:8080`.
- Containerfile ENV: `FLEET_HUB_PORT=51777 FLEET_BRIDGE_PORT=51778`.
- harness env.mjs always passes `-e FLEET_HOST_ADDR=host.docker.internal`; publishes `-p
  <port>:8080` UNLESS `docker.network==="none"`.
- run.mjs / test.sh start the Hub with `FLEET_WS_ADDR=0.0.0.0`; spawn.rs notes the bridge must
  launch with `FLEET_BRIDGE_ADDR=0.0.0.0` for containers to reach it.

---

### L2.NET.001 — entrypoint derives HOST from FLEET_HOST_ADDR and builds the dial URLs
- layer: L2
- scenarios: [base]
- isolation: fresh
- precondition: container run with `-e FLEET_HOST_ADDR=host.docker.internal` (harness default).
- action: `entrypoint.sh` computes `HOST`, exports `FLEET_HUB_URL`/`FLEET_BRIDGE_URL`.
- expected: `HOST=host.docker.internal`; `FLEET_HUB_URL=ws://host.docker.internal:51777`;
  `FLEET_BRIDGE_URL=ws://host.docker.internal:51778`.
- assert: `docker logs <name>` contains exactly `host=host.docker.internal
  hub=ws://host.docker.internal:51777 bridge=ws://host.docker.internal:51778`; OR
  `exec("printenv FLEET_HUB_URL")` == that string.
- why: the explicit `FLEET_HOST_ADDR` override must win over the default-route auto-detect; if it
  doesn't, the container dials the wrong host and never phones home under colima.
- status: partial (env.mjs always sets `-e FLEET_HOST_ADDR=host.docker.internal`; banner is
  logged; no test asserts the derived URLs)

### L2.NET.002 — HOST falls back to the default-route gateway when FLEET_HOST_ADDR is unset
- layer: L2
- scenarios: [base]
- isolation: fresh
- precondition: container run WITHOUT `-e FLEET_HOST_ADDR` (override the harness default).
- action: entrypoint runs `ip route | awk '/default/{print $3}'` to find the gateway.
- expected: `HOST` == the container's default-route gateway IP (e.g. `192.168.x.1`), not empty;
  ultimate fallback `192.168.64.1` only if `ip route` yields nothing.
- assert: `exec("ip route | awk '/default/{print $3; exit}'")` non-empty and equals the `host=`
  value in `docker logs`.
- why: not every launcher sets `FLEET_HOST_ADDR` (e.g. Apple Containers). The gateway auto-detect
  is the portability path; a wrong/empty derivation breaks phone-home on those launchers.
- status: TODO (harness always passes FLEET_HOST_ADDR, so the fallback branch is never exercised
  by the suite — needs a dedicated scenario that omits the env)

### L2.NET.003 — container→host reachability: host.docker.internal resolves and connects
- layer: L2
- scenarios: [base]
- isolation: fresh
- precondition: container Up; host has a listener on :51777 (Hub) bound 0.0.0.0.
- action: from inside the container, connect to the host gateway on the Hub port.
- expected: `host.docker.internal` resolves to the host gateway and TCP :51777 is reachable.
- assert: `exec("getent hosts host.docker.internal || nslookup host.docker.internal")` resolves;
  `exec("nc -z -w2 host.docker.internal 51777; echo $?")` == `0` (when the Hub is up).
- machine-state: docker stats `NetIO` rx/tx for the container is non-zero after phone-home.
- why: this is THE container→host primitive the whole agent pipeline rides on (§8). If
  host.docker.internal stops resolving under a colima upgrade, every reporter/bridge dial fails
  even though both endpoints are individually healthy.
- status: TODO (no test execs a reachability probe; reachability is implied by a successful
  bridge registration but never directly asserted)

### L2.NET.004 — host Hub binds 0.0.0.0 (FLEET_WS_ADDR) so containers can reach it
- layer: L2
- scenarios: [base]
- needs: []  (host Hub — SKIP if absent)
- precondition: run.mjs `startHub` (or test.sh) launches `fleet-hub` with `FLEET_WS_ADDR=0.0.0.0`.
- action: the Hub's `TcpListener::bind((FLEET_WS_ADDR, 51777))` listens on all interfaces.
- expected: :51777 is reachable from BOTH host loopback AND the container (via
  host.docker.internal), not just 127.0.0.1.
- assert: host `nc -z 127.0.0.1 51777` == 0; container `exec("nc -z host.docker.internal 51777")`
  == 0; with `FLEET_WS_ADDR` unset (default 127.0.0.1) the container probe would FAIL.
- why: the default loopback bind is invisible to containers — the single most common
  "phone-home silently does nothing" misconfig (§8). Pins that the suite/desktop set 0.0.0.0.
- status: implemented (run.mjs startHub `FLEET_WS_ADDR:"0.0.0.0"`; test.sh same — but the
  cross-namespace reachability is not asserted, only the bind flag is set)

### L2.NET.005 — Fleet command-bridge binds 0.0.0.0 (FLEET_BRIDGE_ADDR) for container registration
- layer: L2
- scenarios: [base]
- precondition: Fleet's `bridge::serve` reads `FLEET_BRIDGE_ADDR` (default 127.0.0.1).
- action: launch Fleet with `FLEET_BRIDGE_ADDR=0.0.0.0`; `serve` binds `(0.0.0.0, bridge_port)`.
- expected: a container's `fleet-bridge` extension can open a WS to `ws://host.docker.internal:51778`
  and send its `hello`; the registry registers it + emits `SERVERS_CHANGED`.
- assert: container `exec("nc -z host.docker.internal 51778")` == 0; the desktop registry's
  `servers()` gains the `<id>` after the hello. (Harness equivalent: `BridgeHub` binds
  `host:"0.0.0.0"` and `connected(id)` flips true.)
- why: container-mode spawn (spawn.rs) is explicitly documented to REQUIRE `FLEET_BRIDGE_ADDR=
  0.0.0.0` — otherwise a desktop-spawned container can never register and the rail tab never
  appears, even though the container is healthy.
- status: implemented (lib/bridgeHub.mjs binds 0.0.0.0; bridge.rs honors FLEET_BRIDGE_ADDR —
  desktop side has no integration test asserting container registration)

### L2.NET.006 — published port: host→container editor reach on -p <port>:8080
- layer: L2
- scenarios: [base]
- isolation: fresh
- precondition: `docker run ... -p <port>:8080`; code-server `--bind-addr 0.0.0.0:8080`.
- action: from the host, GET `http://127.0.0.1:<port>/`.
- expected: Docker forwards the host port to the container's 8080; code-server answers 302/200.
- assert: `curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:<port>/` ∈ {302,200};
  `docker port <name> 8080/tcp` reports `0.0.0.0:<port>` (or `:::<port>`).
- why: the editor is only reachable because code-server binds 0.0.0.0 (NOT loopback inside the
  container) AND Docker publishes it. A loopback in-container bind would make the published port
  dead despite a healthy container.
- status: implemented (env.reset http-wait on the published port; exercised by base boot)

### L2.NET.007 — published port is unique per env (free-port allocation, no collision)
- layer: L2
- scenarios: [base]
- precondition: N parallel envs, each `freePort()`-allocated.
- action: run.mjs allocates a distinct host port per env before `docker run`.
- expected: no two concurrent envs publish the same host port; no `bind: address already in use`.
- assert: the set of `-p` host ports across N concurrent envs has N distinct values; no env's
  `docker run` fails with a port-in-use error; all N serve 302/200 on their own port.
- why: the PLAN explicitly removed the fixed `8200+i` scheme (§4 Track A) for free-port alloc.
  A collision would make one env's editor shadow another's or fail to publish.
- status: partial (run.mjs `freePort()` per env implements it; no test asserts N distinct ports
  under concurrency)

### L2.NET.008 — desktop inspects the published HostPort to build the embed URL
- layer: L2
- scenarios: [base]
- precondition: container-mode spawn published `-p <free>:8080`.
- action: `spawn.rs::inspect_url` runs `docker inspect -f '{{(index (index
  .NetworkSettings.Ports "8080/tcp") 0).HostPort}}' <name>`.
- expected: returns `http://127.0.0.1:<hostPort>/` where `<hostPort>` == the published port;
  falls back to the asked-for port if inspect fails.
- assert: the inspected HostPort equals the `-p` port; the resulting URL answers 302/200; this
  URL is what `mux::select` navigates the editor webview to.
- why: the rail embeds exactly the inspected URL. If the Docker port-binding JSON shape changes,
  inspect returns empty and the embed silently falls back (possibly to a wrong port) → blank tab.
- status: TODO (Rust inspect_url path; harness uses the `-p` port directly, never the inspect
  template — no test covers it)

### L2.NET.009 — reporter dials the Hub over the network, registering a session
- layer: L2
- scenarios: [base]
- needs: []  (host Hub — SKIP if absent)
- precondition: Hub up on host 0.0.0.0:51777; container can reach host.docker.internal.
- action: entrypoint's `fleet-reporter --serve --ws ws://host.docker.internal:51777`.
- expected: the reporter opens the WS and registers a session titled `<id>`.
- assert: host `target/debug/fleet ls --once` lists a session titled `<id>`; container docker
  stats `NetIO` tx is non-zero. SKIP cleanly if the Hub/CLI is unavailable.
- why: the reporter's dial is the first real use of the container→host network path; a failure
  here (vs. a healthy bridge) isolates a Hub-reachability problem from a bridge-reachability one.
- status: TODO (agentInput.mjs queries the Hub for run state, not the bare session registration
  over the network path)

### L2.NET.010 — bridge dials Fleet over the network and round-trips a query
- layer: L2
- scenarios: [base]
- precondition: container can reach host :51778; bridge registered.
- action: from the host, send `{type:"query", reqId}` over the registered conn.
- expected: the bridge (in-container) replies `{type:"result", reqId, ok:true, data:Snapshot}`
  across the network — proving bidirectional traffic, not just the inbound hello.
- assert: `env.observe()` returns a Snapshot with `terminalCount`/`activeEditor` fields; the
  round-trip completes under the 15s `BridgeHub.send` timeout.
- why: the hello proves inbound; a query round-trip proves the FULL duplex command channel works
  over host.docker.internal, which is what native-menu command forwarding depends on.
- status: implemented (every behaviour's `env.observe()`/`act()` is a host↔container round-trip;
  proven by terminal.new etc.)

### L2.NET.011 — EDGE: --network none disables phone-home but code-server runs locally
- layer: L2
- scenarios: [no-network]
- isolation: fresh
- precondition: `docker run --network none` (no `-p`, no route to host).
- action: container boots; reporter + bridge attempt to dial host.docker.internal.
- expected: code-server still binds 0.0.0.0:8080 LOCALLY; the reporter's Hub dial and the
  bridge's dial both fail (no route); no Hub session, no bridge registration; env un-drivable.
- assert: container `exec("ss -ltn | grep ':8080'")` shows code-server listening locally;
  `exec("nc -z -w2 host.docker.internal 51777; echo $?")` != 0 (no route);
  `BridgeHub.connected(id)` stays false. (run.mjs records `expectBoot:"fail"`.)
- why: the §7 claim made precise — "phone-home FAILS but commands WORK locally." The two must be
  decoupled: isolating the env's network must not crash code-server, only sever Fleet's view.
- status: partial (scenarios/resourceFailure.mjs `no-network` declares expectBoot:fail + boots
  with --network none; the local-up/phone-home-down asserts are documented as deferred to a
  no-network-scoped exec behaviour — not yet implemented)

### L2.NET.012 — EDGE: --network none publishes no port (host can't reach the editor)
- layer: L2
- scenarios: [no-network]
- isolation: fresh
- precondition: `docker.network==="none"` → env.mjs omits the `-p <port>:8080` flag.
- action: attempt to GET `http://127.0.0.1:<port>/` from the host.
- expected: no port is published; the host curl connection is refused (nothing to forward to).
- assert: `_dockerRunCmd()` contains no `-p` for this env; `docker port <name>` is empty; a host
  `curl --max-time 3 http://127.0.0.1:<port>/` returns `000` (connection refused/no route).
- why: with `--network none` there is nothing to publish — env.mjs correctly skips `-p` and the
  http-wait. A regression that still tried to publish would error the `docker run` itself.
- status: implemented (env.mjs `if (d.network !== "none") parts.push(-p ...)` + skips http-wait)

### L2.NET.013 — EDGE: Hub bound loopback (FLEET_WS_ADDR unset) → container can't phone home
- layer: L2
- scenarios: [base]
- precondition: Hub started WITHOUT `FLEET_WS_ADDR` (defaults to 127.0.0.1).
- action: a normal (networked) container tries to dial `ws://host.docker.internal:51777`.
- expected: the host port is bound loopback-only, invisible to the container; the reporter's dial
  is refused; no Hub session registers (even though the editor + bridge are fine if the BRIDGE
  is correctly 0.0.0.0).
- assert: container `exec("nc -z -w2 host.docker.internal 51777; echo $?")` != 0; `fleet ls
  --once` shows NO session for `<id>`; contrast with NET.004 where the 0.0.0.0 bind succeeds.
- why: the negative control for NET.004 — proves the 0.0.0.0 bind is load-bearing, not cargo-cult.
  The most common silent phone-home failure (§8) reproduced deliberately.
- status: TODO (no scenario starts the Hub on loopback to reproduce the failure; the suite only
  exercises the correct 0.0.0.0 path)

### L2.NET.014 — EDGE: custom Hub/bridge ports honored (FLEET_HUB_PORT / FLEET_BRIDGE_PORT)
- layer: L2
- scenarios: [base]
- precondition: container run with `-e FLEET_HUB_PORT=<h> -e FLEET_BRIDGE_PORT=<b>` overriding the
  Containerfile defaults (51777/51778).
- action: entrypoint builds `FLEET_HUB_URL=ws://$HOST:<h>` / `FLEET_BRIDGE_URL=ws://$HOST:<b>`.
- expected: the reporter dials the custom Hub port and the bridge dials the custom bridge port.
- assert: `exec("printenv FLEET_HUB_URL")` ends `:<h>`; `exec("printenv FLEET_BRIDGE_URL")` ends
  `:<b>`; with a matching host listener on `<b>`, the bridge registers; mismatch → no registration.
- why: the desktop supervisor passes `FLEET_BRIDGE_PORT=<self.bridge_port>` (its own free bridge
  port), which is NOT the default 51778. The container must honor whatever port Fleet chose, or a
  desktop-spawned container dials the wrong port and never registers.
- status: TODO (entrypoint reads the ports; spawn.rs passes a non-default FLEET_BRIDGE_PORT; no
  test overrides the ports and asserts the derived URLs / registration)

### L2.NET.015 — EDGE: concurrent envs share one bridge listener without cross-talk
- layer: L2
- scenarios: [base]
- precondition: N envs all dialing the SAME host bridge (:51778 harness / desktop bridge port).
- action: each container's bridge opens its own WS to the one host listener and sends a distinct
  `hello{server_id:<id_i>}`.
- expected: the host listener accepts N independent conns; each keyed by its own `server_id`; a
  `command(id_i)` reaches ONLY env i.
- assert: `BridgeHub.conns.size == N` with N distinct keys; a `query(id_i)` returns env i's
  Snapshot (e.g. its own terminalCount), never env j's; `send_command` to a missing id logs
  `no bridge for active server — dropped` (bridge.rs) and reaches nobody.
- why: one host port multiplexes all envs; the per-`server_id` keying is what keeps a command from
  leaking to the wrong env. A keying bug would cross-wire two rail tabs' commands.
- status: partial (BridgeHub keys conns by server_id; harness ran 3 envs on one :51778; no test
  asserts command isolation between concurrent envs)

### L2.NET.016 — EDGE: bridge WS drops → host listener notices and the conn is reaped
- layer: L2
- scenarios: [base]
- precondition: a registered bridge conn for `<id>`.
- action: kill the container (`docker rm -f`) or close the editor page so the WS closes.
- expected: the host side's `ws.on("close")` (harness) / `read.next()→Close|None` (bridge.rs
  handle_conn) fires; the conn is removed from the registry; desktop emits `SERVERS_CHANGED`.
- assert: `BridgeHub.connected(id)` flips to false within a tick of the close; a subsequent
  `command(id)` rejects `no bridge for <id>` (harness) / logs dropped (desktop).
- why: networking failure modes must clean up, not pin a dead conn. A half-open socket that's
  never reaped keeps a vanished env addressable and forwards commands into nothing.
- status: partial (bridgeHub.mjs `ws.on("close")` deletes conn+caps; bridge.rs unregisters on
  close — no test asserts the post-drop connected==false / reject observable)

### L2.NET.017 — EDGE: reporter socket is local-only (unix socket, not network-exposed)
- layer: L2
- scenarios: [base]
- isolation: fresh
- precondition: reporter `--serve --socket /tmp/fleet-reporter.sock` inside the container.
- action: the claude hooks relay via `nc -U /tmp/fleet-reporter.sock` (unix socket, in-container).
- expected: the hook→reporter channel is an in-container UNIX socket (mode 0600, owner-only),
  never a TCP port — only the reporter→Hub leg crosses the network.
- assert: container `exec("ls -l /tmp/fleet-reporter.sock")` shows a socket `srw-------` (0600);
  no extra TCP listener appears for hooks (`ss -ltn` unchanged); the network leg is solely the
  reporter's outbound WS to the Hub.
- why: pins the trust boundary (serve.rs `restrict_socket_perms` 0600) — hook frames can mutate
  reported agent state, so that channel must NOT be network-reachable; only the already-trusted
  reporter→Hub dial leaves the container.
- status: TODO (serve.rs binds + chmods the socket 0600; no eval test execs the socket perms /
  asserts no hook TCP port exists)
