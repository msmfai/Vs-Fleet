# L2.FLOW — Agent-state flow end-to-end (env claude → reporter → Hub → rail badge/ping)

The spine that makes Fleet useful: a `claude` running INSIDE an env drives the rail tab
through working/waiting/idle/done and pings on waiting. The chain is:

```
  claude (env terminal, shim/shell-fn adds --settings hooks)
    → hook fires → printf 'claude <json>' | nc -U <reporter.sock>      (entrypoint.sh / hooks.json)
    → fleet-reporter --serve : parse_frame → ClaudeAdapter (S15: working/idle/done)
                                          ⊕ ClaudeInferAdapter (S16: PreToolUse-without-Stop ⇒ waiting on a tick)
    → ReporterCommand::UpsertRun → WS phone-home → Hub (ws://host:51777)
    → Hub merges run into the session (rollup_state = most-urgent across runs, rollup.rs)
    → fleet-host hub_client subscribes → InboxModel reducer → RenderedInbox
    → rail row: state glyph (▶ ⏸ · ✓ ✕ ☠), attention=true only for waiting, waiting_count → title badge
```

Two layers read the Hub: the eval harness reads it via the `fleet ls --once` CLI
(`behaviours/agentInput.mjs` `hubSnapshot`), and `fleet-host`'s `hub_client` folds the
same wire into `RenderedInbox`. The CLI path is **implemented** by `agent.claudeRuns` +
`agent.waitingState`; the `fleet-host` rail-render path is the missing L2 lane (needs a
host-harness that boots `fleet-host` and reads `RenderedInbox`/the rail DOM). State enum
+ rollup precedence (`Waiting>Error>Working>Done>Idle>Dead`; only `Waiting` pings) is
unit-locked in `fleet-protocol`. Confidence honesty: inferred waiting is always
`Inferred`, never `High` (invariant 5).

---

### L2.FLOW.001 — `claude -p` in an env drives its Hub session working→idle (CLI face)
- layer: L2
- scenarios: [base, small-repo]
- isolation: fresh
- needs: [termSend]
- precondition: env booted, its `fleet-reporter --serve` phoned home (a session titled
  `FLEET_SERVER_ID == env.id` exists, `[idle]` on `fleet ls --once`); container claude
  authenticated (else SKIP).
- action: `termSend 'claude -p "say hi"\n'` into a terminal (the `claude` shell wrapper
  adds the Fleet hooks).
- expected: the session goes active (working|waiting) then terminates (idle|done|dead);
  ≥1 run recorded.
- assert: poll `fleet ls --once` (matching the row whose title == env.id): saw
  `working`|`waiting`, then saw `idle`|`done`|`dead`, and the row shows `(N run[s])`.
- machine-state: +1 claude process during the run; reporter socket carried ≥2 frames
  (UserPromptSubmit, Stop).
- edges: unauthenticated claude → clean SKIP (env, not regression); Hub/CLI absent →
  clean SKIP; see FLOW.011 (concurrent), FLOW.012 (repeat run).
- why: guards the whole observability chain (container claude → hooks → reporter S15 →
  WS phone-home → Hub session registry → CLI render); a break = Fleet blind to agents.
- status: implemented (behaviour `agent.claudeRuns`)

### L2.FLOW.002 — Inferred waiting (PreToolUse-without-Stop) reaches the Hub as `waiting`
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: []
- precondition: env booted, reporter `--serve` socket live at `/tmp/fleet-reporter.sock`,
  session registered on the Hub.
- action: inject a single controlled frame straight to the socket:
  `printf 'claude {"hook_event_name":"PreToolUse","session_id":"wait-<id>","tool_name":"Bash",…}' | nc -N -U /tmp/fleet-reporter.sock`
  (no following Stop), exercising the S16 infer tick.
- expected: after the `INFER_TICK_INTERVAL` (250ms) advances the clock past
  `DEFAULT_DEBOUNCE_MS`, `ClaudeInferAdapter` emits `UpsertRun(Waiting, Inferred)`; the
  Hub session reaches `[waiting]`.
- assert: poll `fleet ls --once` until the session shows state `waiting`; then inject a
  `Stop` frame so it resolves back to idle (leave the env clean).
