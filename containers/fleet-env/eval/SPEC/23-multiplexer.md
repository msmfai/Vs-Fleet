# L2.MUX — The multiplexer (rail · bridge registration · native-menu forward · embed · switch)

The host-side multiplexer is `fleet-host` (Tauri): ONE window = the Discord-style
**rail** webview (`mux::RAIL`, `index.html`, Fleet IPC) + persistent **editor
surfaces** (one child webview per live server, external code-server origin, no IPC).
`mux::EDITOR` is only the blank placeholder / rollback singleton. The rail's server
list is the **supervisor** (Fleet-spawned, `spawn::ServerSupervisor::servers()`)
chained with the **push-driven registry** (`bridge::BridgeRegistry::servers()`),
deduped by id in `mux::get_servers`. Servers appear by phoning home (`hello` →
`bridge.rs` `handle_conn`) or immediately through the supervisor when Fleet spawned
them. The native menu (`mux::build_menu`) forwards real VS Code command ids
(`cmd:<id>`) through the registry to the **active** server's bridge
(`registry.send_command`). Switching (`mux::select`) shows the target server's
persistent editor webview, creating/navigating it on first selection, and hides the
previous editor without navigating it away.

These exercise host-side Rust + the Tauri window, which the current container eval
harness (which only drives in-container bridges over `:51778`) does **not** start. So
almost everything here is `TODO` — it needs a new L2 harness lane that boots
`fleet-host` (e.g. `FLEET_AUTOSPAWN=n`, `FLEET_SPAWN_MODE=container`) and a Tauri
WebDriver / `tauri::test` mock-runtime hook to read the rail DOM + drive menu events.
A few assertions overlap the in-container bridge already wired by the eval harness
(the `hello` frame, command-forward round-trip) and are marked `partial`.

---

### L2.MUX.001 — A phoned-home bridge registers and appears in the rail server list
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: `fleet-host` running with the bridge WS server bound on `:51778`
  (`BRIDGE_PORT`); zero servers registered (`registry.servers()` == `[]`); no servers
  spawned (`supervisor.servers()` == `[]`).
- action: launch ONE container (`docker run fleet-env` with `FLEET_SERVER_ID=env-1`,
  `FLEET_BRIDGE_PORT=51778`, `FLEET_HOST_ADDR=host.docker.internal`) so its
  `fleet-bridge` dials `ws://host:51778` and sends its first frame
  `{type:"hello", server_id:"env-1", url, label, caps}`.
- expected: the registry holds exactly one `Conn` keyed `"env-1"`; `mux::get_servers`
  returns one `Server{id:"env-1", label, url}`; the rail renders one row.
- assert: `get_servers` (Tauri `invoke`) `.len() == 1` and `[0].id == "env-1"`; the
  `SERVERS_CHANGED` event was emitted exactly once with a 1-element payload (the
  `app.emit(SERVERS_CHANGED, registry.servers())` after `register`); rail DOM has one
  server tab with `data-server-id="env-1"`.
