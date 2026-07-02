# VS Fleet code-smell + refactor audit — 2026-06

> **Status (branch `refactor/audit-2026-06`):**
> **TIER 3 + 4.2 APPLIED (CI-verified):** dead deps, `ws_port`→`DEFAULT_WS_PORT`,
> `sort_tab_refs` ptr_arg, dead `removeRow()` + broken `npm check`, `cargo audit` gate.
> **TIER 1 COMPLETE — FULLY GREEN IN CI** (coverage gate: zero uncovered lines, both
> workspaces; all non-Windows CI jobs pass). CI-under-load caught + we fixed a real
> concurrency regression (hub subscribe-atomicity) the first T1.2 introduced.
> Applied in reviewable commits, each fix has a fails-before regression test:
> - T1.2 hub SQLite I/O → `spawn_blocking` + RwLock snapshot cache · T1.3 mute/solo
>   append-first w/ rollback · T1.6 empty-rollup→Idle + clear unread · env-race `ENV_LOCK`.
> - T1.4 host spawn → `#[tauri::command(async)]`+`spawn_blocking` · T1.5 accept-loop backoff
>   (no die on transient err) · T1.8 deleted dead dynamic-menu machinery (kept live `cmd:`).
> - T1.1 reporter REAL detection: Stop→Done, Codex dead via timeout, killed phantom-field +
>   approval-response paths, wired transcript corroboration into serve.rs, count parse-drops.
> - T1.7 node `__TAURI__` boot guard.
> - **DECISIONS FLAGGED FOR REVIEW:** (a) reporter chose real `Stop→Done` (per-turn) — flip to
>   `Idle` if you want conservative-idle (costs `Done` reachability). (b) native-menu
>   per-server switching was dead code, now removed (rail switching is the path).
> **TIER 2 — NEXT (staged):** jiff date seam (T2.1, decided), reporter 4-machine dedup (T2.2),
> hub delta-vocab (T2.3), node UI boilerplate (T2.4), smaller dup seams (T2.5), tab_transition.
> **STILL DEFERRED (needs node or entangled):** T4.1 commit JS lockfiles, confidence.rs
> delete-vs-wire, Tier 5 test-smell pass.


Fresh-model pass, six parallel surface audits, every non-trivial assumption web-verified
against live 2026 sources. Verdicts are CONFIRMED / REFUTED / UNVERIFIED per finding.
Ranked by value × confidence within tiers.

## What the web-verification *cleared* (skeptic wins — do NOT re-flag)
- **macOS focus design is correct** — `activateIgnoringOtherApps` deprecation (macOS 14, cooperative
  activation), the "can't activate on timer/network events" constraint, and Wayland's
  `xdg_activation_v1` "can only receive focus" all CONFIRMED; the AppleScript/NSWorkspace path +
  `confirmation_possible:false` on Wayland are the *right* choices. (host-core)
- **SQLite persistence can't corrupt** — WAL + `synchronous=NORMAL` loses at most the last txn on
  crash, never corrupts. (hub)
- **WS server + broadcast backpressure** — per-conn tasks + bounded broadcast → `Lagged`; a slow
  face can't stall the hub. tokio-tungstenite 0.29 idiom is current. (hub)
- **Deps are current + non-vulnerable** — tokio 1.52, tungstenite 0.29, serde 1.0.228, schemars 1.2,
  rusqlite 0.40, clap 4.6, ws 8.21 all latest; RUSTSEC clean (tungstenite/ring/tauri advisories all
  predate pins). Only `tauri` lock trails 3 patches. (cli/deps)
- **`Message::Text(x.into())`, `set_dock_visibility`, clipboard-with-fallback, reconnect backoff
  guards, generation-guarded async races** — all CONFIRMED correct idioms. (host, node)

---

## TIER 1 — Correctness (highest value; behavior is wrong against the live contract)

### T1.1 Agent-state inference is broken against the CURRENT Claude Code / Codex hook schemas
Sources: code.claude.com/docs/en/hooks, developers.openai.com/codex/hooks (Codex hooks ARE official in 2026).
- **`Done` is unreachable in production** — completion is derived from `task_complete`/`reason`/`subtype`
  fields that do NOT exist on real Stop payloads (REFUTED). Every finished turn resolves to `Idle`,
  never `Done`. `claude.rs:225`, `codex.rs:287`, `claude_infer.rs:337`, `claude_shim.rs:356`.
- **Codex `dead` via `SessionEnd` never fires** — `SessionEnd` isn't in Codex's event set; death is
  timeout-only. `codex.rs:493`.