- why: inferred-waiting is Fleet's entire ping and the ONLY e2e guard that serve's
  tick-driven inference + urgency/rollup plumbing + CLI `[waiting]` rendering actually
  fire (the Rust tests exercise the infer machine in isolation, not the socket→Hub→CLI
  path); deterministic injection chosen over a flaky real claude block.
- status: implemented (behaviour `agent.waitingState`)

### L2.FLOW.003 — Activity before the debounce cancels the pending waiting inference
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: []
- precondition: reporter `--serve` live; session registered.
- action: inject `PreToolUse(session=cancel-<id>)`, then within < `DEFAULT_DEBOUNCE_MS`
  inject a `Stop` (or any further frame) for the same session.
- expected: the infer adapter's arm is cancelled before the tick fires — the Hub session
  NEVER shows `waiting` for that frame; it settles at `idle`.
- assert: poll `fleet ls --once` for ~3× the debounce window: the session state is never
  `waiting` (only `working`→`idle`); contrast with FLOW.002 which DID reach waiting.
- why: the cancel path must suppress false pings (activity resumed before the human was
  actually blocked); guards the infer adapter's arm-then-cancel through the live socket,
  not just the unit `activity_before_debounce_cancels_the_inference`.
- status: implemented (behaviour `flow.cancelBeforeDebounce`)

### L2.FLOW.004 — The waiting session's rollup pings; the title badge increments (rail face)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: `fleet-host` running + subscribed to the Hub; one env session `idle`
  (rail shows one row, `waiting_count==0`, title badge 0).
- action: drive that session to `waiting` (FLOW.002 injection).
- expected: `hub_client` folds the delta → `RenderedInbox`: the row's `state=="waiting"`,
  `attention==true`, `state_glyph=="⏸"`, `urgency` set; `waiting_count==1`.
- assert: read `get_inbox` (Tauri command) / the emitted `inbox` event: the tab has
  `attention:true` and `waiting_count==1`; the rail DOM marks the row attention-styled
  and the window title shows a 1-badge.
- why: the ping must reach the user's eyes — the rail render + title badge is where
  Fleet pays off; guards `render.rs` `attention`/`waiting_count` + the badge wiring
  (the `agent.waitingState` behaviour only proves the CLI face, not the rail face).
- status: partial(the Hub→CLI `[waiting]` is proven by `agent.waitingState`; the
  `fleet-host` `RenderedInbox`/badge face is not — needs host-harness)

### L2.FLOW.005 — Only `waiting` is attention; working/idle/done/error never ping
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: `fleet-host` subscribed; one env session.
- action: drive the session through working (UserPromptSubmit), then idle (Stop), then
  inject a Done/Error-shaped frame; observe each.
- expected: at each non-waiting state the rendered tab has `attention==false` and does
  NOT contribute to `waiting_count` (`State::pings()` true only for Waiting; `render.rs`
  `attention = state.is_attention()`).
- assert: `get_inbox`: across working/idle/done/error the tab's `attention` stays false
  and `waiting_count==0`; glyphs map working=▶ idle=· done=✓ error=✕.
- why: only the genuinely-blocked state may demand attention — a working agent must not
  cry wolf; guards the single-pinging-state invariant end-to-end.
- status: TODO (the unit `only_waiting_pings` covers the enum; the rendered-face path is
  not exercised)

### L2.FLOW.006 — Done is kept DISTINCT from Idle through the whole chain (D9)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: env session live.
- action: drive a run that reports task-complete (`done`) vs one that merely finishes a
  turn (`idle`).
- expected: `done` and `idle` are never collapsed: distinct wire tokens (`"done"` vs
  `"idle"`, `state.rs` test `state_wire_tokens`), distinct glyphs (✓ vs ·), distinct
  rollup rank (done=2 > idle=1).
- assert: the rendered tab `state` reads `"done"` for the completed run and `"idle"`
  for the turn-finished run — never the same; `fleet ls` shows `[done]` vs `[idle]`.
- why: D9 is locked — "task complete" must read differently from "waiting for next
  prompt"; guards that no layer (reporter, Hub, render) silently merges them.
- status: TODO (enum distinctness unit-tested; the e2e claude→done→render path is not)

### L2.FLOW.007 — Session rollup = most-urgent state across its runs
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: one env session with TWO runs (two claude/agent threads under the same
  session id), one `working`, one driven to `waiting`.