- machine-state: +1 WS connection accepted on `:51778`; +1 container.
- edges: see MUX.002 (empty), MUX.003 (drop → vanish), MUX.004 (malformed hello).
- why: registration IS the rail — a server that phones home must become a switchable
  tab with zero static config; guards the push-only invariant ("servers PUSH, Fleet
  never pulls") against any refactor that reintroduces a pulled/static list.
- status: partial(in-container bridge `hello` is exercised by `bridgeHub.waitFor` in
  the eval harness, but `fleet-host`'s `bridge::handle_conn` registry + `get_servers`
  + rail render are not — needs host-harness)

### L2.MUX.002 — Empty rail: no servers registered shows an empty switchable list
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: `fleet-host` just built its window; no container launched; registry
  and supervisor both empty.
- action: none (observe initial state); call `get_servers`.
- expected: `get_servers` returns `[]`; the editor surface is `about:blank` (the
  `EDITOR` webview built with `WebviewUrl::External("about:blank")`); `selected_server`
  returns `None`.
- assert: `get_servers().len() == 0`; `selected_server()` == `None`; the editor
  webview's current URL == `about:blank` (no navigate has fired).
- why: the window must come up stable with no servers (the cold-start case) and not
  crash or auto-navigate; the "awaiting registrations" baseline.
- status: TODO

### L2.MUX.003 — A dropped bridge deregisters and the server vanishes from the rail
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: one container registered (MUX.001 reached); `get_servers().len()==1`.
- action: `docker rm -f` the container (or kill its `fleet-bridge`) so the WS read half
  yields `Close`/`None` and `handle_conn` falls out of its `select!` loop.
- expected: `registry.unregister("env-1")` runs; `get_servers()` returns `[]`; a second
  `SERVERS_CHANGED` is emitted with an empty payload.
- assert: poll `get_servers()` until `.len()==0` within 5s; the rail row for
  `env-1` is removed from the DOM; `SERVERS_CHANGED` emitted with `[]`.
- machine-state: -1 WS connection; -1 container.
- why: a vanished env must leave the rail (no ghost tabs) — the dual of MUX.001;
  guards the `unregister` + re-emit path on disconnect.
- status: TODO

### L2.MUX.004 — Malformed / pre-hello frames never register a server
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: `fleet-host` bridge listening; registry empty.
- action: open a raw WS to `:51778` and send (a) a non-JSON text frame, then (b) a JSON
  frame with `type:"command"` (not `hello`), then (c) a `hello` missing `server_id`.
- expected: none of (a)/(b)/(c) registers a server — the `handle_conn` loop keeps
  reading until a `hello` *with* a `server_id` arrives (the `loop { match … }` skips
  non-hello and id-less hellos via `continue`).
- assert: `get_servers()` stays `[]` after all three frames; no `SERVERS_CHANGED`
  emitted; only after a valid `{type:"hello",server_id:"x",…}` does a row appear.
- why: the phone-home handshake must be strict — junk on the socket must not create a
  phantom rail entry; guards the registration gate in `handle_conn`.
- status: TODO

### L2.MUX.005 — `hello` url/label/caps are carried into the rail Server
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: bridge listening.
- action: send `{type:"hello", server_id:"env-1", url:"http://127.0.0.1:9001/", label:"alpha"}`.
- expected: the `Conn` stores `url=="http://127.0.0.1:9001/"`, `label=="alpha"`; the
  `Server` surfaced has those fields; a `hello` with NO `label` defaults `label` to the
  `server_id` (the `.unwrap_or(id)` in `handle_conn`).
- assert: `get_servers()[0].label == "alpha"` and `.url == "http://127.0.0.1:9001/"`;
  a second connection `{server_id:"env-2"}` (no label) yields `label=="env-2"`.
- why: the rail label + the embeddable URL come straight from the env's own hello (the
  env knows its host-reachable URL); guards the field extraction + label default.
- status: TODO

### L2.MUX.006 — get_servers merges supervisor + registry, deduped by id, id-sorted
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: one Fleet-spawned server `server-1` (supervisor) AND one externally
  phoned-home server `env-1` (registry).
- action: call `mux::get_servers`.
- expected: both appear, supervisor-first within the dedup but the final list is sorted
  by id (`out.sort_by(a.id.cmp(b.id))`); a server present in BOTH (same id spawned then
  its own bridge phones home) appears ONCE (the `seen.insert(id)` dedup keeps the
  supervisor entry).
- assert: `get_servers()` ids == `["env-1","server-1"]` (sorted); when the spawned
  `server-1`'s bridge also registers under `server-1`, `get_servers()` still has a
  single `server-1` row (supervisor's, since it's iterated first).
- why: a Fleet-spawned server's own phone-home must not double it in the rail; guards
  the dedup + stable ordering that keeps rail rows from churning/duplicating.
- status: TODO

### L2.MUX.007 — Selecting a server creates/shows its persistent editor webview
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: one server `env-1` with `url=http://127.0.0.1:<p>/?folder=…`
  registered; editor webview at `about:blank`; `selected==None`.
