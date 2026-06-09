# L1.EXT — Extensions: installed list, activation events, the fleet-bridge itself + its caps

Extensions are observed through the bridge `extensions` query, which returns
`vscode.extensions.all.map(e => ({ id: e.id, active: e.isActive }))` — the full
installed set with each one's live activation state. The bridge ALSO advertises its
own feature surface in the `hello` frame as `caps:string[]` (the `CAPS` const in
`packages/fleet-bridge/src/extension.ts`), which the harness reads to gate behaviours
via `needs[]`. This area covers both: the generic extension surface AND the
self-referential fact that the test harness is itself driven by an extension that
must be installed, activated, and capability-honest.

> **THE GOTCHA (carry forward, PLAN §8).** The bridge needs
> `extensionKind:["workspace"]` + `capabilities.untrustedWorkspaces` in its manifest
> AND the image must disable `security.workspace.trust` — otherwise the extension
> installs but **silently never activates** (no log). AND the ext-host only starts
> once a workbench client connects (Playwright opens the editor) — pure HTTP won't
> activate it. Several entries below pin exactly these facts so a regression that
> re-breaks activation is caught loudly, not silently.

---

### L1.EXT.001 — Extensions query returns the installed list
- layer: L1
- scenarios: [base, small-repo]
- isolation: shared
- needs: [extensions]
- precondition: workbench booted, ext-host online (Playwright opened the editor)
- action: bridge `extensions {}`
- expected: a non-empty array of `{id, active}` entries
- assert: reply `ok:true`; `r.items` is an array with length ≥ 1; every entry has a string `id` and boolean `active`
- why: smoke for the `extensions` query path — proves `vscode.extensions.all` is reachable through the bridge and well-shaped. The prerequisite for every fleet-bridge-presence assertion below; an empty/throwing result here means the ext-host or query handler regressed.
- status: TODO

