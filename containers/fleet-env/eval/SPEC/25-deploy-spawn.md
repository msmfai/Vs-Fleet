# L2.SPAWN — Deploy / spawn (supervisor process vs container modes · env wiring · close/GC · claude shim+hooks)

`spawn::ServerSupervisor` is Fleet's process supervisor: `spawn()` selects a launch
path by `FLEET_SPAWN_MODE` and `close()` tears it down. Two modes:

- **local** (default / `FLEET_SPAWN_MODE` unset|≠container): `spawn_local` launches a
  host **code-server** (`--auth none`, shared `--extensions-dir` with the fleet-bridge,
  per-server `--user-data-dir`), a per-server `fleet-reporter --serve` (session id =
  server id) on a `reporter-<id>.sock`, and prepends a **claude shim** dir to the
  code-server's `PATH`. The shim (`install_claude_shim`) wraps the real `claude` with
  `--settings <fleet-hooks.json>` whose hooks relay each lifecycle payload via
  `nc -U` to the reporter socket. Children tracked in `children: HashMap<id,Vec<Child>>`.
- **container** (`FLEET_SPAWN_MODE=container`): `spawn_container` `docker run -d` the
  `fleet-env` image (bridge+reporter+claude baked in), passing `FLEET_SERVER_ID`,
  `FLEET_HOST_ADDR`, `FLEET_BRIDGE_PORT`, `FLEET_HUB_URL`, publishing a free host port to
  the container's `:8080`; the container phones home on its own. Tracked in
  `containers: HashMap<id,name>`. `close` `docker rm -f`'s it.

Both append a `Server{id,label,url}` to `servers` (the rail source for spawned envs) and
return it; the caller emits `SERVERS_CHANGED`. This is host-side Rust — the current eval
harness doesn't boot `fleet-host`, so these are `TODO` pending a host-harness; the
container-mode `docker run` contract MIRRORS `eval/lib/env.mjs` (`_dockerRunCmd`), and
the image's own claude-hooks→reporter path is exercised by `agent.*` behaviours, so a
few are `partial`.

---

### L2.SPAWN.001 — Local-mode spawn launches code-server + reporter + records the server
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: `fleet-host` running, `FLEET_SPAWN_MODE` unset (→ local); supervisor
  counter at n, `servers()` empty.
- action: invoke `spawn_server` (Tauri command → `supervisor.spawn()` → `spawn_local`).
- expected: a code-server child + a `fleet-reporter --serve` child are spawned and
  tracked under id `server-<n>` in `children`; a `Server{id:"server-<n>",label,url}` is
  pushed to `servers()` and returned; the url is
  `http://127.0.0.1:<freeport>/?folder=<tmp>/fleet-mux/ws-server-<n>`.
- assert: `servers()` count +1 with id `server-<n>`; `pgrep -f code-server` and
  `pgrep -f 'fleet-reporter --serve'` each show +1; `<tmp>/fleet-mux/ws-server-<n>/`
  exists with a `server-<n>.md` seed file.
- machine-state: +≥2 processes (code-server + reporter); +1 workspace dir.
- edges: see SPAWN.011 (reporter bin missing), SPAWN.012 (claude absent), SPAWN.020
  (concurrent spawns).
- why: a local spawn must produce a self-registering editor with its agent pipeline
  wired; guards the `spawn_local` child set + the rail Server record.
- status: TODO

### L2.SPAWN.002 — Spawned local code-server phones home and appears in the rail
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: spawn (SPAWN.001) launched code-server with
  `FLEET_BRIDGE_URL=ws://127.0.0.1:51778`, `FLEET_SERVER_ID=server-<n>`,
  `FLEET_SERVER_URL=<url>` in its env.
- action: bring its ext-host online (the fleet-bridge activates on a workbench client
  connect) so the bridge dials `:51778` and sends `hello`.
- expected: the bridge registers under `server-<n>`; because the supervisor ALSO holds
  it, `get_servers` dedups to ONE `server-<n>` row (supervisor entry wins — iterated
  first; see MUX.006).
- assert: `get_servers()` has exactly one `server-<n>`; the bridge registry shows a
  Conn for `server-<n>`; no duplicate rail row.