- **Approval *response*-as-input path is fictional** — `decision` is hook *output*, not an inbound
  event; all `RawDecision` inbound parsing is dead (codex's variant even lacks `behavior`).
  `codex.rs:168`, `claude_shim.rs:139`.
- **Transcript-corroboration subsystem is entirely dead in prod** — `serve.rs` never reads
  `transcript_path`/calls `corroborate*`; the "inferred-waiting corroborated by JSONL" flagship is
  debounce-only. Two duplicate `Corroboration` types + JSONL scanners. `transcript.rs`, `claude_infer.rs`.
> These are the crown-jewel correctness gaps. Fixing needs a decision (see Questions): is Done/dead
> detection meant to work, or is the alpha knowingly debounce-only? The parsing of non-existent fields
> should go regardless.

### T1.2 Hub: blocking SQLite I/O inside the global async mutex
`persist.rs:148` under `server.rs:75`. Every delta holds `Mutex<StateStore>` across a *blocking*
`rusqlite` write (no `spawn_blocking` anywhere) — a slow disk stalls all reporters + faces + snapshot
reads. CONFIRMED (tokio: never block in async). Fix: `spawn_blocking`/dedicated DB thread, or narrow
the lock. **M.**

### T1.3 Hub: mute/solo mutate memory *before* persist; append failure only logged
`persist.rs:452,498`. `apply_mute/unmute/solo` update memory then persist, swallowing append errors —
so a mute is broadcast + in-memory but non-durable, vanishing on restart. Directly contradicts the
module's own "memory and log never diverge" doc (`server.rs:93-97`). `apply_session_upsert` does it
right (append-first). Fix: append-first for these paths. **S–M.**

### T1.4 Host: heavy blocking work runs on the UI thread in a sync Tauri command
`mux.rs:399` → `spawn.rs:207`. `spawn_server_with_options` (sync `#[tauri::command]`) runs
`git clone`, `code serve-web --help`, `code --install-extension`, port scan, dir writes — all
synchronously on the main thread. CONFIRMED (Tauri v2: sync commands block the UI). A spawn hangs the
whole window for seconds. Fix: `#[tauri::command(async)]` + `spawn_blocking`. **M.**

### T1.5 Host: bridge accept loops die permanently on the first transient `accept()` error
`bridge.rs:356,381`. `while let Ok((s,_)) = listener.accept().await` — any `Err` (EMFILE/ENFILE, peer
reset in-queue) ends the loop and kills phone-home for the process lifetime. CONFIRMED (tokio docs:
these errors are non-fatal). Fix: `loop { match … Err(e) => { warn; continue } }`. **S.**

### T1.6 Hub: empty session keeps stale `rollup_state` → stuck `unread` badge
`merge.rs:78`. `recompute_rollups` only overwrites when ≥1 run; removing the last run leaves the old
`Waiting` rollup + armed `unread`. Fix: reset to idle sentinel on empty. **S–M.**

### T1.7 Node: top-level `window.__TAURI__` destructure can throw and kill the whole rail
`main.js:6-7`. Runs at module-load before `__TAURI__` is guaranteed populated (CONFIRMED tauri#12990,
Windows-reported — matches the "Windows = no agent state" note). A `TypeError` there means `init()`
never runs. Fix: defer behind `window.load` / poll. **S.**

### T1.8 Host: native menu is frozen — all dynamic-menu machinery is dead
`mux.rs:1053`. `refresh_menu` is `{ let _ = app; }` (no-op); the menu is built once with an empty
server list and never rebuilt (no `set_menu` call). So `build_server_menu`, `RailMenuState`, per-server
items, Close/Open-Current enable-state are test-only; native menu server-switching can never fire.
CONFIRMED (`AppHandle::set_menu` exists — it's fixable, not a platform limit). Fix: rebuild on change,
or delete the dead machinery. **M.**

---

## TIER 2 — Cross-cutting duplication (one seam, repeated) — refactor value

### T2.1 Hand-rolled ISO-8601 parsers, 3 copies, all fragile the same way
`sort.rs:95` (host-core), `persist.rs:619` (hub), `fake.rs:162` (reporter, and it's the *production*
timestamp source). All hard-code `Z`, reject offsets, silently degrade (sort ranks unparseable as
age-0). Fix: one shared correct time helper, or adopt `jiff`/`time` (deps are otherwise minimal by
design — a deliberate call). **S–M each; high leverage.**

### T2.2 Reporter: four parallel agent state machines ~80% copy-paste
`claude.rs`/`claude_shim.rs`/`codex.rs`/`claude_infer.rs` — `Transition`×4, `to_run`, adapter
boilerplate, `RawDecision` table ×2. One generic lifecycle core + thin adapters collapses ~1000 lines
and makes T1.1 a one-place fix instead of four. **L.**

### T2.3 Hub: delta vocabulary triplicated
`ClientMessage` vs `PersistEvent` vs `Event` encode the same mutation set + two near-identical match
ladders (`server.rs:98`, `persist.rs:233`). One internal `Mutation` type projected to each. **M.**

### T2.4 Node: repeated UI boilerplate
3× optimistic-update-with-rollback (`main.js:1217`), 5× row-action guards (`main.js:1413`), 2 menu
renderers with copy-pasted viewport clamp (`main.js:977,1041`). Extract `runOptimistic`,
`guardRowAction`, `clampToViewport`/`renderMenu`. **S–M.**

### T2.5 Smaller dup seams
CLI `connect_ws`/`connect_unix` read-loops (diverged) `connection.rs:47/111`; rollup-recompute copy
(`render.rs:211/230`); host `first_server_id` re-implements `servers_for_app` (`mux.rs:934`); hub
`Urgency::None` normalization ×2. **S each.**

---

## TIER 3 — Dead code / mechanical cleanups (safe, high-confidence, low-risk)

- **Dead deps:** `serde` unused + `serde_json` misplaced in host-core `[dependencies]` (Cargo.toml:29);
  `thiserror` unused in fleet-protocol (Cargo.toml:29). **S.**
- **Dead code:** node `removeRow` (`main.js:1496`) + `rowFlags` passthrough; host-core `confidence.rs`
  `BadgeMarker`/`badge_for` (zero consumers); reporter `corroborate_transcript` alias.
- **Stale annotations:** host `#[allow(dead_code)]` on live `send_command` (`bridge.rs:83`); host-core
  `#[coverage(off)]` on tested `focus_editor` (`editors.rs:310`).
- **Broken tooling:** node `npm run check` → missing `scripts/check-*.mjs` in `ui/` (dead guard).
- **Magic constants:** host `ws_port` fallback `51777` should be `fleet_hub::DEFAULT_WS_PORT`
  (`spawn.rs:1828`); CLI `sort_key` `255 - rank` → `cmp::Reverse` (`render.rs:137`).
- **clippy:** host-core `sort_tab_refs(&mut Vec)` → `&mut []` (ptr_arg) (`sort.rs:185`).
- **Poison-handling inconsistency:** host supervisor `.lock().unwrap()` in hot paths while read paths
  recover; give it the `lock_map()` recovery helper everywhere (`spawn.rs:325+`). **S.**

## TIER 4 — Build / supply-chain / CI

- **T4.1 [HIGH] No committed JS lockfiles + `--no-frozen-lockfile`/`npm install` everywhere** — VSIX +
  E2E resolve transitive deps fresh each run: non-reproducible + supply-chain exposure. Commit
  lockfiles, switch to `--frozen-lockfile`/`npm ci`. **M.**
- **T4.2 [MED/HIGH] No `cargo audit`/`cargo deny` gate** — Dependabot only nudges; a new advisory in a
  pinned crate fails nothing. Add an advisories job. **S.**
- **T4.3 [LOW/MED] `tauri` lock 3 patches behind** (2.11.2 → 2.11.5); `cargo update -p tauri`. **S.**
- **T4.4 [LOW/MED] Redundant CI** — ci.yml 80% coverage job is subsumed by coverage.yml's 100% gate;
  standalone `cargo build` before `cargo test` rebuilds the graph. Trim. **S.**
- **T4.5 [LOW] Release pulls `npx @tauri-apps/cli@^2` unpinned** in all 6 lanes; pin exact. **S.**

## TIER 5 — Test smells (systemic; won't find bugs, but the gate is being gamed)
Coverage-driven contortions across host-core/reporter/hub: `#[coverage(off)]` on real (tested) logic,
a whole `open_read_only`/`with_trace` mechanism built only to hit format regions, `expect_fire`
deriving its expectation from the production fn, `Debug`-string equality, `len==20`/`is_empty` asserts
on literals, `debug_assert_eq!` (compiled out in `--release`). Worth a dedicated cleanup pass.

---

## Recommended execution order
1. **Tier 3 mechanical batch** (dead deps/code, stale annotations, magic constants, clippy, poison
   helper) — safe, behind green CI. ~1 batch.
2. **Tier 1 correctness** — but T1.1 (agent contracts) needs your call first (below). T1.2–T1.8 are
   clear fixes; do them with regression tests.
3. **Tier 4 build/supply-chain** (lockfiles + cargo-audit gate) — independent, high value.
4. **Tier 2 cross-cutting refactors** — largest; T2.1 (time helper) and T2.2 (reporter machines) first
   since they unblock T1.1's fix.

## Questions for you (gate the big/ambiguous work)
- **Agent-contract drift (T1.1):** is Done/dead detection *meant* to work today, or is the alpha
  knowingly debounce-only? That decides delete-dead-parsing vs. build-real-detection (transcript read).
- **Date dependency (T2.1):** OK to add `jiff` (or `time`) to kill the 3 hand-rolled parsers, or keep
  zero-date-dep and just harden them in place?
- **Refactor appetite:** green-light Tiers 3 + 4 for me to apply now behind CI, and I hold Tiers 1–2
  for per-item review? Or a different cut.