- action: invoke `select_server("env-1")` (the rail-click path → `mux::select`).
- expected: `MuxState.selected` becomes `Some("env-1")`; `mux::server_url` resolves the
  url; a child webview labeled from `env-1` is created if missing; that webview
  navigates to the URL; the placeholder `EDITOR` webview is hidden; the rail eval's
  `window.__fleetSyncSelection()` runs.
- assert: `selected_server()` == `"env-1"`; the persistent editor webview's URL ==
  `env-1`'s url (poll until the code-server workbench responds `200`/`302`); the
  rail's selected row has the active marker; the placeholder is not visible.
- machine-state: the embedded code-server's ext-host comes online (the webview is a
  real workbench client — like Playwright in the eval harness).
- edges: see MUX.008 (re-select no-op), MUX.009 (unknown id), MUX.010 (switch
  reattach).
- why: first select attaches one persistent editor client to that server. Later
  switches must preserve that client rather than unload it, which is the Fleet
  version of cmux's "pane stays alive" contract.
- status: TODO

### L2.MUX.008 — Re-selecting the already-loaded server does NOT reload its editor
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: `env-1` selected and loaded (MUX.007); its `EditorEntry.loaded_url`
  is `env-1.url`.
- action: invoke `select_server("env-1")` again.
- expected: `select` updates `selected` (idempotent) but the per-server loaded URL
  guard is false, so `wv.navigate` is NOT called — the webview keeps its live session
  (no reload, no terminal churn).
- assert: instrument/observe that no `navigate` fires on the second select (e.g. the
  embedded workbench's page-load counter is unchanged; the editor's running terminal
  survives — `terminalCount` via the in-container bridge query is unchanged across the
  re-select).
- why: re-clicking the active tab must not nuke the session; guards the per-server
  loaded-URL short-circuit in `select`.
- status: TODO

### L2.MUX.009 — Selecting an unknown server id is a clean no-op (no navigate)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: registry has `env-1`; editor at `about:blank`.
- action: invoke `select_server("does-not-exist")`.
- expected: `select` sets `selected=Some("does-not-exist")` but `server_url` returns
  `None`, so no editor webview is created or navigated; the placeholder remains
  visible; the rail sync eval still runs.
- assert: no `editor:does-not-exist` webview exists; placeholder editor URL stays
  `about:blank`; `selected_server()` == `"does-not-exist"`.
- why: a stale rail click (server dropped between render and click) must not crash or
  blank-navigate; guards the `server_url` `None` path.
- status: TODO