- action: hold one run `working` while injecting `PreToolUse`-without-Stop for the other.
- expected: the session's `rollup_state` is `waiting` (the most-urgent across runs,
  `rollup.rs` `state_rank` Waiting=5 > Working=3); the rail shows ONE row in `waiting`
  with `run_count==2`.
- assert: `fleet ls --once` row shows `[waiting]` and `(2 runs)`; `get_inbox` tab has
  `run_count==2`, `state=="waiting"`, `attention==true`.
- why: a session with several agents must surface the worst — if any sub-run is blocked,
  the tab pings; guards the rollup precedence end-to-end (not just the `rollup.rs`
  unit `waiting_beats_working`).
- status: TODO

### L2.FLOW.008 — Rollup urgency = most-urgent across runs (approval beats question)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: a session with two waiting runs, one urgency `question`, one `approval`.
- action: inject the two waiting frames with differing urgencies.
- expected: `rollup_urgency == approval` (`rollup.rs` `urgency_rank` Approval=3 >
  Question=2); the tab's `urgency` field renders `approval`.
- assert: `get_inbox` tab `urgency == "approval"`; never `question` while an approval
  is pending.
- why: the loudest tier wins the tab so the user triages the most-blocking ask first;
  guards urgency rollup end-to-end (unit: `approval_is_most_urgent`).
- status: TODO

### L2.FLOW.009 — Inferred waiting carries Confidence::Inferred to the face (never High)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: env session live.
- action: drive `waiting` via the S16 PreToolUse-without-Stop inference (FLOW.002).
- expected: the resulting run's `confidence == Inferred` (never `High` — invariant 5,
  `serve.rs` test `pretool_then_tick_past_debounce_infers_waiting`); the rendered tab's
  `confidence` field reads `inferred`.
- assert: `get_inbox` tab `confidence == "inferred"` for the inferred-waiting; an
  authoritative-source waiting (if ever produced) would read `high` — the inferred path
  must never upgrade.
- why: honesty about how Fleet knows a waiting — heuristic must be labeled heuristic so
  the UI can be appropriately tentative; guards confidence flows through unaltered.
- status: TODO

### L2.FLOW.010 — last_message preview surfaces the rolled-up run's last line
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: env session, a claude run that emits an idle/done with a message
  (e.g. "All tests pass.").
- action: complete a run that produces a final assistant message.
- expected: the rendered tab's `last_message` carries that line (the inbox preview),
  per `render.rs` `surfaces_the_rolled_up_run_last_message_as_preview`.
- assert: `get_inbox` tab `last_message` contains the run's final line; `None` when the
  run has no message.
- why: the rail preview is the at-a-glance "what did it say" — it must reflect the
  rolled-up run; guards the preview field flowing through render.
- status: TODO

### L2.FLOW.011 — Many envs' agents flow concurrently to distinct rail rows
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [termSend, host-harness]
- precondition: N=3 envs booted, all phoned home (3 idle rows); each claude authed.
- action: `termSend 'claude -p "say hi"\n'` in all 3 envs at once.
- expected: each env's run flows to ITS own session (keyed by distinct
  `FLEET_SERVER_ID`); the rail shows 3 rows each going active→settled independently; no
  cross-talk (one env's state never lands on another's row).
- assert: `fleet ls --once` shows 3 distinctly-titled rows each reaching `idle`/`done`;
  `get_inbox` has 3 tabs with distinct `session_id`s; state changes are partitioned by
  session.
- machine-state: 3 reporter sockets, 3 WS phone-home conns, 3 sessions.
- why: Fleet's whole pitch is N parallel agents in one inbox — the per-session keying
  must isolate them under concurrency; guards no state bleed across sessions.
- status: partial(`agent.claudeRuns` runs per-env in the matrix but the eval harness
  runs behaviours per-env serially; concurrent multi-env rail render needs host-harness)

### L2.FLOW.012 — A second run on the same session appears as run_count +1, not a new row
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [termSend]
- precondition: env session that already completed one `claude -p` run
  (FLOW.001 reached; row shows `(1 run)`).
