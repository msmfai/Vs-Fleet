# L2.LIFE — Env lifecycle: spawn → phone-home → bridge register → editor reachable → close → cleanup

The full birth-to-death of one Fleet environment, end-to-end through real containers.
The canonical happy path the suite already exercises is `Env.reset()` (lib/env.mjs):
`docker run -d` the `fleet-env:latest` image → `entrypoint.sh` starts
`fleet-reporter --serve` (phone-home to the Hub) + `code-server --bind-addr
0.0.0.0:8080` → poll code-server for `302/200` → Playwright `goto` `?folder=/home/coder/project`
(brings the ext-host online so the `fleet-bridge` extension dials `FLEET_BRIDGE_URL`
and sends its `hello`) → `hub.waitFor(id)` (the bridge registered) → `Env.close()`
(`docker rm -f`). This area pins each transition + its failure edges.

Exact wire facts these entries assert against:
- entrypoint: `HOST = FLEET_HOST_ADDR || default-route gw`; `FLEET_HUB_URL =
  ws://$HOST:51777`, `FLEET_BRIDGE_URL = ws://$HOST:51778`; reporter `--serve --ws
  $FLEET_HUB_URL --socket /tmp/fleet-reporter.sock --session-id $FLEET_SERVER_ID`.
- bridge `hello` frame (`packages/fleet-bridge/src/extension.ts`): `{type:"hello",
  server_id: FLEET_SERVER_ID, url: FLEET_SERVER_URL||"", label: FLEET_SERVER_LABEL||id,
  caps: CAPS}` — `BridgeHub` records the conn + caps keyed by `server_id`.
- reporter registers a Hub **session** titled by `FLEET_SESSION_TITLE` (== id), queryable
  on the host via `target/debug/fleet ls --once` (one line per session).
- desktop supervisor (`crates/fleet-host/src/spawn.rs`) container mode: container name
  `fleet-<id>`, `docker run -d --name fleet-<id> -p <free>:8080`, close = `docker rm -f`,
  drop from `ServerSupervisor.servers` + `containers` maps.

---

### L2.LIFE.001 — `docker run -d` starts the container; entrypoint emits the phone-home banner
- layer: L2
- scenarios: [base]
- isolation: fresh
- precondition: no container named `fleet-eval-<id>`; `fleet-env:latest` present.
- action: `Env.reset()` runs `_dockerRunCmd()` (`docker run -d --name fleet-eval-<id>
  -e FLEET_SERVER_ID=<id> -e FLEET_HOST_ADDR=host.docker.internal -p <port>:8080
  fleet-env:latest`).
- expected: container is created + `Up`; entrypoint logs `[fleet-env] id=<id>
  host=host.docker.internal hub=ws://host.docker.internal:51777 bridge=ws://host.docker.internal:51778`.
- assert: `exec("printf ''")`-style — `docker inspect -f '{{.State.Running}}' fleet-eval-<id>`
  == `true`; `docker logs fleet-eval-<id>` contains the exact `[fleet-env] id=<id>` line with
  `hub=ws://host.docker.internal:51777`.
- machine-state: container exists (was absent); 1 container delta.
- why: the very first link — the image must boot under the harness's exact run argv and
  resolve the host gateway to `host.docker.internal`; a banner mismatch means the
  entrypoint's host/url derivation drifted before anything else can phone home.
- status: implemented (life.bannerAndRunning)

### L2.LIFE.002 — code-server serves 302/200 on the published host port
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: []
- precondition: container Up, `-p <port>:8080` published, `--network` != none.
- action: reset()'s readiness loop curls `http://127.0.0.1:<port>/` up to 60×1s.
- expected: code-server (`--bind-addr 0.0.0.0:8080`) answers `302` (auth redirect) or `200`
  within the window; reset() proceeds past the http-wait.
- assert: `curl -s -o /dev/null -w '%{http_code}' --max-time 3 http://127.0.0.1:<port>/`
  ∈ {302,200}; reset() does NOT throw `code-server never served 302/200`.
- machine-state: container `procs` includes the code-server node process.
- why: pins the published-port → `0.0.0.0:8080` bind path AND the readiness contract (§8:
  wait for 302/200, not any byte). A regression in the bind addr or the port publish surfaces
  here as a 60s timeout rather than a silent half-boot.
- status: implemented (env.reset http-wait — exercised by `base` scenario boot)