### L2.MUX.010 — Switching away and back preserves both workspace clients
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness, termSend]
- precondition: TWO servers `env-1`,`env-2` registered; `env-1` selected; a terminal
  opened in `env-1`'s embedded workbench running a marker (`echo FLEET_REATTACH; sleep
  600`).
- action: `select_server("env-2")`, then `select_server("env-1")`.
- expected: Fleet hides `env-1`'s persistent editor webview and shows `env-2`'s; on
  return it shows the already-loaded `env-1` webview without navigating it. Both
  bridge generations stay stable through the switch.
- assert: after the round-trip, query `env-1`'s in-container bridge: `terminalCount`
  is the same as before the switch-away and `terminalText` still contains
  `FLEET_REATTACH`; assert the bridge did not deregister/reregister for either
  server; assert full-window screenshots include the rail, selected tab, VS Code tabs,
  and a nonblank editor pane.
- why: this is the multiplexer's reason to exist — switching tabs must feel like
  cmux: in-flight agents/terminals stay alive and switching is instantaneous surface
  visibility work, not a client teardown/reconnect cycle.
- status: TODO

### L2.MUX.011 — Native menu `cmd:<id>` forwards the command to the ACTIVE server only
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness, termSend]
- precondition: TWO servers `env-1`,`env-2` registered, both with a live bridge;
  `env-1` selected (`MuxState.selected == "env-1"`).
- action: fire the menu event `cmd:workbench.action.terminal.new` (the Terminal ▸ New
  Terminal item from `mux.rs` `TERMINAL`), which `main.rs` `on_menu_event` routes to
  `registry.send_command(active, "workbench.action.terminal.new")`.
- expected: a `{type:"command", id:"workbench.action.terminal.new"}` frame is sent to
  `env-1`'s bridge ONLY; `env-2`'s bridge receives nothing.
- assert: `env-1`'s in-container bridge `query.terminalCount` goes +1; `env-2`'s stays
  0; `send_command` logged `sent=true` for `env-1`.
- why: the OS menu must drive whichever server the user is looking at, never a
  background one; guards the active-id lookup + single-target forward in
  `send_command` (the webview can't own the OS menu, so this wiring is load-bearing).
- status: partial(the in-container bridge runs `executeCommand` and `terminalCount`
  +1 is proven by behaviour `terminal.new`; the host `cmd:`→`send_command`→active-only
  routing is not — needs host-harness)

### L2.MUX.012 — Menu command with NO active server is dropped (warn, no crash)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: a server `env-1` registered but `MuxState.selected == None` (nothing
  clicked yet).
- action: fire `cmd:workbench.action.showCommands`.
- expected: `on_menu_event`'s `if let Some(active)` is `None`, so `send_command` is
  never called — the command is dropped silently (no panic); nothing reaches any bridge.
- assert: no frame arrives at `env-1`'s bridge (its `terminalCount`/state unchanged);
  the app does not crash; `selected_server()` stays `None`.
- why: a menu invoked before any tab is selected must no-op, not crash or broadcast;
  guards the active-required gate.
- status: TODO

### L2.MUX.013 — Menu command for a registered-but-disconnected active server is dropped
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: `env-1` was selected, then its bridge dropped (so `registry.servers()`
  no longer holds it) but `MuxState.selected` still reads `"env-1"`.
- action: fire `cmd:workbench.action.terminal.new`.
- expected: `send_command("env-1", …)` finds no `Conn` in the map → the `None` arm logs
  `"no bridge for active server — dropped"`; no send.
- assert: `send_command` logs the warn line for `env-1`; no panic; no frame delivered.
- why: a command targeting a just-departed server must drop gracefully (the select
  state can lag the registry); guards the `map.get(server_id)` `None` arm.
- status: TODO

### L2.MUX.014 — Native clipboard menu items are native (work in the external webview)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: `fleet-host` menu built; a server selected with text selected in the
  embedded editor.
- action: invoke Edit ▸ Copy (the `MItem::Copy` built via `b.copy()` in `build_sub`).
- expected: Copy is a NATIVE menu role (not a forwarded `cmd:` id), so it operates on
  the focused external webview directly — selected text lands on the OS clipboard
  WITHOUT a bridge round-trip.
- assert: after Copy then Paste into a scratch field, the pasted text == the selection;
  no `cmd:` menu event was emitted for Copy (it has no `cmd:` id — `Cut/Copy/Paste`
  build via `b.cut()/.copy()/.paste()`, the `cmd:` ids are only for `MItem::Cmd`).
- why: clipboard must work in the external code-server origin where Fleet IPC is
  absent; the menu deliberately makes Cut/Copy/Paste native rather than forwarded —
  guards that split.
- status: TODO

### L2.MUX.015 — Server ▸ New Server menu item spawns a server and adds a rail tab
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: `get_servers()` count == N (≥0).
- action: fire the menu event `spawn:new` (Server ▸ New Server, `mux.rs` `server_menu`),
  routed by `on_menu_event` to `supervisor.spawn()`.
- expected: a new `Server` is created and pushed to the supervisor; `get_servers()`
  count == N+1; (in container mode it `docker run`s the image, which then phones home).
  See 25-deploy-spawn for the spawn-mode internals.
- assert: `get_servers()` count == N+1 and includes the new `server-<n>` id; a
  `SERVERS_CHANGED` is emitted.
- why: the menu's lifecycle action must create a switchable server; guards the
  `spawn:new` → supervisor wiring (distinct from the rail's switch action).
- status: TODO

### L2.MUX.016 — Server ▸ Close Current closes the selected server, removing its tab
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: a Fleet-spawned `server-1` exists AND is selected
  (`MuxState.selected == "server-1"`).
- action: fire `spawn:close-current` (Server ▸ Close Current Server), routed to
  `supervisor.close(active)`.
- expected: `close("server-1")` returns true; its children are killed (local) or its
  container `docker rm -f`'d (container); `get_servers()` no longer lists it.
- assert: `get_servers()` count drops by 1 and excludes `server-1`; `SERVERS_CHANGED`
  emitted. (Close internals + child/container teardown: see 25-deploy-spawn.)
- edges: `spawn:close-current` with `selected==None` → the `if let Some(active)` is
  None → no-op (no crash, nothing closed).
- why: the menu must close whatever the user is viewing; guards the
  selected→`close` wiring + the no-selection no-op.
- status: TODO

### L2.MUX.017 — Window resize re-tiles rail (fixed width) + editor (fills remainder)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: window at 1320×860; rail at x=0 w=248 (`RAIL_W`); editor at x=248
  w=1072.
- action: resize the window to 1600×900 (fires `WindowEvent::Resized` → `retile`).
- expected: rail stays x=0 w=248 full height; editor repositions to x=248 and its width
  becomes `(w - RAIL_W).max(120)` == 1352; both fill the new height.
- assert: read the two webviews' geometry: `rail.size == (248, 900)`,
  `editor.position == (248,0)`, `editor.size == (1352, 900)`.
- edges: shrink below min: width clamped so editor pane is never < 120 (`.max(120.0)`);
  the window's own `min_inner_size(760,480)` floors it.
- why: the single-editor layout must track the window so the embedded workbench always
  fills the pane; guards `retile`'s arithmetic + the rail-width constant.
- status: TODO

### L2.MUX.018 — Only the rail webview gets Fleet IPC; the editor surface does not
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: window built with `RAIL` (App `index.html`) + `EDITOR` (External url).
- action: from the EDITOR webview's page context, attempt to call a Fleet Tauri command
  (e.g. `window.__TAURI__.core.invoke("get_servers")`).
- expected: the call is unavailable/denied — the editor is a plain external origin with
  no Fleet API injected; only the rail (App webview) can `invoke` Fleet commands.
- assert: in the EDITOR context `window.__TAURI__` is undefined (or `invoke` rejects);
  in the RAIL context `invoke("get_servers")` succeeds.
- why: the embedded third-party code-server must not be able to drive Fleet's own
  commands (trust boundary); guards the App-vs-External webview split.
- status: TODO

### L2.MUX.019 — Concurrent registrations from N envs all land as distinct rail tabs
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: bridge listening; registry empty.
- action: launch N=5 containers simultaneously (`env-1`..`env-5`), each phoning home in
  parallel (each `handle_conn` runs on its own `tokio::spawn`).
- expected: all 5 register under distinct ids (the registry `HashMap` is `Mutex`-guarded
  per insert); `get_servers()` returns 5 id-sorted rows; 5 `SERVERS_CHANGED` emits.
- assert: `get_servers()` ids == `["env-1",…,"env-5"]`; no lost/duplicate ids under the
  concurrent inserts.
- machine-state: 5 WS conns on `:51778`; 5 containers.
- why: Fleet's value is many parallel envs in one window — concurrent phone-home must
  not race the registry; guards the `Mutex<HashMap>` register path under load.
- status: TODO

### L2.MUX.020 — Re-registration under an existing id replaces the connection (no dup)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: `env-1` registered (conn A).
- action: open a SECOND bridge connection that sends `hello` with the same
  `server_id:"env-1"` (e.g. the env's bridge reconnected before the old conn's close
  was processed).
- expected: `register` does `map.insert(id, conn)` — the new `Conn` (tx B) REPLACES the
  old; `get_servers()` still has exactly one `env-1` row; subsequent `send_command`
  goes to conn B.
- assert: `get_servers()` count for `env-1` == 1; a forwarded command appears on conn B,
  not conn A; when conn A later closes it `unregister`s — verify a stale-close doesn't
  remove the live conn B's entry (NOTE: current code's `unregister` removes by id
  unconditionally — flag this as a known hazard if A's close races after B registers).
- why: a bridge reconnect must not double the rail tab nor leave commands going to a
  dead socket; guards `register`'s insert-replace and surfaces the close-race hazard.
- status: TODO