- why: a Fleet-spawned server still phones home (Fleet never pulls), and that must NOT
  double it; guards the spawn-env wiring (`FLEET_BRIDGE_URL`/`FLEET_SERVER_ID`) + dedup.
- status: TODO

### L2.SPAWN.003 — Per-server user-data-dir is isolated; extensions-dir is shared
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: two local spawns `server-1`, `server-2`.
- action: inspect each code-server's launch args.
- expected: each got a distinct `--user-data-dir <tmp>/fleet-mux/cs-userdata-server-<n>`
  (no collision under concurrency) but the SAME `--extensions-dir` (default
  `<tmp>/fleet-mux/cs-exts` or `FLEET_EDITOR_EXTENSIONS_DIR`) so the fleet-bridge is
  installed once and shared.
- assert: the two processes' `--user-data-dir` paths differ; their `--extensions-dir`
  paths are identical; both resolve the same bridge extension.
- why: concurrent code-servers must not corrupt each other's state dir while still
  sharing one bridge install (the documented spawn invariant); guards the
  per-server-userdata / shared-exts split.
- status: TODO

### L2.SPAWN.004 — Local-mode close kills BOTH children (code-server + reporter)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: `server-1` spawned local (children = [reporter, code-server]).
- action: invoke `close_server("server-1")` (→ `supervisor.close`).
- expected: `close` removes `server-1` from `servers`, then drains its `children` vec —
  `kill()` + `wait()` on EACH child; returns true; the server leaves the rail.
- assert: after close, `pgrep` shows the code-server AND reporter PIDs gone (both, not
  just one); `servers()` no longer lists `server-1`; `SERVERS_CHANGED` emitted; a second
  `close("server-1")` returns false (already gone).
- machine-state: -≥2 processes; no orphaned reporter holding the socket.
- why: closing a tab must reap its ENTIRE pipeline — a leaked reporter would keep a dead
  session phoning home; guards the `children` drain (all entries, with `wait` to reap
  zombies).
- status: TODO

### L2.SPAWN.005 — Container-mode spawn `docker run`s the image with the env wiring
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness, docker]
- precondition: `FLEET_SPAWN_MODE=container`, `FLEET_BRIDGE_ADDR=0.0.0.0` (so the
  container can reach the bridge), `fleet-env:latest` present.
- action: invoke `spawn_server` (→ `spawn_container`).
- expected: a `docker run -d --name fleet-server-<n>` runs the image with
  `-e FLEET_SERVER_ID=server-<n>`, `-e FLEET_SERVER_LABEL=server-<n>`,
  `-e FLEET_HOST_ADDR=host.docker.internal`, `-e FLEET_BRIDGE_PORT=51778`,
  `-e FLEET_HUB_URL=<hub>`, `-p <freeport>:8080`; the server is tracked in `containers`
  (NOT `children`) and pushed to `servers()`.
- assert: `docker ps` shows `fleet-server-<n>` running with `:8080` published to
  `<freeport>`; `servers()` has `server-<n>`; the container's env (`docker exec … env`)
  has the five `FLEET_*` vars; `containers` maps `server-<n> → fleet-server-<n>`.
- machine-state: +1 container; +1 published host port.
- edges: see SPAWN.013 (stale name removed first), SPAWN.014 (docker run fails),
  SPAWN.015 (inspect fallback url).
- why: container mode must launch a self-phoning env mirroring the eval harness contract;
  guards the exact `docker run` argv + the `containers` (not `children`) bookkeeping.
- status: partial(the same `docker run` env-wiring is exercised by `eval/lib/env.mjs`
  `_dockerRunCmd` + the env phones home in the harness; `spawn_container`'s host-side
  argv + `containers` map is not — needs host-harness)

### L2.SPAWN.006 — Container url comes from `docker inspect`'s published HostPort
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness, docker]
- precondition: a container spawned (SPAWN.005).
- action: observe the `Server.url` produced.
- expected: `inspect_url` runs `docker inspect -f '{{(index (index
  .NetworkSettings.Ports "8080/tcp") 0).HostPort}}'` and the url is
  `http://127.0.0.1:<that-host-port>/`; if inspect fails/empty it FALLS BACK to the
  port we asked for (`format!("http://127.0.0.1:{port}/")`).