### L1.EXT.010 — fleet-bridge is present in the installed list
- layer: L1
- scenarios: [base, small-repo]
- isolation: shared
- needs: [extensions]
- precondition: image built with the fleet-bridge .vsix installed
- action: bridge `extensions {}`
- expected: an entry whose `id` matches the fleet-bridge publisher.name
- assert: `r.items.some(e => /fleet[-.]?bridge/i.test(e.id))` (exact id = the .vsix's `<publisher>.fleet-bridge`)
- why: self-referential presence check — if the bridge isn't even listed, the image's .vsix install step broke. Distinct from EXT.011 (listed-but-inactive is the silent-trust-failure mode). This is the cheapest proof the harness's own driver shipped into the image.
- status: TODO

### L1.EXT.011 — fleet-bridge is ACTIVE (the silent-trust-failure guard)
- layer: L1
- scenarios: [base, small-repo]
- isolation: shared
- needs: [extensions]
- precondition: image disables `security.workspace.trust`; manifest has `extensionKind:["workspace"]` + untrustedWorkspaces; a workbench client (Playwright) has connected
- action: bridge `extensions {}`
- expected: the fleet-bridge entry has `active === true`
- assert: the matched fleet-bridge entry's `active === true`
- why: THE GOTCHA as a test — installed-but-inactive is the EXACT silent failure mode from PLAN §8 (no log, extension present, never activates). If this goes red while EXT.010 is green, the trust/extensionKind/manifest wiring regressed. This is also implicitly true any time ANY behaviour passes (they all go through the bridge), but an explicit assertion makes the diagnosis unambiguous.
- status: TODO

### L1.EXT.012 — fleet-bridge active is a precondition the harness already relies on
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [query]
- precondition: harness reset() completed (which waits on the bridge hello)
- action: bridge `query {}` (the basic Snapshot round-trip)
- expected: a valid Snapshot returns, implying the bridge activated and is serving
- assert: `query` reply `ok:true` with a `data` object containing `terminalCount` (number) — the round-trip only succeeds if the bridge activated
- why: documents that EVERY existing behaviour transitively proves fleet-bridge activation (the `query`/`command` round-trip can't complete otherwise). This entry names that invariant explicitly so the spec doesn't pretend activation is untested — it's covered by the whole suite's existence.
- status: implemented (transitively — any green behaviour via `env.observe`/`env.act`, e.g. `palette.open`)

### L1.EXT.020 — Bridge `hello` advertises the expected capability set
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [query]
- precondition: bridge connected (the harness captured the `hello` frame on connect)
- action: inspect the recorded `hello.caps` from the bridge's registration frame
- expected: caps superset includes the frozen CAPS list — `command, query, openFile, typeText, termSend, writeFile, saveAll, closeEditor, fileContent, terminalText, diagnostics, openEditors, setting, extensions, editorText, selection`
- assert: every token in the §3.3 contract list is present in `hello.caps`; `env.supports(cap)` returns true for each
- why: the capability handshake is what gates `needs[]` skips — if caps drift (e.g. a Track-E refactor drops a token), behaviours silently SKIP instead of running, hiding regressions. This pins the advertised set against the CAPS const so a missing cap is a loud failure, not a silent skip.
- status: TODO

### L1.EXT.021 — A behaviour needing an unadvertised cap SKIPS cleanly (not fails)
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [query]
- precondition: a synthetic behaviour declaring `needs:["nonexistent.cap"]`
- action: run the matrix including that behaviour
- expected: the runner records it as SKIPPED, not FAILED
- assert: the result row for that behaviour has status `skipped` (the runner consults `env.supports(cap)` and skips when false)
- why: EDGE (capability gating) — proves the skip-not-fail contract (PLAN §5: "a partial suite is always green via skips"). Guards against a runner regression that turns a missing cap into a hard failure, which would make incremental Track-E work red the whole suite.
- status: TODO

### L1.EXT.030 — Bridge stays dormant (no hello) when FLEET_BRIDGE_URL is unset
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: []
- precondition: container started WITHOUT `FLEET_BRIDGE_URL`/`FLEET_SERVER_ID` (edge image/env)
- action: open the editor (Playwright) and watch for a `hello` on the harness WS server
- expected: NO `hello` frame arrives (extension.ts early-returns "stay dormant" when url/serverId absent); editor still usable
- assert: harness bridge-hub receives no registration within the bounded boot wait; code-server still serves `302/200`
- why: EDGE (missing precondition) — pins the "pure pass-through, never intrusive" contract from extension.ts (`if (!url || !serverId) return`). Guards against the bridge dialing a garbage URL or crashing the ext-host when Fleet isn't driving it. The harness must record "no bridge" cleanly, not hang waiting for hello.
- status: TODO

### L1.EXT.031 — Bridge reconnects after the harness WS server drops
- layer: L1
- scenarios: [base]
- isolation: fresh
- needs: [query]
- precondition: bridge connected and serving queries
- action: close the harness-side WS, then re-open the listener; observe the bridge re-register
- expected: the bridge re-sends `hello` (extension.ts `reconnect()` retries every 1000ms on close) and resumes answering queries
- assert: a second `hello` frame arrives after the listener re-opens; a post-reconnect `query` returns `ok:true`
- why: EDGE (failure/recovery) — proves the bridge's reconnect loop (the `ws.on("close", reconnect)` path), critical because a harness restart or transient drop must not permanently mute an env. Guards the retry timer wiring.
- status: TODO

### L1.EXT.040 — Open the Extensions view command resolves (UI surface)
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [command]
- precondition: workbench booted
- action: executeCommand "workbench.view.extensions"
- expected: command resolves ok (Extensions side-bar view focused)
- assert: env.act ok (cross-ref 16-views VIEW.043 for the no-folder edge)
- why: covers the View-menu Extensions id — the human-facing surface for the same list the `extensions` query reads programmatically. Pairs the UI command with the query so both paths to "what's installed" are covered.
- status: TODO

### L1.EXT.050 — Activation-event extension activates on its trigger (e.g. language ext on file open)
- layer: L1
- scenarios: [python, node, rust]
- isolation: fresh
- needs: [extensions, openFile]
- precondition: a `+lang` image whose language extension declares `onLanguage:<lang>` activation; that extension shows `active:false` before any matching file opens
- action: bridge `openFile` a `.py`/`.js`/`.rs` file matching the extension's activation event
- expected: the language extension transitions `active:false → active:true`
- assert: `extensions {}` before shows the lang ext `active:false`; after the openFile it shows `active:true`
- why: proves activation EVENTS fire on their real trigger (not just at boot) — the `onLanguage` lifecycle. Uses a real `+lang` image (Track G) so the activation is genuine; gates on `scenarios:[python,node,rust]` so base/minimal images SKIP. Distinguishes lazy activation from the eagerly-activated bridge.
- status: TODO

### L1.EXT.051 — Language extension stays dormant until its activation event (no premature activation)
- layer: L1
- scenarios: [python, node, rust]
- isolation: fresh
- needs: [extensions]
- precondition: a `+lang` image, workbench booted, NO matching file opened yet
- action: bridge `extensions {}` immediately after boot (before opening any lang file)
- expected: the `onLanguage` extension reports `active:false`
- assert: the lang ext entry's `active === false` at boot
- why: EDGE (negative / pre-trigger) — the necessary BEFORE half of EXT.050: if the extension were eagerly active at boot the activation-event test would be vacuous. Pins that lazy extensions don't activate prematurely (which would also waste boot mem — cross-ref Track-D machine Δ).
- status: TODO

### L1.EXT.060 — Extensions query shape is stable when zero workspace extensions are active
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [extensions]
- precondition: base image (only built-in + fleet-bridge), no `+lang` extensions
- action: bridge `extensions {}`
- expected: array returns; contains built-in extensions (always `active` varies) plus the fleet-bridge entry; never throws on a minimal set
- assert: reply `ok:true`; `r.items` array length ≥ 1; the fleet-bridge id present
- why: EDGE (minimal env) — guards the query against a sparse extension set (no language servers installed); proves the list is well-formed even when the only Fleet-relevant entry is the bridge itself. Complements EXT.001 (which may run under repo scenarios with more extensions).
- status: TODO

### L1.EXT.070 — fleet-bridge log file records activation (out-of-band evidence)
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [extensions]
- precondition: bridge activated (serving queries)
- action: `exec cat /tmp/fleet-mux/bridge-<FLEET_SERVER_ID>.log` in the container
- expected: the log contains an `activate: url=... serverId=...` line and a `ws open → hello` line (extension.ts `log()` writes these)
- assert: `env.exec` of the log file contains `"activate:"` and `"ws open → hello"`
- why: cross-checks the in-VS-Code activation state (EXT.011) against the extension's OWN on-disk activation log — an independent witness. If the `extensions` query says active but this log is empty (or vice-versa), the discrepancy localizes the bug (query plumbing vs real activation). The log path is deterministic from `FLEET_SERVER_ID`.
- status: TODO

### L1.EXT.071 — Bridge log records a forwarded command (command path evidence)
- layer: L1
- scenarios: [base]
- isolation: shared
- needs: [command]
- precondition: bridge active
- action: executeCommand "workbench.action.showCommands", then `exec cat` the bridge log
- expected: the log shows `command recv: workbench.action.showCommands` followed by `command ok: workbench.action.showCommands` (extension.ts logs both recv and result)
- assert: `env.exec` of the log contains both the `command recv:` and `command ok:` lines for that id
- why: EDGE (out-of-band command tracing) — proves the command handler's logging witnesses both receipt AND successful execution, the on-disk counterpart to the `ok:true` reply. Useful for debugging a behaviour that times out: if `recv` is present but `ok` is not, the command hung inside VS Code, not in transport.
- status: TODO