- action: `termSend` a SECOND `claude -p "again"\n`.
- expected: the new run merges into the SAME session (same `FLEET_SERVER_ID`) — the row
  stays one tab, `run_count`/`(N runs)` increments; the Hub reclaims by durable id
  (claude `session_id`) rather than spawning a ghost session.
- assert: `fleet ls --once` row count for the env stays 1 and `(2 runs)` is shown after
  the second run; the row title is unchanged.
- why: repeated agent runs in one workspace must accumulate under one tab, not litter
  the rail; guards Hub session merge + the reclaim-no-ghost path end-to-end.
- status: implemented (behaviour `flow.secondRunMergesSession`)

### L2.FLOW.013 — A dead env's session goes `dead` (reporter gone past timeout)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: env session live (idle).
- action: `docker rm -f` the env (its reporter + WS link vanish).
- expected: past the liveness/grace window the run/session is marked `dead`
  (`State::Dead`, glyph ☠), then GC reaps it (see 21-hub-state for the reap rules); the
  rail row shows `dead` then disappears.
- assert: `fleet ls --once` shows the env row at `[dead]` after the grace window, then
  gone after the reap; `get_inbox` reflects the same.
- why: a vanished env must visibly die (not hang as idle forever) then be cleaned up;
  guards the liveness→dead→reap flow reaching the face.
- status: TODO

### L2.FLOW.014 — Hub-link drop shows last tabs as disconnected, not blanked (rail face)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: [host-harness]
- precondition: `fleet-host` subscribed; ≥1 env session rendered.
- action: stop/restart the Hub (drop the `hub_client` WS link).
- expected: `mark_disconnected` flips the last `RenderedInbox` to `connected:false`
  while KEEPING the tabs visible (not blanking); on reconnect a fresh `InboxModel` +
  resent snapshot reconciles (no stale accumulation).
- assert: `get_inbox().connected == false` and `tabs` still non-empty during the
  outage; after the Hub returns `connected == true` and tabs reflect the live snapshot.
- why: a transient Hub blip must not erase the user's inbox — degrade visibly, recover
  cleanly; guards `hub_client` reconnect + `mark_disconnected` end-to-end.
- status: TODO

### L2.FLOW.015 — No-network env: phone-home FAILS but the editor stays drivable
- layer: L2
- scenarios: [no-network]
- isolation: fresh
- needs: []
- precondition: env launched `--network none` — the reporter cannot reach the Hub on
  `ws://host:51777`; the in-container bridge/editor still run (driven via `docker exec`).
- action: boot the env; attempt a state flow (the reporter retries the Hub link).
- expected: NO session ever registers on the Hub (`fleet ls --once` never lists the env)
  — phone-home fails as expected; yet in-container commands still work (the bridge query
  answers, asserted via `exec`).
- assert: poll `fleet ls --once` for the budget: the env's session is ABSENT (this is
  the expected outcome, asserted, not a skip); an in-container `executeCommand` still
  succeeds (terminalCount +1 via the exec-driven bridge).
- why: agent-state flow must degrade cleanly when isolated — the editor remains usable
  even when Fleet can't see its agents; guards the reporter-unreachable boundary
  (contrast: a regression that hangs boot waiting for the Hub).
- status: partial(the `no-network` scenario exists in `scenarios/resourceFailure.mjs`
  and asserts boot; the explicit "Hub session absent yet bridge works" flow assertion
  is TODO)

### L2.FLOW.016 — Codex frames flow the same chain (cross-adapter parity)
- layer: L2
- scenarios: [base]
- isolation: fresh
- needs: []
- precondition: reporter `--serve` live; session registered.
- action: inject a `codex`-tagged frame (`printf 'codex {…thread.id…}'`) representing a
  Codex turn start, then a turn end.
- expected: `parse_frame` routes to `CodexAdapter` (the tag selects the adapter — the
  two payload shapes overlap so the sender declares it); the run flows to the Hub the
  same way claude does, the rail tab's `agent` reads `codex`.
- assert: `fleet ls --once`/`get_inbox`: a session appears with `agent=="codex"` going
  working→idle; the `claude`-tagged and `codex`-tagged frames produce the right adapter
  routing (no cross-contamination).
- why: the flow must be agent-agnostic at the Hub/face — Codex and Claude both light up
  the rail; guards the tagged-dispatch in `serve.rs` `dispatch` reaching the face.
- status: TODO