- assert: `Server.url` host-port == the `docker inspect` HostPort for `8080/tcp`; a
  forced inspect failure (e.g. wrong name) yields the fallback url, not a crash.
- why: the embeddable url must be the ACTUAL host-bound port docker chose (colima can
  remap), with a safe fallback; guards `inspect_url` parsing + the `unwrap_or_else`.
- status: TODO

### L2.SPAWN.007 — Container-mode close `docker rm -f`s the container, not host kills
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness, docker]
- precondition: container `server-1` spawned (mapped in `containers`).
- action: invoke `close_server("server-1")`.
- expected: `close` removes it from `servers`, finds it in `containers` (the
  `containers.remove(id)` branch runs FIRST, returning early), and
  `docker rm -f fleet-server-1`; it does NOT touch `children`.
- assert: `docker ps -a` no longer shows `fleet-server-1`; `servers()` excludes
  `server-1`; the published host port is freed; no host code-server/reporter was killed.
- machine-state: -1 container.
- why: the two modes must tear down differently — a container is removed, not
  process-killed; guards the container-first branch in `close` and clean container GC.
- status: TODO

### L2.SPAWN.008 — claude shim wraps the real claude with the hooks settings file
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: local spawn on a host that HAS a real `claude` on PATH (or
  `FLEET_CLAUDE_BIN` set); `find_real_claude` resolves it (skipping the shim dir).
- action: `spawn_local` runs `install_claude_shim(shim_dir, reporter_socket)`.
- expected: `<shim_dir>/claude` is a 0755 `#!/bin/sh` script that `exec`s the REAL
  claude with `--settings <shim_dir>/fleet-hooks.json "$@"`; `fleet-hooks.json` exists
  with hooks for SessionStart/UserPromptSubmit/PreToolUse/PostToolUse/Stop/SessionEnd,
  each relaying `printf 'claude %s\n' "$(cat|tr -d '\r\n')" | nc -U <socket> || true`.
- assert: read `<shim_dir>/claude`: mode 0755, contains `exec '<real>' --settings`; the
  real path is NOT the shim itself (`find_real_claude` excludes `shim_dir`);
  `fleet-hooks.json` parses and has the six hook event keys.
- why: the shim is what lights a spawned server's tab with zero user setup — it must
  wrap (never recurse into) the real claude and install the relaying hooks; guards
  `install_claude_shim` + `find_real_claude`'s self-exclusion.
- status: TODO

### L2.SPAWN.009 — Shim dir is prepended to the code-server PATH so terminal claude is wrapped
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: local spawn where the shim installed successfully.
- action: inspect the spawned code-server's `PATH` env.
- expected: `PATH == "<shim_dir>:<existing PATH>"` (shim FIRST), so `claude` typed in
  the server's integrated terminal resolves to the shim, which relays hooks to THIS
  server's reporter socket.
- assert: the code-server child's `PATH` starts with `<tmp>/fleet-mux/shim-server-<n>`;
  `which claude` in its terminal resolves to the shim path.
- edges: if the shim did NOT install (no real claude), `PATH` is the unchanged
  existing PATH (the `(_, Ok(p)) => p` arm) — claude still runs, just unwrapped (no
  hooks); no crash.
- why: the wrapping only works if the shim shadows the real claude on PATH; guards the
  prepend ordering and the graceful no-shim fallback.
- status: TODO

### L2.SPAWN.010 — Shim hooks relay drives the spawned server's reporter → Hub session
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: a local-spawned `server-1` with shim + a live `fleet-reporter --serve`
  on `reporter-server-1.sock`; the Hub running.
- action: run `claude -p "hi"` in `server-1`'s terminal (resolves to the shim).
- expected: the shim's hooks relay UserPromptSubmit/Stop to the reporter socket → the
  reporter forwards UpsertRun(working→idle) to the Hub under session `server-1`; the
  rail row for `server-1` advances working→idle.
- assert: `fleet ls --once` shows the `server-1` session go active then settle;
  `get_inbox` reflects it. (This is the host-spawned analogue of FLOW.001's container
  path.)
- why: ties the spawn pipeline to the state flow — a spawned server's tab must light up
  on its own; guards shim→socket→reporter→Hub for the LOCAL spawn path specifically.