### L2.LIFE.003 — opening the editor brings the ext-host online so the bridge dials home
- layer: L2
- scenarios: [base]
- isolation: fresh
- precondition: code-server serving 302/200; ext-host NOT yet started (no workbench client).
- action: Playwright `page.goto(http://127.0.0.1:<port>/?folder=/home/coder/project)`.
- expected: the `fleet-bridge` extension activates (workspace-trust disabled in the image)
  and opens a WS to `FLEET_BRIDGE_URL`, sending its `hello`.
- assert: `BridgeHub.connected(id)` flips true (the conn map gains `<id>`) within
  `hub.waitFor(id, 60000)`; the harness records the `hello` (caps set populated).
- machine-state: a code-server ext-host child process appears (procs +1..+2).
- why: the §8 gotcha made first-class — pure HTTP never starts the ext-host, so the bridge
  only registers after a workbench client connects. If `extensionKind:["workspace"]` /
  trust-disable regresses, the extension installs but never activates and waitFor times out.
- status: implemented (env.reset Playwright goto + hub.waitFor — every base behaviour depends on it)

### L2.LIFE.004 — bridge `hello` registers the server with id, caps, label, url
- layer: L2
- scenarios: [base]
- precondition: bridge WS open to `FLEET_BRIDGE_URL`.
- action: bridge sends `{type:"hello", server_id:<id>, url:FLEET_SERVER_URL, label:<id>, caps:CAPS}`.
- expected: `BridgeHub` inserts `conns[<id>]=ws` and `caps[<id>]=Set(BASELINE ∪ CAPS)`; the
  server is now drivable.
- assert: `env.supports("command")` && `env.supports("query")` && `env.supports("typeText")`
  all true; a follow-up `query` round-trips (returns `{ok:true, data:Snapshot}`).
- why: registration IS how a server becomes addressable — there is no static list (bridge.rs
  invariant: servers PUSH, Fleet never pulls). Caps gate every other behaviour, so a missing/
  malformed caps array silently skips half the suite.
- status: implemented (bridgeHub.mjs hello handler + caps tracking; supports() used by run.mjs gate)

### L2.LIFE.005 — reporter phone-home registers a Hub session titled by env id
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: []  (host Hub on :51777 — runtime-SKIP if absent, like agent.* behaviours)
- precondition: a Hub listening on host `0.0.0.0:51777` (run.mjs `startHub` or test.sh).
- action: entrypoint's `fleet-reporter --serve --ws ws://host.docker.internal:51777
  --session-id <id>` registers a session.
- expected: a Hub session with title == `<id>` (FLEET_SESSION_TITLE) appears; 0 runs yet.
- assert: host `target/debug/fleet ls --once` stdout has a line whose title == `<id>`
  (state `idle`/no-run); if the CLI/Hub is absent → clean SKIP (never hard-fail), mirroring
  agentInput.mjs's gate.
- machine-state: container `procs` includes the `fleet-reporter` process.
- why: phone-home is the agent-state spine — without a registered session the rail shows
  nothing for this env. Distinguishes "container booted" from "container is visible to Fleet".
- status: implemented (life.reporterSessionRegistered)

### L2.LIFE.006 — the embedded editor URL is 302-reachable from the host (desktop embed contract)
- layer: L2
- scenarios: [base]
- precondition: container Up, port published, `inspect_url` resolvable.
- action: resolve the host-reachable URL the way `spawn.rs::inspect_url` does:
  `docker inspect -f '{{(index (index .NetworkSettings.Ports "8080/tcp") 0).HostPort}}' <name>`
  → `http://127.0.0.1:<hostPort>/`.