- status: TODO

### L2.SPAWN.011 — Reporter binary missing: spawn still succeeds, sans agent state
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: `FLEET_REPORTER_BIN` points at a nonexistent path (or no
  `fleet-reporter` on PATH).
- action: `spawn_local`.
- expected: `spawn_reporter` errors; the `match` logs `warn "reporter not started (no
  agent state)"` and does NOT push a reporter child; the code-server still launches and
  the `Server` is still recorded (spawn does not fail).
- assert: `spawn_server` returns Ok with a valid `server-<n>`; `children` for it has
  only the code-server (no reporter); the rail row appears (editor works) but no agent
  state will flow.
- why: a missing reporter must degrade (editor without agent badges), never abort the
  whole spawn; guards the reporter-spawn error tolerance.
- status: TODO

### L2.SPAWN.012 — Real claude absent: shim is skipped, spawn proceeds unwrapped
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: no `claude` on PATH and `FLEET_CLAUDE_BIN` unset/non-file.
- action: `spawn_local`.
- expected: `install_claude_shim` returns Err (NotFound "real `claude` not on PATH"),
  mapped to a `warn` + `shim_path = None`; PATH is left unchanged; code-server still
  launches; `Server` recorded.
- assert: `spawn_server` returns Ok; the code-server child's PATH has NO shim dir
  prepended; no `shim-server-<n>` dir is required for boot.
- why: most machines spawning won't have claude — that must not block spawning an
  editor; guards the `.ok()` shim-optional path + the PATH fallback arms.
- status: TODO

### L2.SPAWN.013 — Container spawn removes a stale same-named container first
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness, docker]
- precondition: `FLEET_SPAWN_MODE=container`; a leftover container named
  `fleet-server-<n>` exists from a prior crashed run.
- action: `spawn_container` (which counter would reuse the name, e.g. after a restart).
- expected: the best-effort `docker rm -f <name>` runs BEFORE `docker run`, so the stale
  container can't collide with the new `--name`; the new container starts clean.
- assert: only ONE `fleet-server-<n>` exists after spawn (the new one — different
  container id than the stale); `docker run` did not fail with a name conflict.
- why: name collisions from a previous run must self-heal; guards the pre-run
  `rm -f` cleanup.
- status: TODO

### L2.SPAWN.014 — `docker run` failure surfaces as an Err, no phantom rail row
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness, docker]
- precondition: `FLEET_SPAWN_MODE=container`, `FLEET_SPAWN_IMAGE=does-not-exist:nope`.
- action: `spawn_container`.
- expected: `docker run` exits non-zero → `spawn_container` returns
  `Err("`docker run` failed for … (image …)")`; NO server is pushed to `servers`, NO
  entry added to `containers`.
- assert: `spawn_server` returns Err (Tauri command surfaces the string to the rail);
  `get_servers()` count unchanged; no `fleet-server-<n>` lingers; no `SERVERS_CHANGED`
  with a phantom row.
- why: a failed deploy must not leave a dead/ghost tab; guards the `status.success()`
  check and that bookkeeping happens only after a successful run.
- status: TODO

### L2.SPAWN.015 — Close is idempotent / safe for an unknown id
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: supervisor with no server `ghost`.
- action: invoke `close_server("ghost")`.
- expected: `close` finds `ghost` in neither `containers` nor `children` → returns false;
  no process killed, no `docker rm` run, no panic; the (unconditional) `servers.retain`
  is a harmless no-op.
- assert: `close("ghost")` == false; `get_servers()` unchanged; no docker/process side
  effects; the rail is unaffected.
- why: a double-close or a stale rail click must be inert; guards the both-maps-miss
  path returning false.
- status: TODO

### L2.SPAWN.016 — FLEET_AUTOSPAWN boots N servers on startup (test-harness hook)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: `fleet-host` launched with `FLEET_AUTOSPAWN=3`.
- action: start the app (the `setup` hook in `main.rs` loops `sup.spawn()` N times).
- expected: 3 servers are spawned before the window finishes setup; `get_servers()`
  returns 3 (mode-dependent: 3 local pipelines or 3 containers).
- assert: `get_servers()` count == 3 after boot; with `FLEET_SPAWN_MODE=container`,
  `docker ps` shows 3 `fleet-server-*`; an invalid `FLEET_AUTOSPAWN=abc` (parse fails)
  spawns 0 (the `and_then(parse)` yields None) — clean no-op.
- why: the integration test must drive Fleet without clicking; guards the autospawn
  loop + the parse-guard for a bad value.
- status: TODO

### L2.SPAWN.017 — Spawn mode selection: only `container` opts into Docker; else local
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: run three spawns with `FLEET_SPAWN_MODE` = `container`, `local`, unset.
- action: `spawn()` under each.
- expected: `container` → `spawn_container` (a container appears in `containers`);
  `local` AND unset → `spawn_local` (children appear, no container) — the `match
  spawn_mode()` maps `Ok("container")` to Container and everything else (incl. unset,
  `"process"`, garbage) to Local.
- assert: `container` run has a `docker ps` entry + a `containers` map entry;
  `local`/unset runs have code-server+reporter PIDs + a `children` entry and NO
  container.
- why: the deploy target must be a single explicit switch with a safe default; guards
  `spawn_mode()`'s exact match (no accidental container deploys).
- status: TODO

### L2.SPAWN.018 — Free-port allocation avoids collisions across concurrent local spawns
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: spawn several local servers in quick succession.
- action: spawn 4 local servers.
- expected: each `free_port()` binds `127.0.0.1:0` and reads the OS-assigned port, so
  each code-server `--bind-addr` is distinct; all 4 code-servers serve on different
  ports (the small TOCTOU window is acceptable for local spawn, per the doc comment).
- assert: the 4 `Server.url` ports are pairwise distinct; all 4 code-servers respond
  `200`/`302` on their own port.
- why: concurrent spawns must not fight over a port (no fixed `8200+i`); guards
  `free_port` producing usable distinct ports under burst spawning.
- status: TODO

### L2.SPAWN.019 — Spawned-server logs are written for debuggability
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: a local spawn.
- action: `spawn_local` (which calls `log_files("cs-<id>")` + `log_files("reporter-<id>")`).
- expected: `<tmp>/fleet-mux/cs-server-<n>.log` and `reporter-server-<n>.log` are
  created and receive the children's stdout/stderr (falls back to null only if the file
  can't be created).
- assert: both log files exist after spawn and grow as the children run (non-empty once
  code-server prints its banner / the reporter logs "listening").
- why: a wedged spawn must be diagnosable from disk without attaching a debugger; guards
  the `log_files` redirection wiring.
- status: TODO

### L2.SPAWN.020 — Concurrent spawns get distinct ids + isolated bookkeeping (no races)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: supervisor counter at n.
- action: invoke `spawn()` from several threads at once (the rail + autospawn + menu can
  race).
- expected: the `AtomicU64` counter hands each a unique `server-<n>`; each `children`/
  `containers` insert and `servers.push` is `Mutex`-guarded — no two spawns get the same
  id, no lost entries.
- assert: K concurrent spawns yield K pairwise-distinct ids; `servers()` has exactly K
  entries; `children`/`containers` has K keys; no panic / poisoned mutex.
- why: spawning is concurrent (rail clicks, menu, autospawn) — the id counter + maps
  must be race-free; guards the `AtomicU64::fetch_add` + per-map `Mutex` discipline.
- status: TODO

### L2.SPAWN.021 — Workspace dir + seed file are created for a local spawn
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: local spawn.
- action: `spawn_local`.
- expected: `<tmp>/fleet-mux/ws-server-<n>/` is created and seeded with
  `server-<n>.md` (the "Run `claude` in the terminal — this tab will light up." note);
  the code-server opens that folder (`?folder=<ws>` in the url).
- assert: the workspace dir + `server-<n>.md` exist; the spawned url's `folder` query ==
  that dir; the embedded editor opens it as the workspace root.
- edges: if the dir create / write fails (`let _ =`), spawn proceeds anyway (best-effort)
  — code-server just opens an empty/absent folder, no crash.
- why: a spawned server needs a real workspace to open (and a hint for the user); guards
  the workspace seeding + url folder wiring + its best-effort tolerance.
- status: TODO