- expected: that exact URL (what Fleet's mux navigates the editor webview to) answers 302/200.
- assert: parse HostPort via the inspect template above; `curl` it → ∈ {302,200}; the port
  equals the `-p` host port reset() chose.
- why: the desktop multiplexer embeds precisely this inspected URL (mux.rs `select` →
  `wv.navigate`). If docker's port-binding shape or the inspect template drifts, the rail tab
  shows a blank/unreachable editor even though the container is healthy.
- status: implemented (life.inspectUrlReachable)

### L2.LIFE.007 — `Env.close()` removes the container (no orphan)
- layer: L2
- scenarios: [base]
- isolation: fresh
- precondition: container Up with name `fleet-eval-<id>`.
- action: `Env.close()` → `docker rm -f fleet-eval-<id>` (after `browser.close()`).
- expected: the container no longer exists; the Playwright browser is closed.
- assert: `docker ps -a --filter name=fleet-eval-<id> -q` is empty; `docker inspect
  fleet-eval-<id>` exits non-zero (No such object).
- machine-state: container count back to pre-reset; the chromium process is gone.
- why: the suite's hard DoD is **zero orphan containers**. close() must succeed even on a
  half-built env; an orphan leak would compound across a parallel matrix and exhaust colima.
- status: implemented (env.close() — run.mjs `finally { env.close() }` per scenario/behaviour)

### L2.LIFE.008 — closing the container drops it from the bridge registry (deregister)
- layer: L2
- scenarios: [base]
- precondition: bridge registered (`BridgeHub.connected(id)` true).
- action: `docker rm -f fleet-eval-<id>` (the bridge WS dies with the container).
- expected: the bridge conn closes → `BridgeHub` deletes `conns[<id>]` and `caps[<id>]`.
  (Desktop equivalent: `bridge.rs::handle_conn` unregisters + emits `SERVERS_CHANGED`, removing
  the rail tab.)
- assert: after close, `BridgeHub.connected(id)` == false; a `query(id)` rejects with
  `no bridge for <id>`.
- why: a server "vanishes from the rail when its bridge drops" (bridge.rs). A leaked conn entry
  would keep a dead env addressable and forward commands into the void.
- status: partial (BridgeHub.ws `close` handler deletes the conn — but no test asserts the
  post-close `connected(id)==false` / query-rejects observable)

### L2.LIFE.009 — desktop container-mode spawn: `docker run` + record in supervisor maps
- layer: L2
- scenarios: [base]
- precondition: `FLEET_SPAWN_MODE=container`, `FLEET_BRIDGE_ADDR=0.0.0.0` set on Fleet.
- action: `ServerSupervisor::spawn_container` runs `docker run -d --name fleet-<id>
  -e FLEET_SERVER_ID=<id> -e FLEET_HOST_ADDR=host.docker.internal -e FLEET_BRIDGE_PORT=<port>
  -e FLEET_HUB_URL=<hub> -p <free>:8080 fleet-env:latest`.
- expected: returns a `Server{id, label:id, url}`; `containers[<id>]=fleet-<id>` and the server
  is pushed to `servers`. The env phones home on its own (rail gains the tab via
  `SERVERS_CHANGED`).
- assert: `docker ps --filter name=fleet-<id> -q` non-empty; `sup.servers()` contains `<id>`;
  `sup.containers` maps `<id> → fleet-<id>`.
- why: this is the desktop path the harness mirrors (spawn.rs doc-comment references harness.mjs).
  It must wire the SAME env (HOST_ADDR/BRIDGE_PORT/HUB_URL) so a desktop-spawned env is
  indistinguishable from a harness-spawned one.
- status: TODO (eval harness drives `docker run` directly; the Rust `spawn_container` path has
  no container-backed integration test yet)

### L2.LIFE.010 — desktop container-mode close: `docker rm -f` + drop from maps (no orphan)
- layer: L2
- scenarios: [base]
- precondition: a container-mode server `<id>` recorded in `sup.containers`.
- action: `ServerSupervisor::close(<id>)`.
- expected: `containers.remove(<id>)` hits → `docker rm -f fleet-<id>`; `<id>` retained out of
  `servers`; returns true; `SERVERS_CHANGED` emitted (rail tab removed).
- assert: `docker ps -a --filter name=fleet-<id> -q` empty; `sup.servers()` no longer contains
  `<id>`; second `close(<id>)` returns false (already gone).
- why: container-mode must `docker rm` (not try to `child.kill` a process it never spawned).
  Mismatched bookkeeping leaves an orphan container OR a phantom rail tab.
- status: TODO (no integration test for spawn.rs container close)

### L2.LIFE.011 — EDGE: spawn over a stale same-name container (force-rm then run)
- layer: L2
- scenarios: [base]
- isolation: fresh
- precondition: a leftover container named `fleet-eval-<id>` exists (prior crashed run).
- action: `Env.reset()` first runs `docker rm -f fleet-eval-<id> || true`, then `docker run`.
  (Desktop: `spawn_container` runs the same best-effort `docker rm -f` before `run`.)
- expected: the stale container is removed and a fresh one starts; no `name already in use` error.
- assert: only ONE container named `fleet-eval-<id>` exists post-reset; its created-time is
  newer than the stale one; reset() reaches `hub.waitFor` normally.
- why: parallel/retried runs MUST be idempotent on the container name. Without the pre-rm,
  a crashed prior run wedges every retry with a name collision.
- status: implemented (env.reset `docker rm -f ... || true`; spawn.rs pre-`rm -f` — but no test
  seeds a stale container then asserts single-instance)

### L2.LIFE.012 — EDGE: bridge never registers → bounded boot failure, no hang
- layer: L2
- scenarios: [no-network]
- precondition: `--network none` — the bridge can't dial the host; ext-host never registers.
- action: `Env.reset()` skips the http/Playwright path and goes straight to
  `hub.waitFor(id, 60000)`.
- expected: `waitFor` throws `bridge <id> never connected` after ≤60s; `bootOrReport` (run.mjs)
  records it as an EXPECTED pass for `expectBoot:"fail"`; the pool keeps moving.
- assert: the result row for the no-network env reads `boot-failed-as-expected
  (expectBoot:fail): bridge <id> never connected`; total boot wall-clock ≤ `BOOT_TIMEOUT_MS`.
- machine-state: container still Up (code-server runs locally) — close() still removes it.
- why: an un-drivable env must fail FAST and CLEAN, never wedge the parallel pool on a hang.
  Guards the boot-failure plumbing (waitFor throws → bootOrReport catches).
- status: implemented (scenarios/resourceFailure.mjs `no-network`; run.mjs bootOrReport)

### L2.LIFE.013 — EDGE: code-server never serves → http-wait throws, recorded as expected fail
- layer: L2
- scenarios: [crash-boot]
- precondition: `CODE_SERVER_BIND_ADDR=256.256.256.256:0` (invalid) — code-server refuses to bind.
- action: `Env.reset()`'s 60×1s curl loop never sees 302/200.
- expected: reset() throws `code-server never served 302/200 on :<port>`; `bootOrReport` records
  an EXPECTED pass (`expectBoot:"fail"`).
- assert: result row reads `boot-failed-as-expected (expectBoot:fail): code-server never
  served 302/200`; no Playwright/`hub.waitFor` is reached (fails before them).
- why: a poisoned bind addr is the deterministic crash-boot trigger that lives in code-server
  itself (not the entrypoint). Proves a broken editor dies on the http-wait, not on a 3-minute
  pool stall.
- status: implemented (scenarios/resourceFailure.mjs `crash-boot`)

### L2.LIFE.014 — EDGE: boot exceeds BOOT_TIMEOUT_MS → race rejects, env still cleaned up
- layer: L2
- scenarios: [base]
- precondition: an env whose reset() hangs past `FLEET_BOOT_TIMEOUT_MS` (default 180000).
- action: `bootOrReport` races `env.reset()` against a `setTimeout` rejecting `boot timed out
  after <ms>ms`.
- expected: the timeout wins; `env.bootError` set; for `expectBoot:"ok"` the row is a real
  ERROR (`env boot failed: boot timed out`), for fail/degraded an expected pass; the `finally`
  in runScenario still calls `env.close()`.
- assert: result row carries `boot timed out after 180000ms`; `docker ps -a` has no
  `fleet-eval-<id>` afterward (close ran).
- why: an unbounded reset() (e.g. waitFor with a higher cap, or a stuck curl) could starve the
  pool forever. The outer race is the backstop that guarantees forward progress AND cleanup.
- status: implemented (run.mjs bootOrReport Promise.race + runScenario finally close)

### L2.LIFE.015 — EDGE: repeated reset on the same id is clean (sequential lifecycle)
- layer: L2
- scenarios: [base]
- isolation: fresh
- precondition: an env that already booted + closed once with id `<id>`.
- action: construct a new `Env` with the same `<id>` and `reset()` again.
- expected: pre-rm clears any remnant, a fresh container boots, bridge re-registers, Hub
  re-titles the session `<id>` (same title, new run lineage).
- assert: `hub.waitFor(id)` succeeds the 2nd time; `BridgeHub.connected(id)` true; exactly one
  container named `fleet-eval-<id>`.
- why: ids are reused across `--keep`/retry/soak cycles. A second lifecycle on the same id must
  not collide on the container name nor leave a stale bridge conn pinning the slot.
- status: partial (pre-rm + bridgeHub close handler make this work; no explicit two-cycle test)

### L2.LIFE.016 — EDGE: concurrent N-env spawn — no name/port collisions, all phone home
- layer: L2
- scenarios: [base]
- precondition: `--parallel N` over N scenarios (or harness.mjs's proven 3/3 parallel).
- action: the bounded worker pool boots N envs concurrently, each `freePort()`-allocated and
  uniquely id'd (`r<i>-<scenario>`).
- expected: all N containers run with distinct names + distinct published ports; all N bridges
  register on the single `BridgeHub` (:51778); all N reporters register distinct Hub sessions.
- assert: N distinct `docker ps` rows; `BridgeHub.conns.size == N`; `fleet ls --once` shows N
  distinct titles. (PLAN §0 proven: phone-home 3/3 parallel.)
- machine-state: aggregate procs/mem scale ~linearly; no port `EADDRINUSE`.
- why: the whole point is dozens of envs in parallel. Free-port allocation + per-env ids must
  prevent the fixed-port collisions the PLAN explicitly removed (§4 Track A).
- status: partial (proven manually 3/3 in PLAN §0; run.mjs pool + freePort implement it; no
  assertion-bearing concurrency test in the suite)

### L2.LIFE.017 — EDGE: close() on a half-built env (boot threw) still removes the container
- layer: L2
- scenarios: [crash-boot, no-network]
- precondition: `reset()` threw partway (no page, or no bridge) — `env.bootError` set.
- action: runScenario's `finally` calls `env.close()` regardless.
- expected: `browser?.close()` is a no-op-safe optional; `docker rm -f` still runs and removes
  whatever container `docker run` created (even if code-server never served).
- assert: post-run `docker ps -a --filter name=fleet-eval-<id> -q` empty for the crash-boot /
  no-network envs; no error escapes close() (it swallows).
- why: failure scenarios are exactly where orphans hide — close() must clean up a container that
  exists even though its editor/bridge never came up. The DoD "zero orphans" must hold under
  failure, not just success.
- status: implemented (env.close swallows + always `docker rm -f`; run.mjs finally; failure
  scenarios exist)

### L2.LIFE.018 — EDGE: claude auth injection failure does not break boot
- layer: L2
- scenarios: [base]
- precondition: neither `ANTHROPIC_API_KEY` nor a host `~/.claude/.credentials.json` nor
  Keychain access is available; `FLEET_CLAUDE_OAUTH` unset.
- action: reset() calls `_injectClaudeAuth()` after the bridge is live.
- expected: it returns false after ≤5 retries without throwing; `env.claudeAuthed=false`; boot
  still completes; agent.* behaviours later SKIP, non-agent behaviours run.
- assert: `env.claudeAuthed === false`; reset() did NOT throw; a non-agent behaviour (e.g.
  terminal.new) still passes on this env.
- why: auth is optional sugar for the agent suite; its absence must degrade gracefully (SKIP),
  never fail the env boot or leak the credential through harness logs.
- status: implemented (env._injectClaudeAuth best-effort returns false; reset proceeds)

### L2.LIFE.019 — EDGE: SIGINT mid-run closes the BridgeHub + Hub child (trap cleanup)
- layer: L2
- scenarios: [base]
- precondition: a run in progress with envs booted.
- action: deliver SIGINT/SIGTERM to `run.mjs`.
- expected: the `onSignal` trap runs `hub.close()` + `stopFleetHub()` (`fleetHub.child.kill()`)
  and `process.exit(130)`; the per-scenario `finally` may not run, but the BridgeHub WS and the
  spawned Hub die.
- assert: after the signal, port :51778 is free (BridgeHub closed) and the spawned `fleet-hub`
  child is reaped. (Container cleanup of in-flight envs is best-effort — documented gap.)
- why: a Ctrl-C must not leave the bridge port held or the Hub orphaned (§8: always free :51778
  before a run; a held port wedges the next run). Honest gap: in-flight containers may survive
  a hard signal — see LIFE.020.
- status: partial (run.mjs SIGINT/SIGTERM trap closes hub + kills fleetHub; in-flight CONTAINER
  cleanup on signal is NOT guaranteed — gap)

### L2.LIFE.020 — EDGE: no container/proc orphans survive a full matrix run (DoD gate)
- layer: L2
- scenarios: [base, no-network, crash-boot, mem-capped, cpu-capped]
- precondition: a complete `node run.mjs` over the matrix has exited.
- action: enumerate residual Fleet artifacts after the process returns.
- expected: zero containers named `fleet-eval-*`; the BridgeHub port :51778 free; no leftover
  chromium processes; the spawned `fleet-hub` (if run.mjs started it) killed.
- assert: `docker ps -a --filter name=fleet-eval- -q` empty; `lsof -ti tcp:51778` empty;
  `pgrep -f 'fleet-eval'` empty; if `fleetHub.child` was spawned, it's not in `ps`.
- machine-state: post-run docker container count == pre-run.
- why: the suite's headline DoD (§9): "leaves zero orphan containers/images/browsers." This is
  the aggregate guard over all the per-env close() calls + the trap; one leaked failure-scenario
  env would fail it.
- status: TODO (no post-run sweep assertion; close()/finally/trap implement the mechanism but
  nothing verifies the aggregate end-state)
